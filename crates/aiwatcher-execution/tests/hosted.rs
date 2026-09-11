#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! A hosted execution's history: who may append to it, what a conflict does,
//! and what this engine refuses to read.
//!
//! The decider is the worker; the compare-and-append, the inbox
//! and the version are this engine's, and they are the whole of what
//! `agentic.workflow` cannot give itself when two workers hold separate
//! SQLite files.

use std::collections::BTreeMap;

use aiwatcher_core::{CausationId, CorrelationId, MessageId};
use aiwatcher_execution::hosted::{
    HOSTED_APPEND, HostedAppend, HostedError, LeaseOutcome, MAX_BATCH, Timer, TimerWrite,
};
use aiwatcher_execution::message::{
    HostedMessage, MessageMetadata, PayloadPolicy, PayloadRef, SCHEMA_VERSION,
};
use aiwatcher_execution::plan::{
    CachePolicy, DefinitionKind, DefinitionRevision, PlanStep, PythonTaskSpec, RetryPolicy,
    RuntimeBinding,
};
use aiwatcher_execution::store::WorkflowStore;
use aiwatcher_execution::store::memory::MemoryWorkflowStore;
use aiwatcher_execution::{
    ExecutionHandler, ExecutionId, ExecutionMode, ExecutionOwner, ExecutionPlan, HandleError, Now,
    StoreError, WorkflowCommand, WorkflowMessage,
};
use time::OffsetDateTime;

fn at(seconds: i64) -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(seconds)
}

fn execution() -> ExecutionId {
    ExecutionId::new("graph-1")
}

/// A hosted run still pins a plan: its *shape*, which is what the panel draws
/// and what `workflow.declared` carries. What it does not pin is the sequence,
/// because the worker decides that.
fn plan() -> ExecutionPlan {
    ExecutionPlan::seal(
        DefinitionKind::Workflow,
        "searcher-summarizer".to_owned(),
        DefinitionRevision("cd".repeat(32)),
        vec![PlanStep {
            id: "searcher".to_owned(),
            runtime: RuntimeBinding::PythonTask(PythonTaskSpec {
                task_ref: "graph@1".to_owned(),
                queue: "agents".to_owned(),
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

fn metadata(id: &str) -> MessageMetadata {
    MessageMetadata {
        schema_version: SCHEMA_VERSION,
        message_id: MessageId::new(id),
        occurred_at: at(0),
        correlation_id: CorrelationId::new(execution().as_str()),
        causation_id: CausationId::new(id),
        trace_id: None,
        span_id: None,
        step_id: None,
        attempt: None,
    }
}

fn start(mode: ExecutionMode, owner: ExecutionOwner) -> WorkflowMessage {
    WorkflowMessage::Command(WorkflowCommand::StartExecution {
        execution_id: execution(),
        plan: Box::new(plan()),
        owner,
        mode,
        payloads: PayloadPolicy::External,
        requested_by: "the worker".to_owned(),
        input: BTreeMap::new(),
    })
}

fn turn(name: &str) -> HostedMessage {
    HostedMessage {
        message_type: name.to_owned(),
        metadata: serde_json::json!({ "node": "searcher" }),
        payload: Some(PayloadRef {
            reference: "agentic://turn/1".to_owned(),
            digest: "e".repeat(64),
            size: 512,
            policy: PayloadPolicy::External,
        }),
    }
}

fn batch(key: &str, expected: u64, messages: Vec<HostedMessage>) -> HostedAppend {
    by("worker-a", key, expected, messages)
}

fn by(holder: &str, key: &str, expected: u64, messages: Vec<HostedMessage>) -> HostedAppend {
    HostedAppend {
        expected_version: expected,
        idempotency_key: key.to_owned(),
        holder: holder.to_owned(),
        messages,
        timers: Vec::new(),
    }
}

async fn started(mode: ExecutionMode) -> ExecutionHandler<MemoryWorkflowStore> {
    let handler = ExecutionHandler::new(MemoryWorkflowStore::new());
    handler
        .handle(
            &execution(),
            start(mode, ExecutionOwner::Worker),
            metadata("start"),
            Now::at(at(0)),
        )
        .await
        .expect("starting the execution");
    handler
}

fn conflict(error: &HostedError) -> Option<(u64, u64)> {
    match error {
        HostedError::Handle(HandleError::Store(StoreError::VersionConflict {
            expected,
            actual,
        })) => Some((*expected, *actual)),
        _ => None,
    }
}

#[tokio::test]
async fn two_workers_appending_at_one_version_produce_one_append_and_one_conflict() {
    // Two deciders reach the same expected version — a lease that expired, a
    // pod that came back — and exactly one of them writes. The loser is told
    // the version it actually lost to, which is what it needs in order to
    // reload rather than guess.
    let handler = started(ExecutionMode::Hosted).await;
    let version = handler
        .store()
        .load(&execution())
        .await
        .expect("loading")
        .version;

    handler
        .append_hosted(
            &execution(),
            batch("first", version, vec![turn("TurnStarted")]),
            at(1),
        )
        .await
        .expect("the first append at that version");

    let error = handler
        .append_hosted(
            &execution(),
            batch("second", version, vec![turn("TurnStarted")]),
            at(2),
        )
        .await
        .expect_err("the second append at the same version");
    let (expected, actual) =
        conflict(&error).unwrap_or_else(|| panic!("a version conflict, not {error}"));
    assert_eq!(expected, version);
    assert!(
        actual > version,
        "the conflict names where the stream got to"
    );

    // And the loser reloads and succeeds — the whole reason the number is on
    // the error rather than only in a log line.
    let reloaded = handler
        .store()
        .load(&execution())
        .await
        .expect("reloading")
        .version;
    handler
        .append_hosted(
            &execution(),
            batch("second", reloaded, vec![turn("TurnStarted")]),
            at(3),
        )
        .await
        .expect("the loser's second try");
}

#[tokio::test]
async fn a_hosted_run_schedules_none_of_its_plan_s_steps() {
    // A hosted run's plan is its shape, not its program. Dispatching from the
    // shape would put a claimable attempt in front of every reactor for work
    // the worker is also doing — two parties executing one step, which is the
    // whole reason the decider and the history are split apart.
    async fn dispatches(mode: ExecutionMode) -> usize {
        let handler = started(mode).await;
        handler
            .store()
            .load(&execution())
            .await
            .expect("loading")
            .messages
            .iter()
            .filter(|recorded| {
                matches!(
                    recorded.message.command(),
                    Some(WorkflowCommand::ExecuteStep { .. })
                )
            })
            .count()
    }

    assert_eq!(
        dispatches(ExecutionMode::Compiled).await,
        1,
        "a compiled run dispatches its first step"
    );
    assert_eq!(
        dispatches(ExecutionMode::Hosted).await,
        0,
        "a hosted run dispatches nothing: the worker schedules its own next node"
    );
}

#[tokio::test]
async fn a_second_decider_waits_takes_over_when_the_lease_runs_out_and_the_first_gets_a_409() {
    // In the order it is written: refused while the first holds; a takeover
    // once it has run out; and then the first worker's next append is a 409 —
    // because the replacement moved the version, which is the guarantee the
    // lease never was.
    let handler = started(ExecutionMode::Hosted).await;
    let start = at(0);
    let after = at(aiwatcher_jobs::LEASE_SECONDS + 1);
    let version = handler
        .store()
        .load(&execution())
        .await
        .expect("loading")
        .version;

    assert!(
        handler
            .take_decider_lease(&execution(), "worker-a", start)
            .await
            .expect("a first claim")
            .taken()
            .is_some()
    );
    let refused = handler
        .take_decider_lease(&execution(), "worker-b", at(1))
        .await
        .expect("a second claim");
    assert!(
        matches!(&refused, LeaseOutcome::Held { holder, .. } if holder == "worker-a"),
        "{refused:?}"
    );

    // While `worker-a` holds it, `worker-b` cannot append either — told before
    // it pays for the turn rather than after.
    let blocked = handler
        .append_hosted(
            &execution(),
            by("worker-b", "b-1", version, vec![turn("TurnStarted")]),
            at(1),
        )
        .await
        .expect_err("an append by the worker that does not hold the lease");
    assert!(
        matches!(blocked, HostedError::LeaseHeld { .. }),
        "{blocked}"
    );

    // The lease runs out and the replacement takes it, and is told whose it was.
    let taken = handler
        .take_decider_lease(&execution(), "worker-b", after)
        .await
        .expect("a takeover");
    let lease = taken.taken().expect("the takeover was refused");
    assert_eq!(lease.previous_holder.as_deref(), Some("worker-a"));

    // `worker-b` decides, which moves the version.
    handler
        .append_hosted(
            &execution(),
            by("worker-b", "b-2", version, vec![turn("TurnCompleted")]),
            after,
        )
        .await
        .expect("the replacement's append");

    // And now the first worker's next append is refused — a 409 either way, and
    // this is the *better* of the two reasons. A version conflict tells a
    // decider to re-read and decide again, which is exactly what a replaced one
    // must not do; "somebody else is deciding this" tells it to stop. So the
    // lease is checked before the version, and a worker that was taken over
    // learns it was rather than looping on a conflict it can never win.
    let stale = handler
        .append_hosted(
            &execution(),
            by("worker-a", "a-2", version, vec![turn("TurnCompleted")]),
            after,
        )
        .await
        .expect_err("the replaced worker's append");
    assert!(
        matches!(&stale, HostedError::LeaseHeld { holder, .. } if holder == "worker-b"),
        "{stale}"
    );

    // The version is still the guarantee underneath, and it answers on its own
    // when the lease is not the reason: `worker-b` holds the lease and is
    // simply behind.
    let behind = handler
        .append_hosted(
            &execution(),
            by("worker-b", "b-3", version, vec![turn("TurnCompleted")]),
            after,
        )
        .await
        .expect_err("the holder appending at a version it has already passed");
    assert!(
        conflict(&behind).is_some(),
        "the holder's own stale append is a version conflict: {behind}"
    );
}

#[tokio::test]
async fn an_unleased_run_is_appendable_because_the_version_is_the_guarantee() {
    // Leasing is what a decider does to avoid duplicated work, not a permission
    // this route invents. A worker that never asks for one still cannot corrupt
    // a stream, which is the thing worth being clear about: the two mechanisms
    // answer different questions and neither stands in for the other.
    let handler = started(ExecutionMode::Hosted).await;
    let version = handler
        .store()
        .load(&execution())
        .await
        .expect("loading")
        .version;
    handler
        .append_hosted(
            &execution(),
            by(
                "nobody-in-particular",
                "m-1",
                version,
                vec![turn("TurnStarted")],
            ),
            at(1),
        )
        .await
        .expect("an append with no lease anywhere");
}

#[tokio::test]
async fn a_lease_is_refused_on_a_run_this_engine_decides() {
    // A lease on a compiled run would be a row nothing ever reads, and the
    // refusal belongs where the run's mode is known rather than where somebody
    // later wonders why the lease did nothing.
    let handler = started(ExecutionMode::Compiled).await;
    let error = handler
        .take_decider_lease(&execution(), "worker-a", at(0))
        .await
        .expect_err("a lease on a compiled run");
    assert!(matches!(error, HostedError::NotHosted { .. }), "{error}");
}

/// A store that reports [`StoreCapabilities::multi_process`] false.
///
/// Wrapping the memory one rather than opening a file store, because what is
/// being checked is the *capability*, and borrowing a real single-process
/// adapter would also borrow its locking and its disk.
#[derive(Debug)]
struct OneProcess(MemoryWorkflowStore);

#[async_trait::async_trait]
impl WorkflowStore for OneProcess {
    fn capabilities(&self) -> aiwatcher_execution::store::StoreCapabilities {
        aiwatcher_execution::store::StoreCapabilities {
            multi_process: false,
            claimable: false,
        }
    }

    async fn load(
        &self,
        execution: &ExecutionId,
    ) -> aiwatcher_execution::Result<aiwatcher_execution::store::StreamSlice> {
        self.0.load(execution).await
    }

    async fn load_page(
        &self,
        execution: &ExecutionId,
        after: u64,
        limit: usize,
    ) -> aiwatcher_execution::Result<aiwatcher_execution::store::StreamSlice> {
        self.0.load_page(execution, after, limit).await
    }

    async fn due_timers(
        &self,
        now: OffsetDateTime,
        limit: usize,
    ) -> aiwatcher_execution::Result<Vec<Timer>> {
        self.0.due_timers(now, limit).await
    }

    async fn timers_of(&self, execution: &ExecutionId) -> aiwatcher_execution::Result<Vec<Timer>> {
        self.0.timers_of(execution).await
    }

    async fn recorded_outcome(
        &self,
        key: &aiwatcher_execution::AttemptKey,
    ) -> aiwatcher_execution::Result<Option<aiwatcher_execution::WorkflowEvent>> {
        self.0.recorded_outcome(key).await
    }

    async fn take_decider_lease(
        &self,
        execution: &ExecutionId,
        holder: &str,
        now: OffsetDateTime,
    ) -> aiwatcher_execution::Result<LeaseOutcome> {
        self.0.take_decider_lease(execution, holder, now).await
    }

    async fn release_decider_lease(
        &self,
        execution: &ExecutionId,
        holder: &str,
        now: OffsetDateTime,
    ) -> aiwatcher_execution::Result<bool> {
        self.0.release_decider_lease(execution, holder, now).await
    }

    async fn decider_lease(
        &self,
        execution: &ExecutionId,
    ) -> aiwatcher_execution::Result<Option<aiwatcher_execution::hosted::DeciderLease>> {
        self.0.decider_lease(execution).await
    }

    async fn append(
        &self,
        execution: &ExecutionId,
        request: aiwatcher_execution::store::AppendRequest,
    ) -> aiwatcher_execution::Result<aiwatcher_execution::store::AppendOutcome> {
        self.0.append(execution, request).await
    }

    async fn projection(
        &self,
        execution: &ExecutionId,
    ) -> aiwatcher_execution::Result<Option<aiwatcher_execution::message::RunProjection>> {
        self.0.projection(execution).await
    }

    async fn pending_outbox(
        &self,
        limit: usize,
    ) -> aiwatcher_execution::Result<Vec<aiwatcher_execution::message::OutboxMessage>> {
        self.0.pending_outbox(limit).await
    }

    async fn mark_published(
        &self,
        ids: &[aiwatcher_core::MessageId],
        at: OffsetDateTime,
    ) -> aiwatcher_execution::Result<()> {
        self.0.mark_published(ids, at).await
    }

    async fn claim_attempt(
        &self,
        filter: &aiwatcher_execution::ClaimFilter,
        owner: &str,
        now: OffsetDateTime,
    ) -> aiwatcher_execution::Result<Option<aiwatcher_execution::AttemptRow>> {
        self.0.claim_attempt(filter, owner, now).await
    }

    async fn heartbeat(
        &self,
        key: &aiwatcher_execution::AttemptKey,
        owner: &str,
        now: OffsetDateTime,
    ) -> aiwatcher_execution::Result<bool> {
        self.0.heartbeat(key, owner, now).await
    }

    async fn attempt(
        &self,
        key: &aiwatcher_execution::AttemptKey,
    ) -> aiwatcher_execution::Result<Option<aiwatcher_execution::AttemptRow>> {
        self.0.attempt(key).await
    }

    async fn admit_slot(
        &self,
        request: &aiwatcher_execution::schedule::slot::SlotAdmissionRequest,
    ) -> aiwatcher_execution::Result<aiwatcher_execution::schedule::slot::SlotAdmission> {
        self.0.admit_slot(request).await
    }

    async fn settle_slot(
        &self,
        key: &aiwatcher_execution::schedule::slot::SlotKey,
        owner: &str,
        settlement: aiwatcher_execution::schedule::slot::SlotSettlement,
        now: OffsetDateTime,
    ) -> aiwatcher_execution::Result<()> {
        self.0.settle_slot(key, owner, settlement, now).await
    }

    async fn recent_slots(
        &self,
        kind: aiwatcher_execution::plan::DefinitionKind,
        name: &str,
        limit: usize,
    ) -> aiwatcher_execution::Result<Vec<aiwatcher_execution::schedule::slot::SlotRecord>> {
        self.0.recent_slots(kind, name, limit).await
    }

    async fn checkpoint(
        &self,
        processor: &str,
    ) -> aiwatcher_execution::Result<Option<aiwatcher_core::Checkpoint>> {
        self.0.checkpoint(processor).await
    }

    async fn advance_checkpoint(
        &self,
        processor: &str,
        checkpoint: aiwatcher_core::Checkpoint,
    ) -> aiwatcher_execution::Result<()> {
        self.0.advance_checkpoint(processor, checkpoint).await
    }

    async fn prune(
        &self,
        before: OffsetDateTime,
        limit: usize,
    ) -> aiwatcher_execution::Result<aiwatcher_execution::store::Pruned> {
        self.0.prune(before, limit).await
    }

    async fn unclaimed_attempts(
        &self,
        now: OffsetDateTime,
    ) -> aiwatcher_execution::Result<
        std::collections::BTreeMap<aiwatcher_execution::RuntimeKind, u64>,
    > {
        self.0.unclaimed_attempts(now).await
    }

    async fn claimable_attempts(
        &self,
        runtime: aiwatcher_execution::RuntimeKind,
        now: OffsetDateTime,
        limit: usize,
    ) -> aiwatcher_execution::Result<Vec<aiwatcher_execution::AttemptRow>> {
        self.0.claimable_attempts(runtime, now, limit).await
    }
}

fn saga_timeout(timer_id: &str, due_at: OffsetDateTime) -> TimerWrite {
    TimerWrite::Schedule(Timer {
        execution: execution(),
        timer_id: timer_id.to_owned(),
        due_at,
        // `agentic`'s own name for what a saga's timeout produces. Stored whole
        // and handed back unchanged: this engine defers an append and composes
        // nothing.
        message: HostedMessage {
            message_type: "saga.timeout_fired".to_owned(),
            metadata: serde_json::json!({ "timeout_id": timer_id }),
            payload: None,
        },
    })
}

#[tokio::test]
async fn a_timeout_scheduled_before_a_restart_fires_after_it_and_fires_once() {
    // `agentic.workflow.Saga` has had
    // `schedule_timeout`, `due_timeouts` and `fire_timeout` all along; what it
    // has never had is something that wakes up and looks, because a worker
    // holding its own SQLite is not running when the timeout comes due.
    // The store is the shared, durable half — that is the whole premise of a
    // hosted run — so what restarts here is the *worker*: one handler schedules
    // the timeout and goes away, and a second one, holding nothing it learnt,
    // delivers it. Durability across a restart of the store itself is the
    // contract suite's, against the adapters that have a disk.
    let store = MemoryWorkflowStore::new();
    let due = at(300);

    {
        let handler = ExecutionHandler::new(store.clone());
        handler
            .handle(
                &execution(),
                start(ExecutionMode::Hosted, ExecutionOwner::Worker),
                metadata("start"),
                Now::at(at(0)),
            )
            .await
            .expect("starting the execution");
        let version = handler
            .store()
            .load(&execution())
            .await
            .expect("loading")
            .version;
        handler
            .append_hosted(
                &execution(),
                HostedAppend {
                    expected_version: version,
                    idempotency_key: "schedule".to_owned(),
                    holder: "worker-a".to_owned(),
                    messages: vec![turn("TurnStarted")],
                    timers: vec![saga_timeout("reply-deadline", due)],
                },
                at(1),
            )
            .await
            .expect("scheduling the timeout");
        assert!(
            handler
                .fire_due_timers(at(2), 10)
                .await
                .expect("a tick before it is due")
                .is_empty(),
            "a timer is not due before its time"
        );
    }
    // The worker is gone. Everything it knew is in the store.

    let handler = ExecutionHandler::new(store);
    assert_eq!(
        handler
            .store()
            .timers_of(&execution())
            .await
            .expect("the run's timers")
            .len(),
        1,
        "the timeout survived the restart"
    );

    let fired = handler
        .fire_due_timers(due, 10)
        .await
        .expect("the tick that delivers it");
    assert_eq!(fired.len(), 1);
    assert_eq!(fired[0].timer_id, "reply-deadline");
    assert!(fired[0].delivered);

    // Once. The message is recorded under an id derived from the execution and
    // the timer, and the append that delivered it retired the row in the same
    // transaction — so a second tick has nothing to find and could not append
    // beside the first if it did.
    assert!(
        handler
            .fire_due_timers(due, 10)
            .await
            .expect("a second tick")
            .is_empty()
    );
    let delivered: Vec<_> = handler
        .store()
        .load(&execution())
        .await
        .expect("loading")
        .messages
        .iter()
        .filter_map(|recorded| recorded.message.hosted())
        .filter(|hosted| hosted.message_type == "saga.timeout_fired")
        .cloned()
        .collect();
    assert_eq!(delivered.len(), 1, "fired once");
    assert_eq!(delivered[0].metadata["timeout_id"], "reply-deadline");
}

#[tokio::test]
async fn a_hosted_run_is_refused_on_a_store_that_holds_one_process() {
    // The decider *is* the second process, whatever the plan holds. The step
    // check cannot see that: a graph made only of Flow blocks needs no worker
    // for its steps, and would have been allowed on a store one process holds
    // while the worker deciding it appended from another.
    let handler = ExecutionHandler::new(OneProcess(MemoryWorkflowStore::new()));
    let refused = handler
        .handle(
            &execution(),
            start(ExecutionMode::Hosted, ExecutionOwner::Worker),
            metadata("start"),
            Now::at(at(0)),
        )
        .await
        .expect_err("a hosted run on a single-process store");
    assert!(
        matches!(&refused, HandleError::NeedsMultiProcess { what } if what.contains("decides it")),
        "{refused}"
    );
}

#[tokio::test]
async fn a_timer_on_a_run_that_has_ended_is_retired_rather_than_delivered() {
    // A row nothing can deliver would otherwise come back due on every tick for
    // ever. Retired with no output, which leaves the reason in the stream
    // rather than in a log line nobody reads.
    let handler = started(ExecutionMode::Compiled).await;
    let version = handler
        .store()
        .load(&execution())
        .await
        .expect("loading")
        .version;
    handler
        .store()
        .append(
            &execution(),
            aiwatcher_execution::store::AppendRequest {
                expected_version: aiwatcher_execution::store::ExpectedVersion::Exact(version),
                input: aiwatcher_execution::PendingMessage::input(
                    WorkflowMessage::Command(WorkflowCommand::PauseExecution),
                    metadata("park"),
                ),
                outputs: Vec::new(),
                projection: handler
                    .store()
                    .projection(&execution())
                    .await
                    .expect("a projection")
                    .expect("a run"),
                outbox: Vec::new(),
                checkpoint: None,
                timers: vec![saga_timeout("orphan", at(10))],
                attempts: Vec::new(),
            },
        )
        .await
        .expect("scheduling a timer on a compiled run");

    let fired = handler.fire_due_timers(at(20), 10).await.expect("a tick");
    assert_eq!(fired.len(), 1);
    assert!(
        !fired[0].delivered,
        "a run this engine decides is handed no hosted message"
    );
    assert!(
        handler
            .fire_due_timers(at(20), 10)
            .await
            .expect("a second tick")
            .is_empty(),
        "and it does not come back due for ever"
    );
}

#[tokio::test]
async fn a_message_governed_differently_from_its_run_is_refused_rather_than_corrected() {
    // A run started `external` and appending `sealed` references, or the other
    // way round, is one whose words are somewhere other than where it said they
    // would be. Accepting either would make the policy a label on a page.
    let handler = ExecutionHandler::new(MemoryWorkflowStore::new());
    handler
        .handle(
            &execution(),
            WorkflowMessage::Command(WorkflowCommand::StartExecution {
                execution_id: execution(),
                plan: Box::new(plan()),
                owner: ExecutionOwner::Worker,
                mode: ExecutionMode::Hosted,
                payloads: PayloadPolicy::Sealed,
                requested_by: "the worker".to_owned(),
                input: BTreeMap::new(),
            }),
            metadata("start"),
            Now::at(at(0)),
        )
        .await
        .expect("starting a sealed run");
    let version = handler
        .store()
        .load(&execution())
        .await
        .expect("loading")
        .version;

    let refused = handler
        .append_hosted(
            &execution(),
            batch("m-1", version, vec![turn("TurnCompleted")]),
            at(1),
        )
        .await
        .expect_err("an external payload on a sealed run");
    assert!(
        matches!(
            &refused,
            HostedError::PayloadPolicyMismatch { found, expected, .. }
                if *found == "external" && *expected == "sealed"
        ),
        "{refused}"
    );

    // And the same message under the run's own policy goes in.
    let mut sealed = turn("TurnCompleted");
    if let Some(payload) = sealed.payload.as_mut() {
        payload.policy = PayloadPolicy::Sealed;
    }
    handler
        .append_hosted(&execution(), batch("m-2", version, vec![sealed]), at(1))
        .await
        .expect("a sealed payload on a sealed run");
}

#[tokio::test]
async fn a_message_with_no_words_needs_no_policy() {
    // A join bucket and a timeout carry nothing for a policy to govern, and
    // demanding one of them would be demanding one of silence — which is most
    // of what makes a graph resumable.
    let handler = ExecutionHandler::new(MemoryWorkflowStore::new());
    handler
        .handle(
            &execution(),
            WorkflowMessage::Command(WorkflowCommand::StartExecution {
                execution_id: execution(),
                plan: Box::new(plan()),
                owner: ExecutionOwner::Worker,
                mode: ExecutionMode::Hosted,
                payloads: PayloadPolicy::Sealed,
                requested_by: "the worker".to_owned(),
                input: BTreeMap::new(),
            }),
            metadata("start"),
            Now::at(at(0)),
        )
        .await
        .expect("starting a sealed run");
    let version = handler
        .store()
        .load(&execution())
        .await
        .expect("loading")
        .version;

    handler
        .append_hosted(
            &execution(),
            batch(
                "join",
                version,
                vec![HostedMessage {
                    message_type: "JoinBucketFilled".to_owned(),
                    metadata: serde_json::json!({ "arity": 3 }),
                    payload: None,
                }],
            ),
            at(1),
        )
        .await
        .expect("a message with no payload");
}

#[tokio::test]
async fn a_redelivered_batch_is_recognised_rather_than_appended_twice() {
    // The `Idempotency-Key` is the durable inbox key, exactly as a command's
    // message id is. A worker that retried a request whose response it never
    // saw must not get a second copy of every message in it.
    let handler = started(ExecutionMode::Hosted).await;
    let version = handler
        .store()
        .load(&execution())
        .await
        .expect("loading")
        .version;
    let id = execution();
    let send = || {
        handler.append_hosted(
            &id,
            batch(
                "turn-7",
                version,
                vec![turn("TurnStarted"), turn("TurnCompleted")],
            ),
            at(1),
        )
    };

    let first = send().await.expect("the first send");
    assert!(!first.duplicate);
    let again = send().await.expect("the same send again");
    assert!(again.duplicate, "a redelivery, not a race");

    let slice = handler.store().load(&execution()).await.expect("loading");
    let hosted: Vec<_> = slice
        .messages
        .iter()
        .filter(|recorded| recorded.message.is_hosted())
        .collect();
    assert_eq!(hosted.len(), 3, "one marker and two messages, written once");

    // `Duplicate` names where the *input* landed rather than where the stream
    // got to — the version a caller looks the previous outcome up at, and one
    // that stays true as the stream grows past it.
    let marker = hosted
        .iter()
        .find(|recorded| recorded.message.name() == HOSTED_APPEND)
        .expect("the marker row");
    assert_eq!(again.outcome.version(), marker.stream_version);
    assert!(first.outcome.version() >= marker.stream_version);
}

#[tokio::test]
async fn a_worker_may_not_append_to_a_run_this_engine_decides() {
    // Two deciders on one run is the failure hosted mode exists to prevent, and
    // the refusal has to arrive on the append rather than after the messages
    // are in a stream nobody will fold.
    let handler = started(ExecutionMode::Compiled).await;
    let error = handler
        .append_hosted(
            &execution(),
            batch("m", 0, vec![turn("TurnStarted")]),
            at(1),
        )
        .await
        .expect_err("a compiled run refusing a worker's append");
    assert!(matches!(error, HostedError::NotHosted { .. }), "{error}");
}

#[tokio::test]
async fn a_hosted_message_never_reaches_the_fold() {
    // "What the engine does not do", checked through a real append rather than
    // only on the type: the run's state after two turns is the state it had
    // before them.
    let handler = started(ExecutionMode::Hosted).await;
    let before = handler
        .store()
        .projection(&execution())
        .await
        .expect("reading the projection")
        .expect("a started run");

    let handled = handler
        .append_hosted(
            &execution(),
            batch(
                "turn-1",
                before.last_message_version,
                vec![turn("TurnStarted"), turn("TurnCompleted")],
            ),
            at(1),
        )
        .await
        .expect("appending two turns");

    assert_eq!(handled.projection.state, before.state);
    assert!(
        handled.projection.last_message_version > before.last_message_version,
        "the version moves even though the state does not"
    );
    assert!(
        handled.outbox.is_empty(),
        "a fact this engine cannot honestly describe is not one it publishes"
    );
}

#[tokio::test]
async fn an_append_carries_between_one_message_and_the_batch_limit() {
    // An append of nothing at an expected version is a version check, and there
    // is a read for that; an unbounded one holds a row lock while a thousand
    // inserts run.
    let handler = started(ExecutionMode::Hosted).await;
    let empty = handler
        .append_hosted(&execution(), batch("none", 0, Vec::new()), at(1))
        .await
        .expect_err("an empty append");
    assert!(matches!(empty, HostedError::Empty), "{empty}");

    let too_many = handler
        .append_hosted(
            &execution(),
            batch("many", 0, vec![turn("TurnStarted"); MAX_BATCH + 1]),
            at(1),
        )
        .await
        .expect_err("an append past the limit");
    assert!(
        matches!(too_many, HostedError::TooManyMessages { count } if count == MAX_BATCH + 1),
        "{too_many}"
    );
}

#[tokio::test]
async fn a_batch_and_its_messages_are_named_by_everything_that_identifies_them() {
    // An id derived from less than what it identifies makes the second message
    // of a batch read as a redelivery of the first.
    let one = batch("k", 0, Vec::new());
    let other = ExecutionId::new("graph-2");

    assert_ne!(
        one.message_id(&execution(), 0),
        one.message_id(&execution(), 1),
        "two messages of one batch"
    );
    assert_ne!(
        one.message_id(&execution(), 0),
        one.message_id(&other, 0),
        "the same position in two executions"
    );
    assert_ne!(
        one.input_id(&execution()),
        batch("k2", 0, Vec::new()).input_id(&execution()),
        "two batches"
    );
    assert_ne!(
        one.input_id(&execution()),
        one.message_id(&execution(), 0),
        "the marker and the first message it carried"
    );

    // Derived, so a replay reaches the same ids rather than beside them.
    assert_eq!(one.input_id(&execution()), one.input_id(&execution()));
    assert_eq!(HOSTED_APPEND, "HostedAppend");
}
