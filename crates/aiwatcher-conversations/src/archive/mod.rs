//! One picture of the archive: a turn's plaintext head, its sealed content,
//! the order an export reads them in, and the clock that removes them.
//!
//! The **split between head and content** is the whole slice. Everything that
//! makes a turn findable, auditable and reviewable is plaintext — role,
//! ordering, provenance, policy, findings, review state, digests — and
//! everything anybody actually said is sealed. Three things follow, and each
//! is a thing this design can do that a single encrypted blob could not:
//!
//! * a review queue, a count of findings and an export's exclusion report are
//!   all readable without decrypting anything;
//! * erasing content leaves the head, so a turn that was in an export can still
//!   be *explained* after its words are gone;
//! * approving a turn rewrites 400 bytes rather than re-sealing a megabyte.
//!
//! The retention clock is this crate's alone. It is not the log's retention and
//! not the object store's lifecycle policy: those are sized for volume, and
//! this one is sized for what somebody was told when they agreed.

pub mod crypt;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::policy::{ArchivePolicy, ContentPolicy, TrainingScope};
use crate::redaction::{Finding, FindingKind};
use crate::review::{ReviewRequest, ReviewState, TurnReview};
use crate::store::Backend;
use crate::turn::{
    PartSummary, Provenance, RecordTurnRequest, RecordedTurn, Role, TurnContent, TurnState,
};
use crate::{Error, INDEX_SHARD_ENTRIES, MAX_TURN_PAGE, Result, validate_digest, validate_name};

/// A turn as the archive holds it: everything except the words.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ArchivedTurn {
    pub turn_id: String,
    pub conversation_id: String,
    pub message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_message_id: Option<String>,
    pub ordinal: u32,
    pub role: Role,
    /// `sha256` of the canonical content. What makes a re-send idempotent, what
    /// a duplicate is detected on, and what an export version is built from.
    pub content_digest: String,
    pub content_bytes: usize,
    /// The shape of the message, with none of it.
    #[serde(default)]
    pub parts: Vec<PartSummary>,
    #[serde(default)]
    pub tool_results: usize,
    #[serde(default)]
    pub provenance: Provenance,
    pub policy: ContentPolicy,
    /// True when this deployment shortened the retention the producer asked
    /// for. Recorded rather than silent: a corpus built under a policy nobody
    /// noticed had changed is the thing an audit is looking for.
    #[serde(default)]
    pub retention_clamped: bool,
    #[serde(default)]
    pub findings: Vec<Finding>,
    #[serde(default)]
    pub review: TurnReview,
    #[serde(default)]
    pub state: TurnState,
    /// Why the content is gone, when it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub erasure: Option<Erasure>,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String)]
    pub received_at: OffsetDateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    #[schema(value_type = Option<String>)]
    pub occurred_at: Option<OffsetDateTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    #[schema(value_type = Option<String>)]
    pub expires_at: Option<OffsetDateTime>,
    /// Which index shard lists this turn, once one does.
    ///
    /// `None` is the window between writing a head and appending its index
    /// entry. A re-send repairs it, which is why this is on the head rather
    /// than being inferred: without it, a retry would find the head present,
    /// skip the append, and leave the turn invisible to every export forever.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_shard: Option<usize>,
}

impl ArchivedTurn {
    /// Whether an export may take this turn's content at all.
    #[must_use]
    pub fn is_readable(&self) -> bool {
        self.state == TurnState::Held
    }

    /// Whether the recorded consent covers `scope`.
    #[must_use]
    pub fn permits(&self, scope: TrainingScope) -> bool {
        self.policy.consent.permits(scope)
    }

    #[must_use]
    pub fn has_finding(&self, kind: FindingKind) -> bool {
        self.findings.iter().any(|finding| finding.kind == kind)
    }
}

/// Why content was removed.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct Erasure {
    pub reason: ErasureReason,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub by: String,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String)]
    pub at: OffsetDateTime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErasureReason {
    /// The retention this turn declared ran out.
    Expired,
    /// Somebody asked, naming the subject or the conversation.
    Request,
}

impl ErasureReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Expired => "expired",
            Self::Request => "request",
        }
    }
}

/// One conversation's counts. The review queue reads these and nothing else.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ConversationHead {
    pub conversation_id: String,
    pub turns: usize,
    pub pending: usize,
    pub approved: usize,
    pub rejected: usize,
    pub erased: usize,
    /// Findings by kind, so "which conversations hold a credential" is a list
    /// rather than a scan.
    #[serde(default)]
    pub findings: BTreeMap<String, usize>,
    #[serde(default)]
    pub shards: usize,
    #[serde(default)]
    pub entries: usize,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String)]
    pub first_seen: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String)]
    pub last_seen: OffsetDateTime,
    /// The soonest any of this conversation's content expires.
    ///
    /// A hint the sweep uses to decide whether to open this conversation at
    /// all, recomputed whenever it does. Being stale costs one wasted read;
    /// being absent would cost a scan of every turn in the archive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    #[schema(value_type = Option<String>)]
    pub earliest_expiry: Option<OffsetDateTime>,
}

impl ConversationHead {
    /// A head for a conversation whose first turn is arriving now.
    ///
    /// Hand-written rather than derived, because `first_seen` and `last_seen`
    /// have no sensible zero: a head defaulted to the Unix epoch would sort to
    /// the bottom of every list and read as a conversation from 1970.
    fn opening(conversation_id: &str, at: OffsetDateTime) -> Self {
        Self {
            conversation_id: conversation_id.to_owned(),
            turns: 0,
            pending: 0,
            approved: 0,
            rejected: 0,
            erased: 0,
            findings: BTreeMap::new(),
            shards: 0,
            entries: 0,
            first_seen: at,
            last_seen: at,
            earliest_expiry: None,
        }
    }
}

/// The order an export reads a conversation in.
///
/// Appended rather than sorted at read time, because arrival order is the only
/// ordering that is stable: a producer's `occurred_at` can go backwards, and
/// `ordinal` is a hint a producer is free to leave at zero.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct IndexShard {
    #[serde(default)]
    pub(crate) entries: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct DigestMarker {
    turn_id: String,
    conversation_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SubjectMarker {
    turn_id: String,
    conversation_id: String,
}

/// What a listing may narrow on.
#[derive(Clone, Debug, Default)]
pub struct TurnFilter {
    pub review: Option<ReviewState>,
    pub finding: Option<FindingKind>,
    pub role: Option<Role>,
}

impl TurnFilter {
    fn matches(&self, turn: &ArchivedTurn) -> bool {
        self.review.is_none_or(|state| turn.review.state == state)
            && self.finding.is_none_or(|kind| turn.has_finding(kind))
            && self.role.is_none_or(|role| turn.role == role)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct TurnPage {
    pub turns: Vec<ArchivedTurn>,
    /// Where to resume. Absent at the end of the conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
    pub total: usize,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
#[schema(as = ConversationArchivePage)]
pub struct ConversationPage {
    pub conversations: Vec<ConversationHead>,
}

/// What a sweep or an erasure removed.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct ErasureReport {
    pub turns_erased: usize,
    pub conversations_touched: usize,
    /// Turns whose retention had run out but whose content was already gone.
    /// Counted so a repeated sweep is visibly a no-op rather than silently one.
    pub already_erased: usize,
    /// Published corpora whose rows this erasure took away.
    ///
    /// The half of an erasure that is easy to forget: erasing the archive and
    /// leaving the corpus is an erasure in name only. Their manifests survive,
    /// so a training run naming one still resolves to something that can say
    /// what happened to it.
    pub corpora_withdrawn: usize,
}

// ── Operations ───────────────────────────────────────────────────────────────
//
// `pub(crate)`: the public door is `Registry`. Every function here takes an
// already-resolved backend and does one thing to it.

pub(crate) async fn record(
    backend: &Backend,
    policy: ArchivePolicy,
    request: RecordTurnRequest,
) -> Result<RecordedTurn> {
    request.validate()?;

    let problems = policy.check(&request.policy);
    if !problems.is_empty() {
        return Err(Error::Rejected(problems));
    }

    let mut content_policy = request.policy.clone();
    let (retention, clamped) = policy.clamp(&content_policy.retention);
    content_policy.retention = retention;

    let mut findings = crate::redaction::scan(&request.content);
    if policy.reject_on_finding && !findings.is_empty() {
        return Err(Error::Rejected(
            findings
                .iter()
                .map(|finding| {
                    format!(
                        "the content matched {} ({}), and this deployment refuses such a write",
                        finding.rule,
                        finding.kind.as_str()
                    )
                })
                .collect(),
        ));
    }

    let turn_id = request.turn_id();
    let content_digest = request.content.digest();
    let existing: Option<ArchivedTurn> = backend
        .read(&backend.turn_key(&request.conversation_id, &turn_id))
        .await?;

    // The same words under a different turn id: one observation, two rows. The
    // export keeps the first and names the rest, rather than training twice on
    // one exchange.
    if let Some(marker) = backend
        .read::<DigestMarker>(&backend.digest_key(&content_digest))
        .await?
        && marker.turn_id != turn_id
    {
        findings.push(Finding::about_turn(
            FindingKind::Duplicate,
            "same-content",
            "scanner",
        ));
    }

    let now = OffsetDateTime::now_utc();
    let received_at = existing.as_ref().map_or(now, |turn| turn.received_at);
    let unchanged = existing
        .as_ref()
        .is_some_and(|turn| turn.content_digest == content_digest && turn.is_readable());

    // A re-send of identical content keeps its review; a re-send of *different*
    // content under the same message id resets it. Carrying an approval across
    // an edit is how reviewed text becomes unreviewed text with a tick beside
    // it.
    let review = match &existing {
        Some(turn) if unchanged => turn.review.clone(),
        _ => TurnReview::default(),
    };
    // A human's findings survive a re-scan; the scanner's are replaced by this
    // one. Otherwise a reviewer's "unsafe" disappears the next time the
    // producer flushes.
    if let Some(turn) = &existing {
        findings.extend(
            turn.findings
                .iter()
                .filter(|finding| finding.found_by != "scanner")
                .cloned(),
        );
    }

    let turn = ArchivedTurn {
        turn_id: turn_id.clone(),
        conversation_id: request.conversation_id.clone(),
        message_id: request.message_id.clone(),
        parent_message_id: request.parent_message_id.clone(),
        ordinal: request.ordinal,
        role: request.role,
        content_digest: content_digest.clone(),
        content_bytes: request.content.bytes(),
        parts: request.content.summarise(),
        tool_results: request.content.tool_results.len(),
        provenance: request.provenance.clone(),
        retention_clamped: clamped,
        expires_at: Some(content_policy.retention.expires_at(received_at)),
        policy: content_policy,
        findings: findings.clone(),
        review: review.clone(),
        state: TurnState::Held,
        erasure: None,
        received_at,
        occurred_at: request.occurred_at,
        index_shard: existing.as_ref().and_then(|turn| turn.index_shard),
    };

    // Content before the head that names it.
    backend
        .seal(&backend.content_key(&turn_id), &request.content)
        .await?;
    let mut turn = turn;
    write_turn(backend, &mut turn, existing.as_ref()).await?;
    backend
        .write(
            &backend.digest_key(&content_digest),
            &DigestMarker {
                turn_id: turn_id.clone(),
                conversation_id: request.conversation_id.clone(),
            },
        )
        .await?;

    let expires_at = turn.expires_at;
    Ok(RecordedTurn {
        turn_id,
        content_digest,
        created: existing.is_none(),
        findings,
        review,
        expires_at,
    })
}

/// Write a head, then make sure the index and the conversation's counts agree
/// with it.
async fn write_turn(
    backend: &Backend,
    turn: &mut ArchivedTurn,
    previous: Option<&ArchivedTurn>,
) -> Result<()> {
    let key = backend.turn_key(&turn.conversation_id, &turn.turn_id);
    backend.write(&key, &*turn).await?;

    if let Some(subject) = non_empty(&turn.policy.consent.subject) {
        backend
            .write(
                &backend.subject_key(subject, &turn.turn_id),
                &SubjectMarker {
                    turn_id: turn.turn_id.clone(),
                    conversation_id: turn.conversation_id.clone(),
                },
            )
            .await?;
    }

    let mut head = conversation_head(backend, &turn.conversation_id)
        .await?
        .unwrap_or_else(|| ConversationHead::opening(&turn.conversation_id, turn.received_at));

    // The head before the index entry — and the index entry recorded on the
    // head, so a crash between the two is repaired by the next write of this
    // turn rather than leaving it invisible.
    if turn.index_shard.is_none() {
        let last = head.shards.saturating_sub(1);
        let mut current: IndexShard = backend
            .read(&backend.index_key(&turn.conversation_id, last))
            .await?
            .unwrap_or_default();
        // A full shard is never rewritten, which is what keeps appending a
        // turn a bounded write however long a conversation gets.
        let shard = if current.entries.len() >= INDEX_SHARD_ENTRIES {
            current = IndexShard::default();
            head.shards
        } else {
            last
        };
        current.entries.push(turn.turn_id.clone());
        backend
            .write(&backend.index_key(&turn.conversation_id, shard), &current)
            .await?;
        head.shards = head.shards.max(shard + 1);
        head.entries += 1;
        turn.index_shard = Some(shard);
        backend.write(&key, &*turn).await?;
    }

    apply_counts(&mut head, previous, Some(turn));
    head.last_seen = head.last_seen.max(turn.received_at);
    head.earliest_expiry = match (head.earliest_expiry, turn.expires_at) {
        (Some(current), Some(candidate)) => Some(current.min(candidate)),
        (current, candidate) => current.or(candidate),
    };
    backend
        .write(&backend.conversation_key(&turn.conversation_id), &head)
        .await
}

/// Move a conversation's counts from what a turn used to be to what it is now.
fn apply_counts(
    head: &mut ConversationHead,
    previous: Option<&ArchivedTurn>,
    next: Option<&ArchivedTurn>,
) {
    if let Some(previous) = previous {
        head.turns = head.turns.saturating_sub(1);
        decrement(head, previous);
    }
    if let Some(next) = next {
        head.turns += 1;
        increment(head, next);
    }
}

fn increment(head: &mut ConversationHead, turn: &ArchivedTurn) {
    match turn.review.state {
        ReviewState::Pending => head.pending += 1,
        ReviewState::Approved => head.approved += 1,
        ReviewState::Rejected => head.rejected += 1,
    }
    if turn.state == TurnState::Erased {
        head.erased += 1;
    }
    for finding in &turn.findings {
        *head
            .findings
            .entry(finding.kind.as_str().to_owned())
            .or_default() += 1;
    }
}

fn decrement(head: &mut ConversationHead, turn: &ArchivedTurn) {
    let counter = match turn.review.state {
        ReviewState::Pending => &mut head.pending,
        ReviewState::Approved => &mut head.approved,
        ReviewState::Rejected => &mut head.rejected,
    };
    *counter = counter.saturating_sub(1);
    if turn.state == TurnState::Erased {
        head.erased = head.erased.saturating_sub(1);
    }
    for finding in &turn.findings {
        if let Some(count) = head.findings.get_mut(finding.kind.as_str()) {
            *count = count.saturating_sub(1);
        }
    }
    head.findings.retain(|_, count| *count > 0);
}

fn non_empty(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

pub(crate) async fn conversation_head(
    backend: &Backend,
    conversation_id: &str,
) -> Result<Option<ConversationHead>> {
    backend
        .read(&backend.conversation_key(conversation_id))
        .await
}

pub(crate) async fn conversations(backend: &Backend) -> Result<ConversationPage> {
    let entries = backend.list(&backend.conversations_prefix()).await?;
    let mut conversations = Vec::new();
    for entry in entries {
        if !entry.key.ends_with("/head.json") {
            continue;
        }
        if let Some(head) = backend.read::<ConversationHead>(&entry.key).await? {
            conversations.push(head);
        }
    }
    // Newest activity first, and ties broken by id so a page is stable.
    conversations.sort_by(|a, b| {
        b.last_seen
            .cmp(&a.last_seen)
            .then_with(|| a.conversation_id.cmp(&b.conversation_id))
    });
    Ok(ConversationPage { conversations })
}

/// One conversation's turns, in the order an export reads them.
pub(crate) async fn turns(
    backend: &Backend,
    conversation_id: &str,
    filter: &TurnFilter,
    offset: usize,
    limit: usize,
) -> Result<TurnPage> {
    validate_name(conversation_id, "conversation_id")?;
    let Some(head) = conversation_head(backend, conversation_id).await? else {
        return Err(Error::NotFound(format!(
            "the conversation {conversation_id}"
        )));
    };
    let limit = limit.clamp(1, MAX_TURN_PAGE);

    let mut turns = Vec::new();
    let mut seen = 0;
    let mut cursor = offset;
    let mut position = 0;
    for shard in 0..head.shards {
        let entries: IndexShard = backend
            .read(&backend.index_key(conversation_id, shard))
            .await?
            .unwrap_or_default();
        for turn_id in entries.entries {
            let Some(turn) = backend
                .read::<ArchivedTurn>(&backend.turn_key(conversation_id, &turn_id))
                .await?
            else {
                continue;
            };
            if !filter.matches(&turn) {
                continue;
            }
            seen += 1;
            if position >= cursor && turns.len() < limit {
                turns.push(turn);
            }
            position += 1;
        }
    }
    cursor = offset + turns.len();
    Ok(TurnPage {
        next_offset: (cursor < seen).then_some(cursor),
        turns,
        total: seen,
    })
}

pub(crate) async fn turn(
    backend: &Backend,
    conversation_id: &str,
    turn_id: &str,
) -> Result<ArchivedTurn> {
    validate_name(conversation_id, "conversation_id")?;
    validate_digest(turn_id, "turn_id")?;
    backend
        .read(&backend.turn_key(conversation_id, turn_id))
        .await?
        .ok_or_else(|| Error::NotFound(format!("the turn {turn_id}")))
}

/// The words. The one read in this crate that decrypts anything.
pub(crate) async fn content(
    backend: &Backend,
    conversation_id: &str,
    turn_id: &str,
) -> Result<TurnContent> {
    let head = turn(backend, conversation_id, turn_id).await?;
    if let Some(erasure) = &head.erasure {
        return Err(Error::Erased(
            format!("the turn {turn_id}"),
            erasure
                .at
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| "an unknown date".to_owned()),
        ));
    }
    backend
        .open(&backend.content_key(turn_id))
        .await?
        .ok_or_else(|| Error::NotFound(format!("the content of turn {turn_id}")))
}

pub(crate) async fn review(
    backend: &Backend,
    conversation_id: &str,
    turn_id: &str,
    reviewer: &str,
    request: &ReviewRequest,
) -> Result<ArchivedTurn> {
    request.validate()?;
    let previous = turn(backend, conversation_id, turn_id).await?;
    let mut next = previous.clone();
    next.review = TurnReview {
        state: request.state,
        reviewer: reviewer.to_owned(),
        note: request.note.clone(),
        decided_at: Some(OffsetDateTime::now_utc()),
        preference: request.preference,
    };
    next.findings.extend(request.attributed(reviewer));

    backend
        .write(&backend.turn_key(conversation_id, turn_id), &next)
        .await?;
    if let Some(mut head) = conversation_head(backend, conversation_id).await? {
        apply_counts(&mut head, Some(&previous), Some(&next));
        backend
            .write(&backend.conversation_key(conversation_id), &head)
            .await?;
    }
    Ok(next)
}

/// Remove one turn's content, keeping its head.
async fn erase_turn(
    backend: &Backend,
    conversation_id: &str,
    turn_id: &str,
    reason: ErasureReason,
    by: &str,
) -> Result<bool> {
    let Some(previous) = backend
        .read::<ArchivedTurn>(&backend.turn_key(conversation_id, turn_id))
        .await?
    else {
        return Ok(false);
    };
    if previous.state == TurnState::Erased {
        return Ok(false);
    }

    // The content first: a head that says `erased` while the ciphertext is
    // still in the bucket is the one ordering that would be a lie.
    backend.delete(&backend.content_key(turn_id)).await?;
    backend
        .delete(&backend.digest_key(&previous.content_digest))
        .await?;

    let mut next = previous.clone();
    next.state = TurnState::Erased;
    next.erasure = Some(Erasure {
        reason,
        by: by.to_owned(),
        at: OffsetDateTime::now_utc(),
    });
    backend
        .write(&backend.turn_key(conversation_id, turn_id), &next)
        .await?;
    if let Some(mut head) = conversation_head(backend, conversation_id).await? {
        apply_counts(&mut head, Some(&previous), Some(&next));
        backend
            .write(&backend.conversation_key(conversation_id), &head)
            .await?;
    }
    Ok(true)
}

/// Erase everything in one conversation.
pub(crate) async fn erase_conversation(
    backend: &Backend,
    conversation_id: &str,
    by: &str,
) -> Result<ErasureReport> {
    validate_name(conversation_id, "conversation_id")?;
    let Some(head) = conversation_head(backend, conversation_id).await? else {
        return Err(Error::NotFound(format!(
            "the conversation {conversation_id}"
        )));
    };
    let mut report = ErasureReport {
        conversations_touched: 1,
        ..ErasureReport::default()
    };
    for shard in 0..head.shards {
        let entries: IndexShard = backend
            .read(&backend.index_key(conversation_id, shard))
            .await?
            .unwrap_or_default();
        for turn_id in entries.entries {
            if erase_turn(
                backend,
                conversation_id,
                &turn_id,
                ErasureReason::Request,
                by,
            )
            .await?
            {
                report.turns_erased += 1;
            } else {
                report.already_erased += 1;
            }
        }
    }
    refresh_expiry(backend, conversation_id).await?;
    report.corpora_withdrawn = crate::export::withdraw_for(
        backend,
        &std::collections::BTreeSet::from([conversation_id.to_owned()]),
        by,
    )
    .await?;
    Ok(report)
}

/// Erase everything recorded about one consent subject, wherever it sits.
///
/// The reason the subject markers exist. An erasure request names a person, not
/// a conversation, and answering it by scanning every turn in the archive is
/// the kind of operation that is fine in a demo and impossible at the size
/// where somebody actually asks.
pub(crate) async fn erase_subject(
    backend: &Backend,
    subject: &str,
    by: &str,
) -> Result<ErasureReport> {
    validate_name(subject, "subject")?;
    let entries = backend.list(&backend.subject_prefix(subject)).await?;
    let mut report = ErasureReport::default();
    let mut touched = std::collections::BTreeSet::new();
    for entry in entries {
        let Some(marker) = backend.read::<SubjectMarker>(&entry.key).await? else {
            continue;
        };
        if erase_turn(
            backend,
            &marker.conversation_id,
            &marker.turn_id,
            ErasureReason::Request,
            by,
        )
        .await?
        {
            report.turns_erased += 1;
        } else {
            report.already_erased += 1;
        }
        touched.insert(marker.conversation_id);
    }
    for conversation_id in &touched {
        refresh_expiry(backend, conversation_id).await?;
    }
    report.conversations_touched = touched.len();
    report.corpora_withdrawn = crate::export::withdraw_for(backend, &touched, by).await?;
    Ok(report)
}

/// Remove the content of everything whose declared retention has run out.
///
/// Only conversations whose `earliest_expiry` has passed are opened, which is
/// what keeps a sweep from being a scan of the archive.
pub(crate) async fn sweep(backend: &Backend, now: OffsetDateTime) -> Result<ErasureReport> {
    let mut report = ErasureReport::default();
    let mut expired = std::collections::BTreeSet::new();
    for head in conversations(backend).await?.conversations {
        if head.earliest_expiry.is_none_or(|expiry| expiry > now) {
            continue;
        }
        let mut touched = false;
        for shard in 0..head.shards {
            let entries: IndexShard = backend
                .read(&backend.index_key(&head.conversation_id, shard))
                .await?
                .unwrap_or_default();
            for turn_id in entries.entries {
                let Some(turn) = backend
                    .read::<ArchivedTurn>(&backend.turn_key(&head.conversation_id, &turn_id))
                    .await?
                else {
                    continue;
                };
                if turn.expires_at.is_none_or(|expiry| expiry > now) {
                    continue;
                }
                if erase_turn(
                    backend,
                    &head.conversation_id,
                    &turn_id,
                    ErasureReason::Expired,
                    "retention",
                )
                .await?
                {
                    report.turns_erased += 1;
                    touched = true;
                } else {
                    report.already_erased += 1;
                }
            }
        }
        refresh_expiry(backend, &head.conversation_id).await?;
        if touched {
            report.conversations_touched += 1;
            expired.insert(head.conversation_id.clone());
        }
    }
    // A corpus holding content whose retention ran out is the same problem as
    // one holding content somebody asked to have deleted, and it arrives more
    // quietly: nobody filed a request, the clock simply passed.
    report.corpora_withdrawn = crate::export::withdraw_for(backend, &expired, "retention").await?;
    Ok(report)
}

/// Recompute a conversation's expiry hint from the turns it still holds.
async fn refresh_expiry(backend: &Backend, conversation_id: &str) -> Result<()> {
    let Some(mut head) = conversation_head(backend, conversation_id).await? else {
        return Ok(());
    };
    let mut earliest = None;
    for shard in 0..head.shards {
        let entries: IndexShard = backend
            .read(&backend.index_key(conversation_id, shard))
            .await?
            .unwrap_or_default();
        for turn_id in entries.entries {
            let Some(turn) = backend
                .read::<ArchivedTurn>(&backend.turn_key(conversation_id, &turn_id))
                .await?
            else {
                continue;
            };
            if turn.state == TurnState::Erased {
                continue;
            }
            earliest = match (earliest, turn.expires_at) {
                (Some(current), Some(candidate)) => Some(std::cmp::min(current, candidate)),
                (current, candidate) => current.or(candidate),
            };
        }
    }
    head.earliest_expiry = earliest;
    backend
        .write(&backend.conversation_key(conversation_id), &head)
        .await
}
