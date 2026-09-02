//! The archive and its exports, against a real object store.
//!
//! Every test here is one of ADR_0021's acceptance criteria, written as a
//! sentence: a default deployment keeps nothing, an operator can prove why a
//! row was eligible, deletion propagates, a resumed export neither duplicates
//! nor omits, and the reference it produces is reproducible.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use aiwatcher_conversations::{
    ArchivePolicy, ConsentRecord, ContentPart, ContentPolicy, Error, ExclusionReason, ExportFormat,
    ExportRequest, ExportSelection, JobState, Keyring, LEASE_SECONDS, LawfulBasis, PolicyMode,
    PreferenceLabel, Provenance, RecordTurnRequest, RedactionRecord, Registry, RetentionPolicy,
    ReviewRequest, ReviewState, Role, TrainingScope, TurnContent, TurnFilter, TurnState,
};
use aiwatcher_core::prompts::ObjectStore;
use aiwatcher_prompts::adapters::memory::MemoryObjectStore;
use time::{Duration, OffsetDateTime};

const KEY: [u8; 32] = [42; 32];
/// What a worker calls itself. A pod name in a cluster.
const WORKER: &str = "worker-a";

fn store() -> Arc<MemoryObjectStore> {
    Arc::new(MemoryObjectStore::new())
}

fn registry_with(store: Arc<MemoryObjectStore>, policy: ArchivePolicy) -> Registry {
    Registry::new(store, "conversations", Keyring::single("k1", KEY), policy)
}

fn registry() -> Registry {
    registry_with(store(), ArchivePolicy::default())
}

fn consent() -> ConsentRecord {
    ConsentRecord {
        subject: "tenant-17".to_owned(),
        basis: LawfulBasis::Consent,
        reference: "ticket-4102".to_owned(),
        granted_at: None,
        scope: vec![TrainingScope::Train],
    }
}

fn policy() -> ContentPolicy {
    ContentPolicy {
        consent: consent(),
        retention: RetentionPolicy::default(),
        redaction: Some(RedactionRecord::named("acme-scrubber@2.1")),
    }
}

fn turn(conversation: &str, message: &str, role: Role, text: &str) -> RecordTurnRequest {
    RecordTurnRequest {
        conversation_id: conversation.to_owned(),
        message_id: message.to_owned(),
        parent_message_id: None,
        ordinal: 0,
        role,
        content: TurnContent {
            parts: vec![ContentPart::Text {
                text: text.to_owned(),
            }],
            tool_results: Vec::new(),
        },
        provenance: Provenance {
            run_id: "run-1".to_owned(),
            model: "provider-model".to_owned(),
            ..Provenance::default()
        },
        policy: policy(),
        occurred_at: None,
    }
}

fn approve() -> ReviewRequest {
    ReviewRequest {
        state: ReviewState::Approved,
        note: String::new(),
        preference: None,
        findings: Vec::new(),
    }
}

/// One approved exchange in one conversation.
async fn exchange(registry: &Registry, conversation: &str) {
    for (message, role, text) in [
        ("m1", Role::User, "what is the weather?"),
        ("m2", Role::Assistant, "nine degrees"),
    ] {
        let recorded = registry
            .record(turn(conversation, message, role, text))
            .await
            .expect("records");
        registry
            .review(
                conversation,
                &recorded.turn_id,
                "ada@example.com",
                &approve(),
            )
            .await
            .expect("reviews");
    }
}

fn export_request(name: &str) -> ExportRequest {
    ExportRequest {
        name: name.to_owned(),
        description: String::new(),
        format: ExportFormat::Chat,
        selection: ExportSelection::default(),
        require_human_review: true,
        required_scope: TrainingScope::Train,
        exclude_findings: vec![
            aiwatcher_conversations::FindingKind::Secret,
            aiwatcher_conversations::FindingKind::Pii,
            aiwatcher_conversations::FindingKind::Unsafe,
        ],
        roles: Vec::new(),
    }
}

// ── Writing ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_protected_deployment_refuses_a_turn_that_claims_no_basis() {
    let registry = registry();
    let mut request = turn("c1", "m1", Role::User, "hello");
    request.policy = ContentPolicy::default();

    let error = registry.record(request).await.expect_err("refused");
    let Error::Rejected(problems) = error else {
        panic!("expected a rejection, got {error}");
    };
    // Every problem at once, rather than one per round trip.
    assert_eq!(problems.len(), 5, "{problems:?}");
    assert!(problems.iter().any(|p| p.contains("basis")));
    assert!(problems.iter().any(|p| p.contains("redaction")));
}

#[tokio::test]
async fn an_open_deployment_records_the_gap_instead_of_refusing_it() {
    let registry = registry_with(
        store(),
        ArchivePolicy {
            mode: PolicyMode::Open,
            ..ArchivePolicy::default()
        },
    );
    let mut request = turn("c1", "m1", Role::User, "hello");
    request.policy = ContentPolicy::default();
    let recorded = registry.record(request).await.expect("records");

    let turn = registry.turn("c1", &recorded.turn_id).await.expect("reads");
    assert_eq!(turn.policy.consent.basis, LawfulBasis::Unknown);
    // And the gap is what an export excludes on, in a manifest, forever.
    assert!(!turn.policy.consent.basis.is_stated());
}

#[tokio::test]
async fn nothing_in_the_bucket_holds_the_words() {
    // The property the whole crate exists for: a copy of the object store is
    // not a copy of the conversations.
    let store = store();
    let registry = registry_with(Arc::clone(&store), ArchivePolicy::default());
    registry
        .record(turn(
            "c1",
            "m1",
            Role::User,
            "my card is 4111 1111 1111 1111",
        ))
        .await
        .expect("records");

    for entry in store.list("").await.expect("lists") {
        let bytes = store
            .get(&entry.key)
            .await
            .expect("reads")
            .expect("present");
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("4111"), "{} leaked content", entry.key);
        assert!(!text.contains("my card"), "{} leaked content", entry.key);
    }
}

#[tokio::test]
async fn a_credential_in_the_content_is_found_and_counted_without_being_stored() {
    let registry = registry();
    let recorded = registry
        .record(turn(
            "c1",
            "m1",
            Role::Tool,
            "AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE",
        ))
        .await
        .expect("records");

    assert_eq!(recorded.findings.len(), 1);
    assert_eq!(recorded.findings[0].rule, "aws-access-key-id");

    // And the conversation's counts say so without anything being decrypted.
    let head = registry.conversation("c1").await.expect("reads");
    assert_eq!(head.findings.get("secret"), Some(&1));
}

#[tokio::test]
async fn re_sending_the_same_turn_is_one_turn_and_keeps_its_approval() {
    let registry = registry();
    let first = registry
        .record(turn("c1", "m1", Role::User, "hello"))
        .await
        .expect("records");
    registry
        .review("c1", &first.turn_id, "ada@example.com", &approve())
        .await
        .expect("reviews");

    let again = registry
        .record(turn("c1", "m1", Role::User, "hello"))
        .await
        .expect("records");
    assert_eq!(again.turn_id, first.turn_id);
    assert!(!again.created);
    assert_eq!(again.review.state, ReviewState::Approved);

    let head = registry.conversation("c1").await.expect("reads");
    assert_eq!(head.turns, 1, "a retried flush must not double the corpus");
    assert_eq!(head.approved, 1);
    assert_eq!(head.pending, 0);
}

#[tokio::test]
async fn editing_a_turns_content_takes_its_approval_away() {
    // Carrying an approval across an edit is how reviewed text becomes
    // unreviewed text with a tick beside it.
    let registry = registry();
    let recorded = registry
        .record(turn("c1", "m1", Role::Assistant, "nine degrees"))
        .await
        .expect("records");
    registry
        .review("c1", &recorded.turn_id, "ada@example.com", &approve())
        .await
        .expect("reviews");

    let edited = registry
        .record(turn(
            "c1",
            "m1",
            Role::Assistant,
            "nine degrees, and my card is…",
        ))
        .await
        .expect("records");
    assert_eq!(edited.turn_id, recorded.turn_id);
    assert_eq!(edited.review.state, ReviewState::Pending);
}

#[tokio::test]
async fn the_same_words_under_a_second_message_id_are_a_duplicate_finding() {
    let registry = registry();
    registry
        .record(turn("c1", "m1", Role::User, "hello"))
        .await
        .expect("records");
    let second = registry
        .record(turn("c1", "m2", Role::User, "hello"))
        .await
        .expect("records");
    assert!(
        second
            .findings
            .iter()
            .any(|finding| finding.rule == "same-content"),
        "{:?}",
        second.findings
    );
}

#[tokio::test]
async fn a_reviewers_finding_survives_the_producers_next_flush() {
    let registry = registry();
    let recorded = registry
        .record(turn("c1", "m1", Role::Assistant, "an answer"))
        .await
        .expect("records");
    registry
        .review(
            "c1",
            &recorded.turn_id,
            "ada@example.com",
            &ReviewRequest {
                state: ReviewState::Rejected,
                note: "unsafe".to_owned(),
                preference: None,
                findings: vec![aiwatcher_conversations::HumanFinding {
                    kind: aiwatcher_conversations::FindingKind::Unsafe,
                    rule: "self-harm".to_owned(),
                }],
            },
        )
        .await
        .expect("reviews");

    registry
        .record(turn("c1", "m1", Role::Assistant, "an answer"))
        .await
        .expect("records");
    let turn = registry.turn("c1", &recorded.turn_id).await.expect("reads");
    assert!(
        turn.findings
            .iter()
            .any(|finding| finding.rule == "self-harm"),
        "a human's judgement must not be erased by a re-scan: {:?}",
        turn.findings
    );
}

// ── Erasure and retention ────────────────────────────────────────────────────

#[tokio::test]
async fn erasing_a_subject_removes_the_words_and_keeps_the_record() {
    let store = store();
    let registry = registry_with(Arc::clone(&store), ArchivePolicy::default());
    exchange(&registry, "c1").await;

    let report = registry
        .erase_subject("tenant-17", "ada@example.com")
        .await
        .expect("erases");
    assert_eq!(report.turns_erased, 2);
    assert_eq!(report.conversations_touched, 1);

    let page = registry
        .turns("c1", &TurnFilter::default(), 0, 50)
        .await
        .expect("lists");
    assert_eq!(page.turns.len(), 2);
    for turn in &page.turns {
        assert_eq!(turn.state, TurnState::Erased);
        // The head is still an auditable record of what was there.
        assert!(!turn.content_digest.is_empty());
        assert!(turn.erasure.is_some());
        // And the ciphertext is actually gone from the bucket.
        let key = format!("conversations/content/{}.json", turn.turn_id);
        assert!(store.get(&key).await.expect("reads").is_none(), "{key}");
    }
}

#[tokio::test]
async fn reading_erased_content_says_erased_rather_than_missing() {
    let registry = registry();
    let recorded = registry
        .record(turn("c1", "m1", Role::User, "hello"))
        .await
        .expect("records");
    registry
        .erase_conversation("c1", "ada@example.com")
        .await
        .expect("erases");

    let error = registry
        .content("c1", &recorded.turn_id)
        .await
        .expect_err("refused");
    assert!(
        matches!(error, Error::Erased(_, _)),
        "an auditor asked for exactly this distinction, got {error}"
    );
}

#[tokio::test]
async fn the_sweep_removes_what_its_own_retention_ran_out_on() {
    let registry = registry();
    let mut short = turn("c1", "m1", Role::User, "hello");
    short.policy.retention = RetentionPolicy {
        ttl_days: 1,
        policy_id: "p-1".to_owned(),
    };
    registry.record(short).await.expect("records");
    // A second turn with the default thirty days, which must survive.
    registry
        .record(turn("c1", "m2", Role::Assistant, "hi"))
        .await
        .expect("records");

    let report = registry
        .sweep(OffsetDateTime::now_utc() + Duration::days(2))
        .await
        .expect("sweeps");
    assert_eq!(report.turns_erased, 1);

    let page = registry
        .turns("c1", &TurnFilter::default(), 0, 50)
        .await
        .expect("lists");
    let states: Vec<_> = page.turns.iter().map(|turn| turn.state).collect();
    assert_eq!(states, vec![TurnState::Erased, TurnState::Held]);
}

#[tokio::test]
async fn a_sweep_with_nothing_expired_opens_no_conversation() {
    let registry = registry();
    exchange(&registry, "c1").await;
    let report = registry
        .sweep(OffsetDateTime::now_utc())
        .await
        .expect("sweeps");
    assert_eq!(report.turns_erased, 0);
    assert_eq!(report.conversations_touched, 0);
}

#[tokio::test]
async fn a_retention_past_the_ceiling_is_shortened_and_the_turn_says_so() {
    let registry = registry_with(
        store(),
        ArchivePolicy {
            max_ttl_days: 7,
            ..ArchivePolicy::default()
        },
    );
    let mut request = turn("c1", "m1", Role::User, "hello");
    request.policy.retention.ttl_days = 3_650;
    let recorded = registry.record(request).await.expect("records");

    let turn = registry.turn("c1", &recorded.turn_id).await.expect("reads");
    assert!(turn.retention_clamped);
    assert_eq!(turn.policy.retention.ttl_days, 7);
}

// ── Exports ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn an_export_takes_the_approved_turns_and_names_everything_it_left_out() {
    let registry = registry();
    exchange(&registry, "c1").await;
    // A third turn nobody reviewed, and a fourth whose consent covers only
    // evaluation.
    registry
        .record(turn("c1", "m3", Role::Assistant, "unreviewed"))
        .await
        .expect("records");
    let mut narrow = turn("c1", "m4", Role::Assistant, "evaluation only");
    narrow.policy.consent.scope = vec![TrainingScope::Evaluate];
    let narrow = registry.record(narrow).await.expect("records");
    registry
        .review("c1", &narrow.turn_id, "ada@example.com", &approve())
        .await
        .expect("reviews");

    let job = registry
        .create_export(export_request("training/agent-turns"), "ada@example.com")
        .await
        .expect("queued");
    let job = registry
        .run_export(&job.job_id, WORKER)
        .await
        .expect("runs");

    assert_eq!(job.state, JobState::Completed);
    assert_eq!(job.counts.turns_considered, 4);
    assert_eq!(job.counts.turns_included, 2);
    assert_eq!(job.counts.turns_excluded, 2);
    assert_eq!(
        job.exclusions.get(ExclusionReason::NotReviewed.as_str()),
        Some(&1)
    );
    assert_eq!(
        job.exclusions
            .get(ExclusionReason::ScopeNotPermitted.as_str()),
        Some(&1)
    );
    // And each one is named, not just counted.
    assert_eq!(job.excluded.len(), 2);
}

#[tokio::test]
async fn every_exported_row_carries_the_reason_it_was_eligible() {
    let registry = registry();
    exchange(&registry, "c1").await;
    let job = registry
        .create_export(export_request("training/agent-turns"), "ada@example.com")
        .await
        .expect("queued");
    let job = registry
        .run_export(&job.job_id, WORKER)
        .await
        .expect("runs");
    let version = job.version.clone().expect("a completed export has one");

    let page = registry
        .export_rows("training/agent-turns", &version, 0, 10)
        .await
        .expect("reads");
    assert_eq!(page.total, 1);
    let eligibility = &page.rows[0]["eligibility"];
    assert_eq!(eligibility[0]["consent_basis"], "consent");
    assert_eq!(eligibility[0]["consent_reference"], "ticket-4102");
    assert_eq!(eligibility[0]["reviewer"], "ada@example.com");
    assert!(eligibility[0]["content_digest"].is_string());
}

#[tokio::test]
async fn an_exports_rows_are_sealed_in_the_bucket_too() {
    let store = store();
    let registry = registry_with(Arc::clone(&store), ArchivePolicy::default());
    exchange(&registry, "c1").await;
    let job = registry
        .create_export(export_request("training/agent-turns"), "ada@example.com")
        .await
        .expect("queued");
    registry
        .run_export(&job.job_id, WORKER)
        .await
        .expect("runs");

    for entry in store
        .list("conversations/exports/shards/")
        .await
        .expect("lists")
    {
        let bytes = store
            .get(&entry.key)
            .await
            .expect("reads")
            .expect("present");
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("nine degrees"), "{} leaked", entry.key);
    }
}

#[tokio::test]
async fn re_running_an_export_over_an_unchanged_archive_reaches_the_same_version() {
    // What "the reference is reconstructible" means: the version is a content
    // address of the corpus, so the same archive and the same request produce
    // it again. It is *not* a hash of the request alone — a turn approved by
    // somebody else, or approved at all, is a different corpus and says so.
    let store = store();
    let registry = registry_with(Arc::clone(&store), ArchivePolicy::default());
    exchange(&registry, "c1").await;

    let first = registry
        .create_export(export_request("training/agent-turns"), "ada@example.com")
        .await
        .expect("queued");
    let first = registry
        .run_export(&first.job_id, WORKER)
        .await
        .expect("runs");

    // Ask for the same thing again. A finished job does not absorb the second
    // request — that is what makes "export again now that more turns are
    // reviewed" work — so this really does re-read the archive.
    let second = registry
        .create_export(export_request("training/agent-turns"), "ada@example.com")
        .await
        .expect("queued");
    assert_ne!(second.job_id, first.job_id, "a finished job is not reused");
    let second = registry
        .run_export(&second.job_id, WORKER)
        .await
        .expect("runs");

    assert!(first.version.is_some());
    assert_eq!(first.version, second.version);
    // And the name still has one version, not two identical ones.
    let exports = registry.exports().await.expect("lists");
    assert_eq!(exports.exports[0].versions.len(), 1);
}

#[tokio::test]
async fn queueing_the_same_export_twice_joins_the_job_it_already_started() {
    // A retried POST must not start a second export of the same thing. This is
    // the *unfinished* case; the finished one is a new job, tested above.
    let registry = registry();
    exchange(&registry, "c1").await;
    let first = registry
        .create_export(export_request("training/agent-turns"), "ada@example.com")
        .await
        .expect("queued");
    let second = registry
        .create_export(export_request("training/agent-turns"), "ada@example.com")
        .await
        .expect("queued");
    assert_eq!(first.job_id, second.job_id);
}

#[tokio::test]
async fn exporting_again_after_more_turns_are_reviewed_produces_a_new_corpus() {
    // The failure this prevents is the one that is hardest to notice: the
    // request succeeds and hands back a corpus built before the review.
    let registry = registry();
    exchange(&registry, "c1").await;
    let first = registry
        .create_export(export_request("training/agent-turns"), "ada@example.com")
        .await
        .expect("queued");
    let first = registry
        .run_export(&first.job_id, WORKER)
        .await
        .expect("runs");
    assert_eq!(first.counts.turns_included, 2);

    let extra = registry
        .record(turn("c1", "m3", Role::Assistant, "a second answer"))
        .await
        .expect("records");
    registry
        .review("c1", &extra.turn_id, "ada@example.com", &approve())
        .await
        .expect("reviews");

    let second = registry
        .create_export(export_request("training/agent-turns"), "ada@example.com")
        .await
        .expect("queued");
    assert_ne!(second.job_id, first.job_id);
    let second = registry
        .run_export(&second.job_id, WORKER)
        .await
        .expect("runs");
    assert_eq!(second.counts.turns_included, 3);
    assert_ne!(second.version, first.version);
    assert_eq!(
        registry.exports().await.expect("lists").exports[0]
            .versions
            .len(),
        2
    );
}

#[tokio::test]
async fn an_export_of_nothing_is_refused_rather_than_published_as_an_empty_corpus() {
    let registry = registry();
    let error = registry
        .create_export(export_request("training/agent-turns"), "ada@example.com")
        .await
        .expect_err("refused");
    assert!(matches!(error, Error::Refused(_)), "{error}");
}

#[tokio::test]
async fn a_resumed_export_neither_duplicates_nor_omits_a_row() {
    // The acceptance criterion, and the reason a shard is written before the
    // cursor that passes it. 520 conversations is one full shard plus a
    // remainder, so there is a real resume point between them.
    let store = store();
    let registry = registry_with(Arc::clone(&store), ArchivePolicy::default());
    for index in 0..520 {
        let conversation = format!("c{index:04}");
        let recorded = registry
            .record(turn(&conversation, "m1", Role::User, &format!("q{index}")))
            .await
            .expect("records");
        registry
            .review(
                &conversation,
                &recorded.turn_id,
                "ada@example.com",
                &approve(),
            )
            .await
            .expect("reviews");
    }

    let job = registry
        .create_export(export_request("training/all"), "ada@example.com")
        .await
        .expect("queued");
    let uninterrupted = registry
        .run_export(&job.job_id, WORKER)
        .await
        .expect("runs");
    assert_eq!(
        uninterrupted.shards.len(),
        2,
        "one full shard and a remainder, so a resume point exists"
    );
    assert_eq!(uninterrupted.counts.rows, 520);

    // Now put the job back into the state a process killed straight after
    // committing its first shard would have left: one shard, the cursor just
    // past the conversations in it, `running`, and a lease that has run out
    // under a worker that is not coming back.
    let key = format!("conversations/exports/jobs/{}.json", job.job_id);
    let committed = uninterrupted.shards[0].rows;
    let mut stored: serde_json::Value =
        serde_json::from_slice(&store.get(&key).await.expect("reads").expect("present"))
            .expect("parses");
    stored["state"] = serde_json::json!("running");
    stored["claimed_by"] = serde_json::json!("worker-that-died");
    stored["claimed_at"] = serde_json::json!(
        (OffsetDateTime::now_utc() - Duration::seconds(LEASE_SECONDS + 60))
            .format(&time::format_description::well_known::Rfc3339)
            .expect("formats")
    );
    stored["cursor"] = serde_json::json!(committed);
    stored["shards"] = serde_json::json!([uninterrupted.shards[0]]);
    stored["version"] = serde_json::Value::Null;
    stored["finished_at"] = serde_json::Value::Null;
    stored["counts"] = serde_json::json!({
        "conversations": committed,
        "turns_considered": committed,
        "turns_included": committed,
        "turns_excluded": 0,
        "rows": committed,
    });
    store
        .put(&key, serde_json::to_vec(&stored).expect("serialises"))
        .await
        .expect("writes");

    let claimable = registry.claimable_exports().await.expect("lists");
    assert!(
        claimable.contains(&job.job_id),
        "a job left running by a dead process must be claimable again"
    );

    let resumed = registry
        .run_export(&job.job_id, WORKER)
        .await
        .expect("runs");
    assert_eq!(resumed.state, JobState::Completed);
    // Neither duplicated nor omitted: exactly the rows the uninterrupted run
    // produced, and the same content address over them.
    assert_eq!(resumed.counts.rows, 520);
    assert_eq!(resumed.shards.len(), 2);
    assert_eq!(resumed.version, uninterrupted.version);

    let page = registry
        .export_rows("training/all", &resumed.version.expect("completed"), 0, 200)
        .await
        .expect("reads");
    assert_eq!(page.total, 520);
}

#[tokio::test]
async fn an_erased_turn_is_excluded_by_name_from_the_next_export() {
    let registry = registry();
    exchange(&registry, "c1").await;
    registry
        .erase_subject("tenant-17", "ada@example.com")
        .await
        .expect("erases");

    let job = registry
        .create_export(export_request("training/agent-turns"), "ada@example.com")
        .await
        .expect("queued");
    let job = registry
        .run_export(&job.job_id, WORKER)
        .await
        .expect("runs");
    assert_eq!(job.counts.turns_included, 0);
    assert_eq!(
        job.exclusions.get(ExclusionReason::Erased.as_str()),
        Some(&2)
    );
}

#[tokio::test]
async fn a_cancelled_export_produces_no_version() {
    let registry = registry();
    exchange(&registry, "c1").await;
    let job = registry
        .create_export(export_request("training/agent-turns"), "ada@example.com")
        .await
        .expect("queued");
    let cancelled = registry.cancel_export(&job.job_id).await.expect("cancels");
    assert_eq!(cancelled.state, JobState::Cancelled);
    assert!(cancelled.version.is_none());

    // And running it afterwards changes nothing: an interrupted job never
    // appears as a completed dataset version.
    let after = registry
        .run_export(&job.job_id, WORKER)
        .await
        .expect("runs");
    assert_eq!(after.state, JobState::Cancelled);
    assert!(after.version.is_none());
    assert!(registry.exports().await.expect("lists").exports.is_empty());
}

#[tokio::test]
async fn a_preference_export_pairs_only_what_a_reviewer_labelled() {
    let registry = registry();
    let question = registry
        .record(turn("c1", "m1", Role::User, "which is better?"))
        .await
        .expect("records");
    registry
        .review("c1", &question.turn_id, "ada@example.com", &approve())
        .await
        .expect("reviews");

    for (message, text, preference) in [
        ("m2", "the better answer", Some(PreferenceLabel::Chosen)),
        ("m3", "the worse answer", Some(PreferenceLabel::Rejected)),
        ("m4", "an unlabelled answer", None),
    ] {
        let mut request = turn("c1", message, Role::Assistant, text);
        request.parent_message_id = Some("m1".to_owned());
        let recorded = registry.record(request).await.expect("records");
        registry
            .review(
                "c1",
                &recorded.turn_id,
                "ada@example.com",
                &ReviewRequest {
                    state: ReviewState::Approved,
                    note: String::new(),
                    preference,
                    findings: Vec::new(),
                },
            )
            .await
            .expect("reviews");
    }

    let mut request = export_request("training/preferences");
    request.format = ExportFormat::Dpo;
    let job = registry
        .create_export(request, "ada@example.com")
        .await
        .expect("queued");
    let job = registry
        .run_export(&job.job_id, WORKER)
        .await
        .expect("runs");
    let version = job.version.clone().expect("completed");

    let page = registry
        .export_rows("training/preferences", &version, 0, 10)
        .await
        .expect("reads");
    assert_eq!(page.total, 1, "one pair, not three");
    assert_eq!(page.rows[0]["chosen"], "the better answer");
    assert_eq!(page.rows[0]["rejected"], "the worse answer");
}

#[tokio::test]
async fn a_manifest_says_whether_its_exclusion_list_is_the_whole_story() {
    let registry = registry();
    exchange(&registry, "c1").await;
    registry
        .record(turn("c1", "m3", Role::Assistant, "unreviewed"))
        .await
        .expect("records");

    let job = registry
        .create_export(export_request("training/agent-turns"), "ada@example.com")
        .await
        .expect("queued");
    let job = registry
        .run_export(&job.job_id, WORKER)
        .await
        .expect("runs");
    let manifest = registry
        .export("training/agent-turns", &job.version.expect("completed"))
        .await
        .expect("reads");

    assert!(!manifest.excluded_truncated);
    assert_eq!(manifest.counts.turns_excluded, manifest.excluded.len());
    assert!(manifest.reference().contains('@'));
}

#[tokio::test]
async fn an_erasure_takes_the_rows_out_of_the_corpus_it_already_reached() {
    // The half of an erasure that is easy to forget. Erasing the archive and
    // leaving the published corpus is an erasure in name only: the words are
    // still readable through a different route, under a reference somebody has
    // written into a training run.
    let store = store();
    let registry = registry_with(Arc::clone(&store), ArchivePolicy::default());
    exchange(&registry, "c1").await;

    let job = registry
        .create_export(export_request("training/agent-turns"), "ada@example.com")
        .await
        .expect("queued");
    let job = registry
        .run_export(&job.job_id, WORKER)
        .await
        .expect("runs");
    let version = job.version.clone().expect("completed");
    assert_eq!(
        registry
            .export_rows("training/agent-turns", &version, 0, 10)
            .await
            .expect("reads")
            .total,
        1
    );

    let report = registry
        .erase_subject("tenant-17", "ada@example.com")
        .await
        .expect("erases");
    assert_eq!(report.corpora_withdrawn, 1);

    // The rows are gone, and the answer says so rather than 404ing.
    let error = registry
        .export_rows("training/agent-turns", &version, 0, 10)
        .await
        .expect_err("withdrawn");
    assert!(matches!(error, Error::Erased(_, _)), "{error}");

    // The shards are actually deleted from the bucket, not merely flagged.
    let shards = store
        .list("conversations/exports/shards/")
        .await
        .expect("lists");
    assert!(shards.is_empty(), "{shards:?}");

    // And the manifest survives, so a training run naming this reference still
    // resolves to something that can say what happened to it.
    let manifest = registry
        .export("training/agent-turns", &version)
        .await
        .expect("reads");
    assert_eq!(manifest.counts.rows, 1);
    let withdrawal = manifest.withdrawn.expect("withdrawn");
    assert_eq!(withdrawal.conversations, vec!["c1".to_owned()]);
    assert_eq!(withdrawal.by, "ada@example.com");

    let listed = registry.exports().await.expect("lists");
    assert!(listed.exports[0].versions[0].withdrawn);
}

#[tokio::test]
async fn a_corpus_that_read_none_of_the_erased_conversations_is_left_alone() {
    let registry = registry();
    exchange(&registry, "c1").await;
    exchange(&registry, "c2").await;

    let mut only_c2 = export_request("training/c2-only");
    only_c2.selection.conversations = vec!["c2".to_owned()];
    let job = registry
        .create_export(only_c2, "ada@example.com")
        .await
        .expect("queued");
    let job = registry
        .run_export(&job.job_id, WORKER)
        .await
        .expect("runs");
    let version = job.version.clone().expect("completed");

    let report = registry
        .erase_conversation("c1", "ada@example.com")
        .await
        .expect("erases");
    assert_eq!(report.corpora_withdrawn, 0);
    assert_eq!(
        registry
            .export_rows("training/c2-only", &version, 0, 10)
            .await
            .expect("still readable")
            .total,
        1
    );
}

#[tokio::test]
async fn the_retention_sweep_withdraws_a_corpus_the_same_way_a_request_does() {
    // Content that expired is content that may not be read, wherever it is —
    // and it arrives more quietly than an erasure request, because nobody filed
    // one and the clock simply passed.
    let registry = registry();
    let mut short = turn("c1", "m1", Role::User, "hello");
    short.policy.retention = RetentionPolicy {
        ttl_days: 1,
        policy_id: String::new(),
    };
    let recorded = registry.record(short).await.expect("records");
    registry
        .review("c1", &recorded.turn_id, "ada@example.com", &approve())
        .await
        .expect("reviews");

    let job = registry
        .create_export(export_request("training/short-lived"), "ada@example.com")
        .await
        .expect("queued");
    let job = registry
        .run_export(&job.job_id, WORKER)
        .await
        .expect("runs");
    let version = job.version.clone().expect("completed");

    let report = registry
        .sweep(OffsetDateTime::now_utc() + Duration::days(2))
        .await
        .expect("sweeps");
    assert_eq!(report.turns_erased, 1);
    assert_eq!(report.corpora_withdrawn, 1);
    assert!(
        registry
            .export_rows("training/short-lived", &version, 0, 10)
            .await
            .is_err()
    );
}

// ── The export lease ─────────────────────────────────────────────────────────
//
// Two deterministic workers over an *unchanged* archive converge: same cursor,
// same shard index, same bytes, same digest. The lease is for the case where
// they do not — a turn reviewed while both are running — where the last job
// record written would name digests that do not describe the stored shards.

/// The job record, straight out of the store.
async fn stored_job(store: &MemoryObjectStore, job_id: &str) -> serde_json::Value {
    let key = format!("conversations/exports/jobs/{job_id}.json");
    serde_json::from_slice(&store.get(&key).await.expect("reads").expect("present"))
        .expect("parses")
}

async fn put_job(store: &MemoryObjectStore, job_id: &str, job: &serde_json::Value) {
    store
        .put(
            &format!("conversations/exports/jobs/{job_id}.json"),
            serde_json::to_vec(job).expect("serialises"),
        )
        .await
        .expect("writes");
}

fn rfc3339(at: OffsetDateTime) -> serde_json::Value {
    serde_json::json!(
        at.format(&time::format_description::well_known::Rfc3339)
            .expect("formats")
    )
}

#[tokio::test]
async fn a_job_another_worker_is_holding_is_left_alone() {
    let store = store();
    let registry = registry_with(Arc::clone(&store), ArchivePolicy::default());
    exchange(&registry, "c1").await;
    let job = registry
        .create_export(export_request("training/agent-turns"), "ada@example.com")
        .await
        .expect("queued");

    // Another worker claimed it a minute ago and is still going.
    let mut stored = stored_job(&store, &job.job_id).await;
    stored["state"] = serde_json::json!("running");
    stored["claimed_by"] = serde_json::json!("worker-b");
    stored["claimed_at"] = rfc3339(OffsetDateTime::now_utc() - Duration::seconds(60));
    put_job(&store, &job.job_id, &stored).await;

    // It is not offered…
    assert!(
        !registry
            .claimable_exports()
            .await
            .expect("lists")
            .contains(&job.job_id)
    );
    // …and running it anyway changes nothing, which is what a rolling update
    // needs: the old pod is still exporting and the new one must not join in.
    let seen = registry
        .run_export(&job.job_id, WORKER)
        .await
        .expect("runs");
    assert_eq!(seen.state, JobState::Running);
    assert_eq!(seen.claimed_by, "worker-b");
    assert!(seen.version.is_none());
    assert!(registry.exports().await.expect("lists").exports.is_empty());
}

#[tokio::test]
async fn a_pod_that_restarted_reclaims_its_own_lease_immediately() {
    // The worker id is the pod name, so a restarted pod is the same name and a
    // different process. Waiting five minutes for a lease held by something
    // that is demonstrably gone would be five minutes of nothing happening.
    let store = store();
    let registry = registry_with(Arc::clone(&store), ArchivePolicy::default());
    exchange(&registry, "c1").await;
    let job = registry
        .create_export(export_request("training/agent-turns"), "ada@example.com")
        .await
        .expect("queued");

    let mut stored = stored_job(&store, &job.job_id).await;
    stored["state"] = serde_json::json!("running");
    stored["claimed_by"] = serde_json::json!(WORKER);
    stored["claimed_at"] = rfc3339(OffsetDateTime::now_utc() - Duration::seconds(10));
    put_job(&store, &job.job_id, &stored).await;

    let finished = registry
        .run_export(&job.job_id, WORKER)
        .await
        .expect("runs");
    assert_eq!(finished.state, JobState::Completed);
}

#[tokio::test]
async fn a_worker_that_lost_its_lease_stops_instead_of_writing_beside_its_replacement() {
    // 520 conversations is two shards, so there is a boundary at which to
    // notice. The takeover is staged between them: the first shard is
    // committed under this worker, then somebody else claims the job.
    let store = store();
    let registry = registry_with(Arc::clone(&store), ArchivePolicy::default());
    for index in 0..520 {
        let conversation = format!("c{index:04}");
        let recorded = registry
            .record(turn(&conversation, "m1", Role::User, &format!("q{index}")))
            .await
            .expect("records");
        registry
            .review(
                &conversation,
                &recorded.turn_id,
                "ada@example.com",
                &approve(),
            )
            .await
            .expect("reviews");
    }
    let job = registry
        .create_export(export_request("training/all"), "ada@example.com")
        .await
        .expect("queued");
    let done = registry
        .run_export(&job.job_id, WORKER)
        .await
        .expect("runs");
    let committed = done.shards[0].rows;

    // Rewind to just after the first shard, and hand the lease to somebody
    // else — which is what an expired lease being picked up looks like from
    // the original worker's side.
    let mut stored = stored_job(&store, &job.job_id).await;
    stored["state"] = serde_json::json!("running");
    stored["claimed_by"] = serde_json::json!("worker-b");
    stored["claimed_at"] = rfc3339(OffsetDateTime::now_utc());
    stored["cursor"] = serde_json::json!(committed);
    stored["shards"] = serde_json::json!([done.shards[0]]);
    stored["version"] = serde_json::Value::Null;
    stored["finished_at"] = serde_json::Value::Null;
    put_job(&store, &job.job_id, &stored).await;

    let stopped = registry
        .run_export(&job.job_id, WORKER)
        .await
        .expect("runs");
    assert_eq!(stopped.claimed_by, "worker-b");
    assert!(
        stopped.version.is_none(),
        "a worker that lost its lease must not finish the job it was told to stop touching"
    );
    // And it wrote nothing: the second shard is still missing.
    assert_eq!(
        store
            .list("conversations/exports/shards/")
            .await
            .expect("lists")
            .len(),
        2,
        "the two shards the first run wrote, and nothing from the second"
    );
}

#[tokio::test]
async fn a_failing_job_releases_its_lease_rather_than_waiting_for_it_to_expire() {
    // A requeued job should be picked up on the next tick, not in five minutes.
    let store = store();
    let registry = registry_with(Arc::clone(&store), ArchivePolicy::default());
    exchange(&registry, "c1").await;
    let job = registry
        .create_export(export_request("training/agent-turns"), "ada@example.com")
        .await
        .expect("queued");
    let job = registry
        .run_export(&job.job_id, WORKER)
        .await
        .expect("runs");

    let mut stored = stored_job(&store, &job.job_id).await;
    stored["state"] = serde_json::json!("queued");
    stored["claimed_by"] = serde_json::json!("");
    stored["claimed_at"] = serde_json::Value::Null;
    put_job(&store, &job.job_id, &stored).await;

    assert!(
        registry
            .claimable_exports()
            .await
            .expect("lists")
            .contains(&job.job_id)
    );
}
