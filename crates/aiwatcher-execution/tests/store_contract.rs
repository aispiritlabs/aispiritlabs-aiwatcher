#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! The two adapters that ship here, against the one suite.
//!
//! Every property is in `aiwatcher_execution::testing`, so the PostgreSQL
//! adapter in its own crate proves the same things rather than a similar set.
//! What stays here is what is true of *these* adapters and of no other: that a
//! file store survives the process that wrote it, refuses a second one by name,
//! and does not let two execution ids share a stream because they sanitise
//! alike.

use aiwatcher_execution::store::file::FileWorkflowStore;
use aiwatcher_execution::store::memory::MemoryWorkflowStore;
use aiwatcher_execution::testing::{
    appending_the_same_decision_twice_is_idempotent, assert_contract, fresh,
};
use aiwatcher_execution::{
    AppendRequest, ExecutionId, ExecutionMode, ExecutionOwner, ExpectedVersion, PendingMessage,
    RunProjection, RunState, StateType, StoreError, WorkflowCommand, WorkflowMessage,
    WorkflowStore, replay,
};
use time::OffsetDateTime;

/// A directory that removes itself, so a `file` store leaves nothing behind.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "aiwatcher-execution-{name}-{}-{}",
            std::process::id(),
            OffsetDateTime::now_utc().unix_timestamp_nanos()
        ));
        std::fs::create_dir_all(&path).expect("a scratch directory");
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn the_memory_store_keeps_the_contract() {
    let store = MemoryWorkflowStore::new();
    assert_contract("memory", &store).await;
    appending_the_same_decision_twice_is_idempotent("memory", &store)
        .await
        .expect("an idempotent append");
}

#[tokio::test]
async fn the_file_store_keeps_the_contract() {
    let scratch = Scratch::new("contract");
    let store = FileWorkflowStore::open(&scratch.0)
        .await
        .expect("a file store opens on an empty directory");
    assert_contract("file", &store).await;
    appending_the_same_decision_twice_is_idempotent("file", &store)
        .await
        .expect("an idempotent append");
}

#[tokio::test]
async fn a_file_store_survives_the_process_that_wrote_it() {
    // The property `just dev` depends on: restarting the server resumes rather
    // than restarts.
    let scratch = Scratch::new("restart");
    let execution = fresh("restart");
    {
        let store = FileWorkflowStore::open(&scratch.0).await.expect("a store");
        store
            .append(&execution, pause(&execution, "m-1"))
            .await
            .expect("an append");
    }
    let reopened = FileWorkflowStore::open(&scratch.0)
        .await
        .expect("the lock was released when the store was dropped");
    assert_eq!(
        reopened.load(&execution).await.expect("a load").version,
        1,
        "the stream survived the process"
    );
    assert!(
        reopened
            .projection(&execution)
            .await
            .expect("a projection")
            .is_some()
    );
    // A `replay` of a stream with no decision in it is an empty state, which is
    // the honest answer rather than a guess.
    assert!(
        replay(reopened.load(&execution).await.expect("a load").events())
            .active()
            .is_none()
    );
}

#[tokio::test]
async fn a_second_process_is_refused_the_file_store_by_name() {
    // The whole reason this adapter is not the default anywhere a worker runs:
    // a workflow stream has a decider, a reactor and a worker racing to append,
    // and a file offers no compare-and-append across processes.
    let scratch = Scratch::new("lock");
    let _held = FileWorkflowStore::open(&scratch.0)
        .await
        .expect("the first");
    let second = FileWorkflowStore::open(&scratch.0)
        .await
        .expect_err("a second process");
    assert!(matches!(second, StoreError::SingleProcessOnly), "{second}");
    assert!(
        second.to_string().contains("AIWATCHER_WORKFLOW_STORE"),
        "the refusal names the variable that fixes it: {second}"
    );
}

#[tokio::test]
async fn a_store_that_holds_one_process_says_so_before_a_run_needs_two() {
    // Read when a plan is accepted, not discovered when a worker cannot claim.
    let scratch = Scratch::new("capabilities");
    let file = FileWorkflowStore::open(&scratch.0).await.expect("a store");
    assert!(!file.capabilities().multi_process);
    assert!(!file.capabilities().claimable);
    assert!(MemoryWorkflowStore::new().capabilities().multi_process);
}

#[tokio::test]
async fn two_executions_whose_ids_sanitise_alike_keep_two_streams() {
    // An execution id is a caller's string and it becomes a file name. Two ids
    // sharing a stream is a run reading somebody else's history.
    let scratch = Scratch::new("collision");
    let store = FileWorkflowStore::open(&scratch.0).await.expect("a store");
    let one = ExecutionId::new("a/b");
    let other = ExecutionId::new("a:b");
    for (id, message) in [(&one, "m-1"), (&other, "m-2")] {
        store
            .append(id, pause(id, message))
            .await
            .expect("an append");
    }
    assert_eq!(store.load(&one).await.expect("a load").version, 1);
    assert_eq!(store.load(&other).await.expect("a load").version, 1);
}

/// An append with one inert input, for the properties that only need a stream
/// to exist.
fn pause(execution: &ExecutionId, message_id: &str) -> AppendRequest {
    AppendRequest {
        expected_version: ExpectedVersion::NoStream,
        input: PendingMessage::input(
            WorkflowMessage::Command(WorkflowCommand::PauseExecution),
            aiwatcher_execution::MessageMetadata {
                schema_version: aiwatcher_execution::message::SCHEMA_VERSION,
                message_id: aiwatcher_core::MessageId::new(message_id),
                occurred_at: OffsetDateTime::UNIX_EPOCH,
                correlation_id: aiwatcher_core::CorrelationId::new(execution.as_str()),
                causation_id: aiwatcher_core::CausationId::new(message_id),
                trace_id: None,
                span_id: None,
                step_id: None,
                attempt: None,
            },
        ),
        outputs: Vec::new(),
        projection: RunProjection {
            execution_id: execution.clone(),
            plan_id: String::new(),
            definition_name: "import".to_owned(),
            owner: ExecutionOwner::Local,
            mode: ExecutionMode::Compiled,
            state: RunState::of(StateType::Paused),
            requested_by: "somebody".to_owned(),
            steps: Vec::new(),
            last_message_version: 1,
            created_at: OffsetDateTime::UNIX_EPOCH,
        },
        outbox: Vec::new(),
        checkpoint: None,
        attempts: Vec::new(),
    }
}
