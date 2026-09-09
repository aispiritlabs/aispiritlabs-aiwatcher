//! Getting a committed fact onto the event log, after it committed.
//!
//! ADR_0026's one strict ordering. The decision, its projection and these rows
//! land in one transaction; this drains them afterwards. Never from the
//! handler, and never before — a `step.completed` on the log for an attempt the
//! store does not consider complete is the split-brain the whole design exists
//! to prevent, and it is [`aiwatcher_jobs::ORDERING`] in the fourth place it
//! applies.
//!
//! ## Why publishing twice is fine and losing one is not
//!
//! The publisher crashes between the send and the mark often enough that it is
//! the normal case, not the edge one. So the rows are marked published *after*
//! the sink accepted them, which means a crash re-sends — and the envelopes
//! carry `event_id`s derived from the decision that produced them, so the
//! projector's own dedup lands the re-send on the record its first delivery
//! created. Marking first would lose a fact with nothing able to say which.
//!
//! The other direction is the reason this is a loop rather than a task per row:
//! one batch, one `append`, one mark. A sink that took the batch and then
//! failed leaves every row in it unpublished, and the next pass re-sends the
//! whole batch — which is the same redelivery, at a different size.

use aiwatcher_bus::ports::MessageSink;
use aiwatcher_core::{EventEnvelope, MessageId};
use time::OffsetDateTime;

use crate::error::{Result, StoreError};
use crate::message::OutboxMessage;
use crate::store::WorkflowStore;

/// How many rows one pass drains. Bounded so a backlog is worked through in
/// steady batches rather than in one append nobody can retry.
pub const BATCH: usize = 128;

/// What one pass did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Published {
    pub sent: usize,
    /// Rows the store held that this build cannot decode. Left unpublished and
    /// counted, never dropped: a row written by a newer build is a fact this
    /// process must not silently discard.
    pub undecodable: usize,
}

/// Drain up to [`BATCH`] pending rows onto the log.
///
/// Returns what it did, so a caller can loop while there is more rather than
/// polling on a timer it guessed.
///
/// # Errors
///
/// [`StoreError::Backend`] when the sink refused the batch — the rows stay
/// pending and the next pass re-sends them.
pub async fn publish_pending(
    store: &dyn WorkflowStore,
    sink: &dyn MessageSink,
    at: OffsetDateTime,
) -> Result<Published> {
    let pending = store.pending_outbox(BATCH).await?;
    if pending.is_empty() {
        return Ok(Published::default());
    }

    let mut envelopes = Vec::with_capacity(pending.len());
    let mut ids: Vec<MessageId> = Vec::with_capacity(pending.len());
    let mut undecodable = 0usize;
    for row in pending {
        match decode(&row) {
            Some(envelope) => {
                ids.push(row.message_id);
                envelopes.push(envelope);
            }
            None => undecodable += 1,
        }
    }

    if envelopes.is_empty() {
        return Ok(Published {
            sent: 0,
            undecodable,
        });
    }

    let sent = envelopes.len();
    sink.append(envelopes)
        .await
        .map_err(|error| StoreError::Backend(format!("publishing execution facts: {error}")))?;
    // Only now. A crash above this line re-sends; a crash below it re-sends
    // too, which the projector deduplicates by `event_id`.
    store.mark_published(&ids, at).await?;

    Ok(Published { sent, undecodable })
}

fn decode(row: &OutboxMessage) -> Option<EventEnvelope> {
    serde_json::from_value(row.payload.clone()).ok()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aiwatcher_bus::adapters::memory::InMemoryBus;

    use super::*;
    use crate::message::RunProjection;
    use crate::state::{ExecutionId, ExecutionMode, ExecutionOwner, RunState, StateType};
    use crate::store::memory::MemoryWorkflowStore;
    use crate::store::{AppendRequest, ExpectedVersion};

    fn execution() -> ExecutionId {
        ExecutionId::new("exec-1")
    }

    fn projection() -> RunProjection {
        RunProjection {
            execution_id: execution(),
            plan_id: String::new(),
            definition_name: "import".to_owned(),
            owner: ExecutionOwner::Local,
            mode: ExecutionMode::Compiled,
            state: RunState::of(StateType::Running),
            requested_by: "mk".to_owned(),
            steps: Vec::new(),
            last_message_version: 1,
            created_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn metadata(id: &str) -> crate::message::MessageMetadata {
        crate::message::MessageMetadata {
            schema_version: crate::message::SCHEMA_VERSION,
            message_id: MessageId::new(id),
            occurred_at: OffsetDateTime::UNIX_EPOCH,
            correlation_id: aiwatcher_core::CorrelationId::new("exec-1"),
            causation_id: aiwatcher_core::CausationId::new(id),
            trace_id: None,
            span_id: None,
            step_id: None,
            attempt: None,
        }
    }

    fn row(id: &str, event_type: &str) -> OutboxMessage {
        let mut envelope = EventEnvelope::new(
            aiwatcher_core::EventType::parse(event_type),
            "exec-1",
            OffsetDateTime::UNIX_EPOCH,
            aiwatcher_core::Source::new(crate::facts::PUBLISHER_SERVICE, aiwatcher_core::Sdk::Rust),
        );
        envelope.event_id = Some(MessageId::new(id));
        OutboxMessage {
            message_id: MessageId::new(id),
            event_type: event_type.to_owned(),
            partition_key: "workflow:exec-1".to_owned(),
            payload: serde_json::to_value(envelope).expect("an envelope"),
            available_at: OffsetDateTime::UNIX_EPOCH,
            attempts: 0,
            published_at: None,
            last_error: None,
        }
    }

    async fn store_with(rows: Vec<OutboxMessage>) -> MemoryWorkflowStore {
        let store = MemoryWorkflowStore::new();
        store
            .append(
                &execution(),
                AppendRequest {
                    expected_version: ExpectedVersion::NoStream,
                    input: crate::message::PendingMessage::input(
                        crate::WorkflowMessage::Command(crate::WorkflowCommand::PauseExecution),
                        metadata("in-1"),
                    ),
                    outputs: Vec::new(),
                    projection: projection(),
                    outbox: rows,
                    checkpoint: None,
                    timers: Vec::new(),
                    attempts: Vec::new(),
                },
            )
            .await
            .expect("an append");
        store
    }

    #[tokio::test]
    async fn a_pass_sends_every_pending_row_and_marks_it() {
        let store = store_with(vec![
            row("a", "workflow.declared"),
            row("b", "execution.started"),
        ])
        .await;
        let sink = Arc::new(InMemoryBus::new());

        let published = publish_pending(&store, sink.as_ref(), OffsetDateTime::UNIX_EPOCH)
            .await
            .expect("a pass");
        assert_eq!(published.sent, 2);
        assert!(
            store
                .pending_outbox(10)
                .await
                .expect("the outbox")
                .is_empty(),
            "a sent row is marked"
        );

        // And a second pass has nothing to do, rather than sending them again.
        assert_eq!(
            publish_pending(&store, sink.as_ref(), OffsetDateTime::UNIX_EPOCH)
                .await
                .expect("a second pass"),
            Published::default()
        );
    }

    #[tokio::test]
    async fn a_row_this_build_cannot_read_is_counted_rather_than_dropped() {
        // A fact written by a newer build. Publishing the rest and leaving this
        // one pending is the honest answer: discarding it would lose a fact
        // with nothing able to say which.
        let mut broken = row("a", "execution.started");
        broken.payload = serde_json::json!({ "from": "a newer build" });
        let store = store_with(vec![broken, row("b", "execution.completed")]).await;
        let sink = Arc::new(InMemoryBus::new());

        let published = publish_pending(&store, sink.as_ref(), OffsetDateTime::UNIX_EPOCH)
            .await
            .expect("a pass");
        assert_eq!(published.sent, 1);
        assert_eq!(published.undecodable, 1);
        assert_eq!(
            store
                .pending_outbox(10)
                .await
                .expect("the outbox")
                .iter()
                .map(|row| row.message_id.to_string())
                .collect::<Vec<_>>(),
            vec!["a".to_owned()]
        );
    }

    #[tokio::test]
    async fn a_sink_that_refused_leaves_every_row_pending_for_the_next_pass() {
        #[derive(Debug)]
        struct Refuses;

        #[async_trait::async_trait]
        impl MessageSink for Refuses {
            async fn append(
                &self,
                _events: Vec<EventEnvelope>,
            ) -> aiwatcher_bus::ports::BusResult<aiwatcher_bus::ports::AppendResult> {
                Err(aiwatcher_bus::ports::BusError::Unavailable(
                    "the broker is down".to_owned(),
                ))
            }
        }

        let store = store_with(vec![row("a", "execution.started")]).await;
        let error = publish_pending(&store, &Refuses, OffsetDateTime::UNIX_EPOCH)
            .await
            .expect_err("a refused batch");
        assert!(error.to_string().contains("publishing execution facts"));
        assert_eq!(
            store.pending_outbox(10).await.expect("the outbox").len(),
            1,
            "the row is still pending, so the next pass re-sends it"
        );
    }
}
