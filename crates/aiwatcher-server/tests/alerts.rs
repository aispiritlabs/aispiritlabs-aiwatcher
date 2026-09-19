#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! What the alert half does end to end: what a watcher raises, what the queue
//! does when the receiver is not there, and what a first pass must not do.

use std::sync::{Arc, Mutex};

use aiwatcher_alerts::{
    AlertChannel, AlertPayload, AlertRule, AlertTrigger, ChannelDescription, Delivery,
    DeliveryFilter, PublishRule, Registry, RegistryConfig, RuleName,
};
use aiwatcher_core::ports::{PortError, PortResult};
use aiwatcher_core::{EventEnvelope, EventType, RecordedEvent, Sdk, Source};
use aiwatcher_jobs::JobState;
use aiwatcher_projector::readmodel::{ReadModel, ReadModelConfig};
use aiwatcher_prompts::adapters::memory::MemoryObjectStore;
use aiwatcher_server::alerts::watch;
use time::OffsetDateTime;
use time::macros::datetime;

// ── Doubles ──────────────────────────────────────────────────────────────────

/// A receiver that answers however the test says, and remembers what it saw.
#[derive(Debug)]
struct Receiver {
    answers: Mutex<Vec<PortResult<()>>>,
    taken: Mutex<Vec<AlertPayload>>,
}

impl Receiver {
    fn answering(answers: Vec<PortResult<()>>) -> Arc<Self> {
        Arc::new(Self {
            answers: Mutex::new(answers),
            taken: Mutex::new(Vec::new()),
        })
    }

    fn always_takes() -> Arc<Self> {
        Self::answering(Vec::new())
    }

    fn attempts(&self) -> usize {
        self.taken.lock().expect("the log").len()
    }
}

#[async_trait::async_trait]
impl AlertChannel for Receiver {
    async fn deliver(&self, payload: &AlertPayload) -> PortResult<()> {
        self.taken.lock().expect("the log").push(payload.clone());
        let mut answers = self.answers.lock().expect("the answers");
        if answers.is_empty() {
            return Ok(());
        }
        answers.remove(0)
    }

    fn describe(&self) -> ChannelDescription {
        ChannelDescription {
            kind: "webhook",
            variable: "AIWATCHER_ALERT_WEBHOOK_URL",
            signed: false,
        }
    }
}

fn unavailable() -> PortResult<()> {
    Err(PortError::Unavailable {
        target: "alert-channel",
        message: "the alert channel answered 503 Service Unavailable".to_owned(),
    })
}

fn refused() -> PortResult<()> {
    Err(PortError::Rejected {
        target: "alert-channel",
        message: "the alert channel answered 400 Bad Request".to_owned(),
    })
}

// ── Fixtures ─────────────────────────────────────────────────────────────────

fn alerts() -> Arc<Registry> {
    Arc::new(Registry::new(
        Arc::new(MemoryObjectStore::new()),
        RegistryConfig::default(),
    ))
}

async fn publish(alerts: &Registry, trigger: AlertTrigger) {
    alerts
        .publish(
            PublishRule {
                rule: AlertRule {
                    name: RuleName::parse("failures").expect("a name"),
                    description: "a managed run that dies must not die quietly".to_owned(),
                    trigger,
                },
                notes: None,
            },
            Some("mk".to_owned()),
            10,
        )
        .await
        .expect("a publish");
}

const AT: OffsetDateTime = datetime!(2026-09-19 09:00:00 UTC);

fn event(
    event_type: EventType,
    execution: &str,
    workflow: &str,
    position: u64,
    at: OffsetDateTime,
    data: serde_json::Value,
) -> RecordedEvent {
    let mut envelope =
        EventEnvelope::new(event_type, execution, at, Source::new("tests", Sdk::Rust))
            .with_data(data);
    envelope.event_id = Some(aiwatcher_core::MessageId::new(format!(
        "{execution}-{position}"
    )));
    envelope.workflow_id = Some(workflow.to_owned());
    envelope.workflow_run_id = Some(execution.to_owned());
    envelope.record(position, position, at, None)
}

/// A read model holding one managed execution that started and then failed.
async fn read_model_with_a_failure(execution: &str, workflow: &str) -> Arc<ReadModel> {
    let model = Arc::new(ReadModel::new(ReadModelConfig::default()));
    model
        .apply(&event(
            EventType::ExecutionStarted,
            execution,
            workflow,
            1,
            AT,
            serde_json::json!({}),
        ))
        .await;
    model
        .apply(&event(
            EventType::ExecutionFailed,
            execution,
            workflow,
            2,
            AT + time::Duration::seconds(30),
            serde_json::json!({ "reason": "the retry budget ran out" }),
        ))
        .await;
    model
}

/// Everything the drain does, without the loop that paces it.
async fn drain(alerts: &Registry, channel: &dyn AlertChannel, now: i64) {
    for mut delivery in alerts.due(32, now).await.expect("the queue") {
        match channel.deliver(&delivery.payload).await {
            Ok(()) => delivery.sent(now),
            Err(error) => {
                let retryable = error.is_retryable();
                delivery.attempt_failed(&error.to_string(), retryable, now);
            }
        }
        alerts.record(&delivery).await.expect("a record");
    }
}

async fn history(alerts: &Registry) -> Vec<Delivery> {
    alerts
        .deliveries(&DeliveryFilter::default())
        .await
        .expect("the history")
        .deliveries
}

// ── The watcher ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_first_pass_reports_nothing_that_happened_before_alerts_were_turned_on() {
    let alerts = alerts();
    publish(&alerts, AlertTrigger::ExecutionFailed { definition: None }).await;
    let model = read_model_with_a_failure("exec-1", "house-import").await;
    let rules = alerts.active().await.expect("the rules");

    let first = watch::execution_failures(&alerts, &model, &rules, AT.unix_timestamp() + 3_600)
        .await
        .expect("a pass");
    assert!(first.started, "the first pass writes a cursor and stops");
    assert_eq!(first.raised, 0);
    assert!(history(&alerts).await.is_empty());
}

#[tokio::test]
async fn a_failure_after_the_cursor_is_raised_once_and_not_again() {
    let alerts = alerts();
    publish(&alerts, AlertTrigger::ExecutionFailed { definition: None }).await;
    let model = Arc::new(ReadModel::new(ReadModelConfig::default()));
    let rules = alerts.active().await.expect("the rules");

    // A first pass over an empty fold sets the cursor where time is now.
    let started = AT.unix_timestamp() - 10;
    watch::execution_failures(&alerts, &model, &rules, started)
        .await
        .expect("the first pass");

    for (position, event_type, at, data) in [
        (1u64, EventType::ExecutionStarted, AT, serde_json::json!({})),
        (
            2,
            EventType::ExecutionFailed,
            AT + time::Duration::seconds(30),
            serde_json::json!({ "reason": "the retry budget ran out" }),
        ),
    ] {
        model
            .apply(&event(
                event_type,
                "exec-1",
                "house-import",
                position,
                at,
                data,
            ))
            .await;
    }

    let now = AT.unix_timestamp() + 60;
    let pass = watch::execution_failures(&alerts, &model, &rules, now)
        .await
        .expect("a pass");
    assert_eq!(pass.raised, 1);

    let rows = history(&alerts).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].payload.subject, "exec-1");
    assert_eq!(rows[0].payload.title, "house-import failed");
    assert!(
        rows[0]
            .payload
            .facts
            .iter()
            .any(|fact| fact.value.contains("the retry budget ran out")),
        "the notification says why: {:?}",
        rows[0].payload.facts
    );

    let again = watch::execution_failures(&alerts, &model, &rules, now + 60)
        .await
        .expect("a second pass");
    assert_eq!(again.raised, 0, "the same failure is not a second alert");
    assert_eq!(history(&alerts).await.len(), 1);
}

#[tokio::test]
async fn a_rule_narrowed_to_another_definition_hears_nothing() {
    let alerts = alerts();
    publish(
        &alerts,
        AlertTrigger::ExecutionFailed {
            definition: Some("scoring".to_owned()),
        },
    )
    .await;
    let model = Arc::new(ReadModel::new(ReadModelConfig::default()));
    let rules = alerts.active().await.expect("the rules");
    watch::execution_failures(&alerts, &model, &rules, AT.unix_timestamp() - 10)
        .await
        .expect("the first pass");

    for (position, event_type, at) in [
        (1u64, EventType::ExecutionStarted, AT),
        (
            2,
            EventType::ExecutionFailed,
            AT + time::Duration::seconds(30),
        ),
    ] {
        model
            .apply(&event(
                event_type,
                "exec-1",
                "house-import",
                position,
                at,
                serde_json::json!({}),
            ))
            .await;
    }

    let pass = watch::execution_failures(&alerts, &model, &rules, AT.unix_timestamp() + 60)
        .await
        .expect("a pass");
    assert_eq!(pass.seen, 1, "it was read");
    assert_eq!(pass.raised, 0, "and it was not this rule's");
}

// ── The queue ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_channel_outage_is_retried_and_the_notification_still_arrives() {
    let alerts = alerts();
    publish(&alerts, AlertTrigger::ExecutionFailed { definition: None }).await;
    let model = read_model_with_a_failure("exec-1", "house-import").await;
    let rules = alerts.active().await.expect("the rules");
    watch::execution_failures(&alerts, &model, &rules, AT.unix_timestamp() - 10)
        .await
        .expect("the first pass");
    let now = AT.unix_timestamp() + 60;
    watch::execution_failures(&alerts, &model, &rules, now)
        .await
        .expect("a pass");

    let receiver = Receiver::answering(vec![unavailable()]);
    drain(&alerts, receiver.as_ref(), now).await;
    let queued = &history(&alerts).await[0];
    assert_eq!(queued.state, JobState::Queued, "it waits rather than dying");
    assert!(queued.last_error.is_some(), "and the failure is visible");
    assert!(
        alerts.due(10, now).await.expect("the queue").is_empty(),
        "it is not tried again before its backoff"
    );

    let later = now + aiwatcher_alerts::RETRY_BACKOFF_SECONDS[0];
    drain(&alerts, receiver.as_ref(), later).await;
    let delivered = &history(&alerts).await[0];
    assert_eq!(delivered.state, JobState::Completed);
    assert_eq!(delivered.attempts, 2);
    assert_eq!(receiver.attempts(), 2);
}

#[tokio::test]
async fn a_receiver_that_refuses_is_not_retried_and_stays_in_the_history() {
    let alerts = alerts();
    publish(&alerts, AlertTrigger::ExecutionFailed { definition: None }).await;
    let model = read_model_with_a_failure("exec-1", "house-import").await;
    let rules = alerts.active().await.expect("the rules");
    watch::execution_failures(&alerts, &model, &rules, AT.unix_timestamp() - 10)
        .await
        .expect("the first pass");
    let now = AT.unix_timestamp() + 60;
    watch::execution_failures(&alerts, &model, &rules, now)
        .await
        .expect("a pass");

    let receiver = Receiver::answering(vec![refused()]);
    drain(&alerts, receiver.as_ref(), now).await;

    let row = &history(&alerts).await[0];
    assert_eq!(row.state, JobState::Failed);
    assert_eq!(row.attempts, 1, "a refusal spends one attempt, not three");
    assert!(
        row.last_error
            .as_deref()
            .is_some_and(|error| error.contains("400")),
        "the history says what the receiver said"
    );

    // And an operator can still send it by hand once the receiver is fixed.
    alerts
        .retry(&row.dedup_key, now + 3_600)
        .await
        .expect("a retry");
    drain(&alerts, Receiver::always_takes().as_ref(), now + 3_600).await;
    assert_eq!(history(&alerts).await[0].state, JobState::Completed);
}

#[tokio::test]
async fn every_notification_carries_the_key_the_receiver_deduplicates_by() {
    let alerts = alerts();
    publish(&alerts, AlertTrigger::ExecutionFailed { definition: None }).await;
    let model = read_model_with_a_failure("exec-1", "house-import").await;
    let rules = alerts.active().await.expect("the rules");
    watch::execution_failures(&alerts, &model, &rules, AT.unix_timestamp() - 10)
        .await
        .expect("the first pass");
    let now = AT.unix_timestamp() + 60;
    watch::execution_failures(&alerts, &model, &rules, now)
        .await
        .expect("a pass");

    let receiver = Receiver::answering(vec![unavailable()]);
    drain(&alerts, receiver.as_ref(), now).await;
    drain(
        &alerts,
        receiver.as_ref(),
        now + aiwatcher_alerts::RETRY_BACKOFF_SECONDS[0],
    )
    .await;

    let taken = receiver.taken.lock().expect("the log");
    assert_eq!(taken.len(), 2, "the receiver saw it twice");
    assert_eq!(
        taken[0].dedup_key, taken[1].dedup_key,
        "under one key, which is what lets it ignore the second"
    );
}
