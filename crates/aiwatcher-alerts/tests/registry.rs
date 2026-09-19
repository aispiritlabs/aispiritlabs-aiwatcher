#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! The registry over a real object store: what publishing, raising and
//! draining do to the bytes.

use std::sync::Arc;

use aiwatcher_alerts::{
    AlertFact, AlertLink, AlertRule, AlertSignal, AlertTrigger, DeliveryFilter, PublishRule,
    Registry, RegistryConfig, RuleFilter, RuleName, TriggerKind,
};
use aiwatcher_jobs::JobState;
use aiwatcher_prompts::adapters::memory::MemoryObjectStore;

fn registry() -> Registry {
    Registry::new(
        Arc::new(MemoryObjectStore::new()),
        RegistryConfig::default(),
    )
}

fn rule(name: &str, trigger: AlertTrigger) -> PublishRule {
    PublishRule {
        rule: AlertRule {
            name: RuleName::parse(name).expect("a name"),
            description: "somebody wants to hear about this".to_owned(),
            trigger,
        },
        notes: None,
    }
}

fn signal(subject: &str, within: Option<&str>) -> AlertSignal {
    AlertSignal {
        trigger: TriggerKind::ExecutionFailed,
        subject: subject.to_owned(),
        within: within.map(ToOwned::to_owned),
        occurred_at: 1_000,
        title: "the nightly import failed".to_owned(),
        facts: vec![AlertFact::new("reason", "the step gave up")],
        links: vec![AlertLink::new("execution", "/workflows/exec-1")],
    }
}

#[tokio::test]
async fn republishing_an_unchanged_rule_is_not_a_new_version() {
    let registry = registry();
    let first = registry
        .publish(
            rule(
                "nightly",
                AlertTrigger::ExecutionFailed { definition: None },
            ),
            Some("mk".to_owned()),
            10,
        )
        .await
        .expect("a publish");
    assert!(first.created);

    let again = registry
        .publish(
            rule(
                "nightly",
                AlertTrigger::ExecutionFailed { definition: None },
            ),
            Some("mk".to_owned()),
            20,
        )
        .await
        .expect("a second publish");
    assert!(!again.created);
    assert_eq!(again.version.version_id, first.version.version_id);
    assert_eq!(again.head.versions.len(), 1);
}

#[tokio::test]
async fn the_same_occurrence_twice_is_one_delivery() {
    let registry = registry();
    registry
        .publish(
            rule(
                "nightly",
                AlertTrigger::ExecutionFailed { definition: None },
            ),
            None,
            10,
        )
        .await
        .expect("a publish");
    let rules = registry.active().await.expect("the active rules");

    let first = registry
        .raise(&rules, &signal("exec-1", Some("import")), 20)
        .await
        .expect("a raise");
    assert_eq!(first.len(), 1);
    assert!(first[0].created);

    let again = registry
        .raise(&rules, &signal("exec-1", Some("import")), 30)
        .await
        .expect("a second raise");
    assert!(
        !again[0].created,
        "the same occurrence is not a second alert"
    );
    assert_eq!(again[0].dedup_key, first[0].dedup_key);

    let history = registry
        .deliveries(&DeliveryFilter::default())
        .await
        .expect("the history");
    assert_eq!(history.total, 1);
    assert_eq!(history.deliveries[0].created_at, 20, "the first one stands");
}

#[tokio::test]
async fn editing_a_rule_asks_the_same_question_again() {
    // A tolerance loosened after a regression fired is a different question
    // about the same result, and the new answer is worth having.
    let registry = registry();
    registry
        .publish(
            rule(
                "nightly",
                AlertTrigger::ExecutionFailed { definition: None },
            ),
            None,
            10,
        )
        .await
        .expect("a publish");
    let before = registry.active().await.expect("the rules");
    let first = registry
        .raise(&before, &signal("exec-1", None), 20)
        .await
        .expect("a raise");

    let mut edited = rule(
        "nightly",
        AlertTrigger::ExecutionFailed { definition: None },
    );
    edited.rule.description = "and the reason changed".to_owned();
    registry.publish(edited, None, 30).await.expect("an edit");
    let after = registry.active().await.expect("the rules again");
    let second = registry
        .raise(&after, &signal("exec-1", None), 40)
        .await
        .expect("a raise under the new version");
    assert!(second[0].created);
    assert_ne!(second[0].dedup_key, first[0].dedup_key);
}

#[tokio::test]
async fn a_rule_switched_off_raises_nothing_and_switching_it_back_repeats_nothing() {
    let registry = registry();
    let name = RuleName::parse("nightly").expect("a name");
    registry
        .publish(
            rule(
                "nightly",
                AlertTrigger::ExecutionFailed { definition: None },
            ),
            None,
            10,
        )
        .await
        .expect("a publish");
    let rules = registry.active().await.expect("the rules");
    let first = registry
        .raise(&rules, &signal("exec-1", None), 20)
        .await
        .expect("a raise");

    registry
        .set_enabled(&name, false, 30)
        .await
        .expect("switching off");
    assert!(
        registry.active().await.expect("the rules").is_empty(),
        "a rule that is off matches nothing"
    );

    registry
        .set_enabled(&name, true, 40)
        .await
        .expect("switching on");
    let rules = registry.active().await.expect("the rules again");
    let again = registry
        .raise(&rules, &signal("exec-1", None), 50)
        .await
        .expect("a raise");
    assert!(
        !again[0].created,
        "silencing a rule is not an edit, so the key did not move"
    );
    assert_eq!(again[0].dedup_key, first[0].dedup_key);
}

#[tokio::test]
async fn a_narrowed_rule_only_hears_about_its_own_definition() {
    let registry = registry();
    registry
        .publish(
            rule(
                "imports",
                AlertTrigger::ExecutionFailed {
                    definition: Some("import".to_owned()),
                },
            ),
            None,
            10,
        )
        .await
        .expect("a publish");
    let rules = registry.active().await.expect("the rules");

    assert_eq!(
        registry
            .raise(&rules, &signal("exec-1", Some("scoring")), 20)
            .await
            .expect("a raise")
            .len(),
        0
    );
    assert_eq!(
        registry
            .raise(&rules, &signal("exec-2", Some("import")), 20)
            .await
            .expect("a raise")
            .len(),
        1
    );
}

#[tokio::test]
async fn a_queued_delivery_is_due_and_a_delivered_one_is_not() {
    let registry = registry();
    registry
        .publish(
            rule(
                "nightly",
                AlertTrigger::ExecutionFailed { definition: None },
            ),
            None,
            10,
        )
        .await
        .expect("a publish");
    let rules = registry.active().await.expect("the rules");
    registry
        .raise(&rules, &signal("exec-1", None), 20)
        .await
        .expect("a raise");

    let mut due = registry.due(10, 20).await.expect("the queue");
    assert_eq!(due.len(), 1);

    due[0].sent(25);
    registry.record(&due[0]).await.expect("a record");
    assert!(
        registry.due(10, 30).await.expect("the queue").is_empty(),
        "a delivered notification is not sent again"
    );
    let stored = registry
        .delivery(&due[0].dedup_key)
        .await
        .expect("a read")
        .expect("the record");
    assert_eq!(stored.state, JobState::Completed);
    assert_eq!(stored.delivered_at, Some(25));
}

#[tokio::test]
async fn a_failed_delivery_is_visible_and_can_be_asked_for_again() {
    let registry = registry();
    registry
        .publish(
            rule(
                "nightly",
                AlertTrigger::ExecutionFailed { definition: None },
            ),
            None,
            10,
        )
        .await
        .expect("a publish");
    let rules = registry.active().await.expect("the rules");
    registry
        .raise(&rules, &signal("exec-1", None), 20)
        .await
        .expect("a raise");

    let mut delivery = registry.due(10, 20).await.expect("the queue").remove(0);
    delivery.attempt_failed("410 gone", false, 21);
    registry.record(&delivery).await.expect("a record");

    let failed = registry
        .deliveries(&DeliveryFilter {
            state: Some(JobState::Failed),
            ..DeliveryFilter::default()
        })
        .await
        .expect("the history");
    assert_eq!(failed.deliveries.len(), 1);
    assert_eq!(failed.deliveries[0].last_error.as_deref(), Some("410 gone"));

    let retried = registry
        .retry(&delivery.dedup_key, 100)
        .await
        .expect("a retry");
    assert_eq!(retried.state, JobState::Queued);
    assert_eq!(registry.due(10, 100).await.expect("the queue").len(), 1);
}

#[tokio::test]
async fn a_delivery_that_did_not_fail_is_not_sent_again_by_hand() {
    let registry = registry();
    registry
        .publish(
            rule(
                "nightly",
                AlertTrigger::ExecutionFailed { definition: None },
            ),
            None,
            10,
        )
        .await
        .expect("a publish");
    let rules = registry.active().await.expect("the rules");
    let raised = registry
        .raise(&rules, &signal("exec-1", None), 20)
        .await
        .expect("a raise");

    let error = registry
        .retry(&raised[0].dedup_key, 30)
        .await
        .expect_err("a queued delivery");
    assert!(error.to_string().contains("queued"), "{error}");
}

#[tokio::test]
async fn the_sweep_forgets_finished_history_and_never_a_waiting_notification() {
    let registry = registry();
    registry
        .publish(
            rule(
                "nightly",
                AlertTrigger::ExecutionFailed { definition: None },
            ),
            None,
            10,
        )
        .await
        .expect("a publish");
    let rules = registry.active().await.expect("the rules");
    for subject in ["exec-1", "exec-2"] {
        registry
            .raise(&rules, &signal(subject, None), 20)
            .await
            .expect("a raise");
    }
    let mut delivered = registry.due(10, 20).await.expect("the queue").remove(0);
    delivered.sent(21);
    registry.record(&delivered).await.expect("a record");

    let pruned = registry.prune(100, 10).await.expect("a sweep");
    assert_eq!(pruned, 1);
    let left = registry
        .deliveries(&DeliveryFilter::default())
        .await
        .expect("the history");
    assert_eq!(left.total, 1);
    assert_eq!(left.deliveries[0].state, JobState::Queued);
}

#[tokio::test]
async fn a_rule_page_lists_by_name_and_says_what_each_is_at() {
    let registry = registry();
    for name in ["beta", "alpha"] {
        registry
            .publish(
                rule(name, AlertTrigger::ExecutionFailed { definition: None }),
                None,
                10,
            )
            .await
            .expect("a publish");
    }
    let page = registry
        .list(&RuleFilter::default())
        .await
        .expect("the page");
    assert_eq!(page.total, 2);
    assert_eq!(page.rules[0].name.as_str(), "alpha");
    assert!(page.rules[0].enabled);
    assert_eq!(
        page.rules[0]
            .current
            .as_ref()
            .expect("a current version")
            .trigger,
        TriggerKind::ExecutionFailed
    );
}
