#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! The atomic command loop: what it retries, what it refuses, and what survives
//! a restart.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use aiwatcher_core::{CausationId, Checkpoint, CorrelationId, MessageId};
use aiwatcher_execution::message::{MessageMetadata, OutboxMessage, RunProjection, SCHEMA_VERSION};
use aiwatcher_execution::plan::{
    CachePolicy, DefinitionKind, DefinitionRevision, PlanStep, PythonTaskSpec, RetryPolicy,
    RuntimeBinding,
};
use aiwatcher_execution::store::memory::MemoryWorkflowStore;
use aiwatcher_execution::store::{
    AppendOutcome, AppendRequest, Pruned, StoreCapabilities, StreamSlice, WorkflowStore,
};
use aiwatcher_execution::{
    AttemptKey, AttemptRow, ClaimFilter, ExecutionHandler, ExecutionId, ExecutionMode,
    ExecutionOwner, ExecutionPlan, HandleError, Now, RuntimeKind, StateType, StoreError,
    WorkflowCommand, WorkflowEvent, WorkflowMessage,
};
use time::OffsetDateTime;

fn at(seconds: i64) -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(seconds)
}

fn plan() -> ExecutionPlan {
    ExecutionPlan::seal(
        DefinitionKind::Workflow,
        "import".to_owned(),
        DefinitionRevision("ab".repeat(32)),
        vec![PlanStep {
            id: "extract".to_owned(),
            runtime: RuntimeBinding::PythonTask(PythonTaskSpec {
                task_ref: "stage@1".to_owned(),
                queue: "default".to_owned(),
                params: BTreeMap::new(),
            }),
            inputs: Vec::new(),
            outputs: Vec::new(),
            retry: RetryPolicy::default(),
            timeout_seconds: 60,
            cache: CachePolicy::Never,
        }],
        Vec::new(),
    )
}

fn execution() -> ExecutionId {
    ExecutionId::new("exec-1")
}

fn metadata(id: &str) -> MessageMetadata {
    MessageMetadata {
        schema_version: SCHEMA_VERSION,
        message_id: MessageId::new(id),
        occurred_at: at(0),
        correlation_id: CorrelationId::new("exec-1"),
        causation_id: CausationId::new(id),
        trace_id: None,
        span_id: None,
        step_id: None,
        attempt: None,
    }
}

fn start() -> WorkflowMessage {
    WorkflowMessage::Command(WorkflowCommand::StartExecution {
        execution_id: execution(),
        plan: Box::new(plan()),
        owner: ExecutionOwner::Local,
        mode: ExecutionMode::Compiled,
        requested_by: "mk".to_owned(),
        input: BTreeMap::new(),
    })
}

#[tokio::test]
async fn one_start_writes_the_stream_the_projection_and_the_outbox() {
    let store = MemoryWorkflowStore::new();
    let handler = ExecutionHandler::new(store.clone());

    let handled = handler
        .handle(&execution(), start(), metadata("m-1"), Now::at(at(0)))
        .await
        .expect("a start");

    assert!(!handled.duplicate);
    assert_eq!(handled.projection.state.state_type, StateType::Running);
    assert_eq!(handled.projection.steps.len(), 1);
    assert_eq!(
        handled
            .outbox
            .iter()
            .map(|row| row.event_type.as_str())
            .collect::<Vec<_>>(),
        vec![
            "workflow.declared",
            "execution.requested",
            "execution.started"
        ],
        "and no `step.scheduled`: scheduling is a decision, and it stays in the store"
    );
    assert!(
        store
            .projection(&execution())
            .await
            .expect("a read")
            .is_some(),
        "the projection landed with the stream"
    );
}

#[tokio::test]
async fn a_caller_may_not_dispatch_a_step_behind_the_state_machines_back() {
    let handler = ExecutionHandler::new(MemoryWorkflowStore::new());
    let error = handler
        .handle(
            &execution(),
            WorkflowMessage::Command(WorkflowCommand::ExecuteStep {
                step_id: "extract".to_owned(),
                attempt: 1,
                runtime: RuntimeKind::PythonTask,
                idempotency_key: "exec-1/extract/1".to_owned(),
            }),
            metadata("m-1"),
            Now::at(at(0)),
        )
        .await
        .expect_err("an effect command from a caller");
    assert!(matches!(error, HandleError::Decision(_)), "{error}");
}

#[tokio::test]
async fn a_command_that_does_not_apply_is_refused_rather_than_retried() {
    // It will not apply on the second read either, and looping on it would turn
    // a 409 into a hang.
    let handler = ExecutionHandler::new(MemoryWorkflowStore::new());
    let error = handler
        .handle(
            &execution(),
            WorkflowMessage::Command(WorkflowCommand::PauseExecution),
            metadata("m-1"),
            Now::at(at(0)),
        )
        .await
        .expect_err("a pause before a start");
    assert!(
        error.to_string().contains("no execution has been started"),
        "{error}"
    );
}

/// A store that is the memory one in every way but its capabilities.
///
/// A `file` store would do, and would put a directory and a lock file in a test
/// about a decision. What is under test is the *rule*, and the rule reads one
/// boolean.
#[derive(Debug)]
struct OneProcess(MemoryWorkflowStore);

#[async_trait::async_trait]
impl WorkflowStore for OneProcess {
    fn capabilities(&self) -> StoreCapabilities {
        StoreCapabilities {
            multi_process: false,
            claimable: false,
        }
    }

    async fn load(&self, execution: &ExecutionId) -> aiwatcher_execution::Result<StreamSlice> {
        self.0.load(execution).await
    }

    async fn append(
        &self,
        execution: &ExecutionId,
        request: AppendRequest,
    ) -> aiwatcher_execution::Result<AppendOutcome> {
        self.0.append(execution, request).await
    }

    async fn projection(
        &self,
        execution: &ExecutionId,
    ) -> aiwatcher_execution::Result<Option<RunProjection>> {
        self.0.projection(execution).await
    }

    async fn pending_outbox(
        &self,
        limit: usize,
    ) -> aiwatcher_execution::Result<Vec<OutboxMessage>> {
        self.0.pending_outbox(limit).await
    }

    async fn mark_published(
        &self,
        ids: &[MessageId],
        at: OffsetDateTime,
    ) -> aiwatcher_execution::Result<()> {
        self.0.mark_published(ids, at).await
    }

    async fn claim_attempt(
        &self,
        filter: &ClaimFilter,
        owner: &str,
        now: OffsetDateTime,
    ) -> aiwatcher_execution::Result<Option<AttemptRow>> {
        self.0.claim_attempt(filter, owner, now).await
    }

    async fn heartbeat(
        &self,
        key: &AttemptKey,
        owner: &str,
        now: OffsetDateTime,
    ) -> aiwatcher_execution::Result<bool> {
        self.0.heartbeat(key, owner, now).await
    }

    async fn attempt(&self, key: &AttemptKey) -> aiwatcher_execution::Result<Option<AttemptRow>> {
        self.0.attempt(key).await
    }

    async fn admit_slot(
        &self,
        request: &aiwatcher_execution::SlotAdmissionRequest,
    ) -> aiwatcher_execution::Result<aiwatcher_execution::SlotAdmission> {
        self.0.admit_slot(request).await
    }

    async fn settle_slot(
        &self,
        key: &aiwatcher_execution::SlotKey,
        owner: &str,
        settlement: aiwatcher_execution::SlotSettlement,
        now: OffsetDateTime,
    ) -> aiwatcher_execution::Result<()> {
        self.0.settle_slot(key, owner, settlement, now).await
    }

    async fn recent_slots(
        &self,
        kind: aiwatcher_execution::plan::DefinitionKind,
        name: &str,
        limit: usize,
    ) -> aiwatcher_execution::Result<Vec<aiwatcher_execution::SlotRecord>> {
        self.0.recent_slots(kind, name, limit).await
    }

    async fn checkpoint(&self, processor: &str) -> aiwatcher_execution::Result<Option<Checkpoint>> {
        self.0.checkpoint(processor).await
    }

    async fn advance_checkpoint(
        &self,
        processor: &str,
        checkpoint: Checkpoint,
    ) -> aiwatcher_execution::Result<()> {
        self.0.advance_checkpoint(processor, checkpoint).await
    }

    async fn prune(
        &self,
        before: OffsetDateTime,
        limit: usize,
    ) -> aiwatcher_execution::Result<Pruned> {
        self.0.prune(before, limit).await
    }
}

#[tokio::test]
async fn a_plan_that_needs_a_worker_is_refused_by_a_store_that_holds_one_process() {
    // Before the run rather than when nothing claims the step. The second
    // failure is the invisible one: an attempt no worker can ever take looks
    // exactly like a worker that is busy, and no log anywhere says otherwise.
    let handler = ExecutionHandler::new(OneProcess(MemoryWorkflowStore::new()));
    let error = handler
        .handle(&execution(), start(), metadata("m-1"), Now::at(at(0)))
        .await
        .expect_err("a python task on a single-process store");

    assert!(
        matches!(error, HandleError::NeedsMultiProcess { .. }),
        "{error}"
    );
    assert!(error.to_string().contains("'extract'"), "{error}");
    assert!(
        error.to_string().contains("AIWATCHER_WORKFLOW_STORE"),
        "the refusal names the variable that fixes it: {error}"
    );

    // And nothing was written: a refused start is not half an execution.
    assert!(
        handler
            .store()
            .load(&execution())
            .await
            .expect("a load")
            .is_empty()
    );
}

/// A store that refuses the first `n` appends with a version conflict.
#[derive(Debug)]
struct Contends {
    inner: MemoryWorkflowStore,
    remaining: AtomicUsize,
}

#[async_trait::async_trait]
impl WorkflowStore for Contends {
    fn capabilities(&self) -> StoreCapabilities {
        self.inner.capabilities()
    }

    async fn load(&self, execution: &ExecutionId) -> aiwatcher_execution::Result<StreamSlice> {
        self.inner.load(execution).await
    }

    async fn append(
        &self,
        execution: &ExecutionId,
        request: AppendRequest,
    ) -> aiwatcher_execution::Result<AppendOutcome> {
        if self
            .remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            return Err(StoreError::VersionConflict {
                expected: 0,
                actual: 3,
            });
        }
        self.inner.append(execution, request).await
    }

    async fn projection(
        &self,
        execution: &ExecutionId,
    ) -> aiwatcher_execution::Result<Option<RunProjection>> {
        self.inner.projection(execution).await
    }

    async fn pending_outbox(
        &self,
        limit: usize,
    ) -> aiwatcher_execution::Result<Vec<OutboxMessage>> {
        self.inner.pending_outbox(limit).await
    }

    async fn mark_published(
        &self,
        ids: &[MessageId],
        at: OffsetDateTime,
    ) -> aiwatcher_execution::Result<()> {
        self.inner.mark_published(ids, at).await
    }

    async fn claim_attempt(
        &self,
        filter: &ClaimFilter,
        owner: &str,
        now: OffsetDateTime,
    ) -> aiwatcher_execution::Result<Option<AttemptRow>> {
        self.inner.claim_attempt(filter, owner, now).await
    }

    async fn heartbeat(
        &self,
        key: &AttemptKey,
        owner: &str,
        now: OffsetDateTime,
    ) -> aiwatcher_execution::Result<bool> {
        self.inner.heartbeat(key, owner, now).await
    }

    async fn attempt(&self, key: &AttemptKey) -> aiwatcher_execution::Result<Option<AttemptRow>> {
        self.inner.attempt(key).await
    }

    async fn admit_slot(
        &self,
        request: &aiwatcher_execution::SlotAdmissionRequest,
    ) -> aiwatcher_execution::Result<aiwatcher_execution::SlotAdmission> {
        self.inner.admit_slot(request).await
    }

    async fn settle_slot(
        &self,
        key: &aiwatcher_execution::SlotKey,
        owner: &str,
        settlement: aiwatcher_execution::SlotSettlement,
        now: OffsetDateTime,
    ) -> aiwatcher_execution::Result<()> {
        self.inner.settle_slot(key, owner, settlement, now).await
    }

    async fn recent_slots(
        &self,
        kind: aiwatcher_execution::plan::DefinitionKind,
        name: &str,
        limit: usize,
    ) -> aiwatcher_execution::Result<Vec<aiwatcher_execution::SlotRecord>> {
        self.inner.recent_slots(kind, name, limit).await
    }

    async fn checkpoint(&self, processor: &str) -> aiwatcher_execution::Result<Option<Checkpoint>> {
        self.inner.checkpoint(processor).await
    }

    async fn advance_checkpoint(
        &self,
        processor: &str,
        checkpoint: Checkpoint,
    ) -> aiwatcher_execution::Result<()> {
        self.inner.advance_checkpoint(processor, checkpoint).await
    }

    async fn prune(
        &self,
        before: OffsetDateTime,
        limit: usize,
    ) -> aiwatcher_execution::Result<Pruned> {
        self.inner.prune(before, limit).await
    }
}

#[tokio::test]
async fn a_conflict_is_re_read_and_decided_again_rather_than_surfaced() {
    // Optimistic on purpose: `decide` does no I/O, so re-reading and deciding
    // again is cheap — while a lock would serialise every command on an
    // execution for the duration of a decision that never blocks.
    let handler = ExecutionHandler::new(Contends {
        inner: MemoryWorkflowStore::new(),
        remaining: AtomicUsize::new(2),
    });
    let handled = handler
        .handle(&execution(), start(), metadata("m-1"), Now::at(at(0)))
        .await
        .expect("two conflicts are absorbed");
    assert!(!handled.duplicate);
}

#[tokio::test]
async fn contention_that_does_not_settle_is_reported_rather_than_looped_on() {
    let handler = ExecutionHandler::new(Contends {
        inner: MemoryWorkflowStore::new(),
        remaining: AtomicUsize::new(99),
    });
    let error = handler
        .handle(&execution(), start(), metadata("m-1"), Now::at(at(0)))
        .await
        .expect_err("a store that never settles");
    assert!(matches!(error, HandleError::Contended { .. }), "{error}");
    assert!(error.to_string().contains("try again"), "{error}");
}

#[tokio::test]
async fn a_restart_resumes_from_the_stream_rather_than_starting_again() {
    // The whole point of keeping the history: a new handler over the same store
    // continues a run it never saw start.
    let store = MemoryWorkflowStore::new();
    ExecutionHandler::new(store.clone())
        .handle(&execution(), start(), metadata("m-1"), Now::at(at(0)))
        .await
        .expect("a start");

    let after_restart = ExecutionHandler::new(store.clone());
    let handled = after_restart
        .handle(
            &execution(),
            WorkflowMessage::Event(WorkflowEvent::StepCompleted {
                step_id: "extract".to_owned(),
                attempt: 1,
                outputs: Vec::new(),
                result: None,
            }),
            metadata("m-2"),
            Now::at(at(10)),
        )
        .await
        .expect("a completion after a restart");

    assert_eq!(handled.projection.state.state_type, StateType::Completed);
    assert!(
        handled
            .outbox
            .iter()
            .any(|row| row.event_type == "execution.completed")
    );
}

#[tokio::test]
async fn an_input_from_the_log_carries_its_cursor_with_the_decision() {
    let store = MemoryWorkflowStore::new();
    let handler = ExecutionHandler::new(store.clone());
    handler
        .handle_from_log(
            &execution(),
            start(),
            metadata("m-1"),
            Now::at(at(0)),
            "execution",
            Checkpoint::from_global_position(12),
        )
        .await
        .expect("a start from the log");

    assert_eq!(
        store.checkpoint("execution").await.expect("a checkpoint"),
        Some(Checkpoint::from_global_position(12))
    );
}

#[tokio::test]
async fn a_redelivered_log_input_still_moves_the_cursor_past_itself() {
    // Otherwise the processor re-reads it forever. Advancing afterwards is the
    // right way round: a crash in between re-reads a message the inbox knows.
    let store = MemoryWorkflowStore::new();
    let handler = ExecutionHandler::new(store.clone());
    for position in [12u64, 13] {
        let handled = handler
            .handle_from_log(
                &execution(),
                start(),
                metadata("m-1"),
                Now::at(at(0)),
                "execution",
                Checkpoint::from_global_position(position),
            )
            .await
            .expect("a start, then its redelivery");
        assert_eq!(handled.duplicate, position == 13);
    }
    assert_eq!(
        store.checkpoint("execution").await.expect("a checkpoint"),
        Some(Checkpoint::from_global_position(13)),
        "the cursor moved past a message the inbox had already handled"
    );
}

#[tokio::test]
async fn a_redelivery_returns_the_state_it_produced_and_publishes_nothing() {
    let store = MemoryWorkflowStore::new();
    let handler = ExecutionHandler::new(store.clone());
    handler
        .handle(&execution(), start(), metadata("m-1"), Now::at(at(0)))
        .await
        .expect("a start");
    let again = handler
        .handle(&execution(), start(), metadata("m-1"), Now::at(at(0)))
        .await
        .expect("a redelivery");

    assert!(again.duplicate);
    assert!(
        again.outbox.is_empty(),
        "a redelivery published a second time"
    );
    assert_eq!(again.projection.state.state_type, StateType::Running);
    assert_eq!(again.outcome.version(), 1);
}

#[tokio::test]
async fn a_dispatch_becomes_a_row_the_reactor_that_holds_that_client_can_claim() {
    // The loop Phase 3 closes: the decider dispatches, the store holds the
    // claim, and the process with the runtime's client takes it. No second
    // topic, no consumer group — the store is transactional, so a command is a
    // row (section 11.1).
    let store = MemoryWorkflowStore::new();
    let handler = ExecutionHandler::new(store.clone());
    handler
        .handle(&execution(), start(), metadata("m-1"), Now::at(at(0)))
        .await
        .expect("a start");

    let claimed = store
        .claim_attempt(
            &ClaimFilter::for_queues(&["default".to_owned()]),
            "worker-1",
            at(0),
        )
        .await
        .expect("a claim")
        .expect("the dispatched attempt is claimable");

    assert_eq!(claimed.key.step_id, "extract");
    assert_eq!(claimed.key.attempt, 1);
    assert_eq!(claimed.key.idempotency_key(), "exec-1/extract/1");
    assert_eq!(claimed.task_ref.as_deref(), Some("stage@1"));
    assert_eq!(claimed.lease_owner.as_deref(), Some("worker-1"));
}

#[tokio::test]
async fn a_completion_settles_the_row_so_nobody_runs_the_step_twice() {
    let store = MemoryWorkflowStore::new();
    let handler = ExecutionHandler::new(store.clone());
    handler
        .handle(&execution(), start(), metadata("m-1"), Now::at(at(0)))
        .await
        .expect("a start");
    handler
        .handle(
            &execution(),
            WorkflowMessage::Event(WorkflowEvent::StepCompleted {
                step_id: "extract".to_owned(),
                attempt: 1,
                outputs: Vec::new(),
                result: None,
            }),
            metadata("m-2"),
            Now::at(at(10)),
        )
        .await
        .expect("a completion");

    // Even long past the lease, which is the case that matters: an expired
    // claim on a *finished* attempt must not become a second run of it.
    let past = at(aiwatcher_jobs::LEASE_SECONDS + 1);
    assert!(
        store
            .claim_attempt(
                &ClaimFilter::for_queues(&["default".to_owned()]),
                "worker-2",
                past,
            )
            .await
            .expect("a claim attempt")
            .is_none()
    );
}

#[tokio::test]
async fn a_scheduled_retry_is_not_claimable_until_its_delay_has_passed() {
    let store = MemoryWorkflowStore::new();
    let handler = ExecutionHandler::new(store.clone());
    handler
        .handle(&execution(), start(), metadata("m-1"), Now::at(at(0)))
        .await
        .expect("a start");
    handler
        .handle(
            &execution(),
            WorkflowMessage::Event(WorkflowEvent::StepFailed {
                step_id: "extract".to_owned(),
                attempt: 1,
                error: aiwatcher_execution::StepError::new(
                    aiwatcher_execution::FailureClass::Transient,
                    "connection reset",
                ),
            }),
            metadata("m-2"),
            Now::at(at(10)),
        )
        .await
        .expect("a transient failure");

    let filter = ClaimFilter::for_queues(&["default".to_owned()]);
    // The policy's first delay for a runtime that declined is five seconds,
    // resolved to an instant by the decider so a replay reaches the same
    // schedule. The row carries it, so a claimant cannot take the retry early
    // by asking again.
    assert!(
        store
            .claim_attempt(&filter, "worker", at(14))
            .await
            .expect("a claim attempt")
            .is_none(),
        "the retry was taken before its backoff"
    );
    let taken = store
        .claim_attempt(&filter, "worker", at(15))
        .await
        .expect("a claim attempt")
        .expect("the retry is claimable now");
    assert_eq!(taken.key.attempt, 2);
}
