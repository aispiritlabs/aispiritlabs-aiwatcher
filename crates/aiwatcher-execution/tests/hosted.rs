#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! A hosted execution's history: who may append to it, what a conflict does,
//! and what this engine refuses to read.
//!
//! Section 40.3. The decider is the worker; the compare-and-append, the inbox
//! and the version are this engine's, and they are the whole of what
//! `agentic.workflow` cannot give itself when two workers hold separate
//! SQLite files.

use std::collections::BTreeMap;

use aiwatcher_core::{CausationId, CorrelationId, MessageId};
use aiwatcher_execution::hosted::{
    HOSTED_APPEND, HostedAppend, HostedError, LeaseOutcome, MAX_BATCH,
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
    // Phase 13, step 1's exit. Two deciders reach the same expected version —
    // a lease that expired, a pod that came back — and exactly one of them
    // writes. The loser is told the version it actually lost to, which is what
    // it needs in order to reload rather than guess.
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
    // Phase 13, step 2's exit, in the order it is written: refused while the
    // first holds; a takeover once it has run out; and then the first worker's
    // next append is a 409 — because the replacement moved the version, which
    // is the guarantee the lease never was.
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
    // Section 40.3's "what the engine does not do", checked through a real
    // append rather than only on the type: the run's state after two turns is
    // the state it had before them.
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
    // The kickoff's first trap, section 43.10 under different names: an id
    // derived from less than what it identifies makes the second message of a
    // batch read as a redelivery of the first.
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
