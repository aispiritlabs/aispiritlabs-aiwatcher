// Assertions, in a module that only a test compiles. `clippy.toml`'s
// allowances reach `#[cfg(test)]` modules and this is not one — it is behind a
// feature so a *second crate's* tests can call it, which is the whole point.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! The storage contract, written once so every adapter proves the same thing.
//!
//! Section 29.2 of `docs/PIPELINE_ARCHITECTURE.md`. An adapter that passes a
//! *different* suite is an adapter that is correct about something else — and
//! the two that ship here are correct in different ways by construction, since
//! one holds everything under a mutex and the other under a transaction. That
//! is exactly why the properties live here and not beside either of them.
//!
//! Each property takes a store and a name, and every one of them mints its own
//! execution id. That matters for a shared PostgreSQL, where the suite runs
//! against a database somebody else's run also used: a property that reused a
//! fixed id would pass alone and fail in CI.
//!
//! Behind the `testing` feature, so none of this reaches a production build.

use time::OffsetDateTime;

use aiwatcher_core::{CausationId, Checkpoint, CorrelationId, MessageId};

use crate::claim::{AttemptKey, AttemptRow, ClaimFilter};
use crate::decide::{Now, decide, replay};
use crate::message::{
    MessageMetadata, OutboxMessage, PendingMessage, RunProjection, SCHEMA_VERSION, WorkflowCommand,
    WorkflowEvent, WorkflowMessage,
};
use crate::plan::{
    CachePolicy, DefinitionKind, DefinitionRevision, ExecutionPlan, PlanStep, PythonTaskSpec,
    RetryPolicy, RuntimeBinding, RuntimeKind,
};
use crate::state::{ExecutionId, ExecutionMode, ExecutionOwner, RunState, StateType, StepState};
use crate::store::{AppendRequest, ExpectedVersion, WorkflowStore};
use crate::{Result, StoreError};

/// A fresh id, so one property never reads another's stream.
#[must_use]
pub fn fresh(property: &str) -> ExecutionId {
    ExecutionId::new(format!(
        "contract-{property}-{}",
        OffsetDateTime::now_utc().unix_timestamp_nanos()
    ))
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
                queue: "houses".to_owned(),
                params: std::collections::BTreeMap::new(),
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

fn metadata(execution: &ExecutionId, message_id: &str) -> MessageMetadata {
    MessageMetadata {
        schema_version: SCHEMA_VERSION,
        message_id: MessageId::new(format!("{execution}/{message_id}")),
        occurred_at: OffsetDateTime::UNIX_EPOCH,
        correlation_id: CorrelationId::new(execution.as_str()),
        causation_id: CausationId::new(message_id),
        trace_id: None,
        span_id: None,
        step_id: None,
        attempt: None,
    }
}

fn start_command(execution: &ExecutionId) -> WorkflowMessage {
    WorkflowMessage::Command(WorkflowCommand::StartExecution {
        execution_id: execution.clone(),
        plan: Box::new(plan()),
        owner: ExecutionOwner::Local,
        mode: ExecutionMode::Compiled,
        requested_by: "somebody".to_owned(),
        input: std::collections::BTreeMap::new(),
    })
}

fn projection(execution: &ExecutionId, version: u64, state: StateType) -> RunProjection {
    RunProjection {
        execution_id: execution.clone(),
        plan_id: plan().plan_id.to_string(),
        definition_name: "import".to_owned(),
        owner: ExecutionOwner::Local,
        mode: ExecutionMode::Compiled,
        state: RunState::of(state),
        requested_by: "somebody".to_owned(),
        steps: vec![StepState::fresh(
            "extract".to_owned(),
            RuntimeKind::PythonTask,
        )],
        last_message_version: version,
        created_at: OffsetDateTime::UNIX_EPOCH,
        started_at: None,
        ended_at: None,
    }
}

fn outbox_row(execution: &ExecutionId, message_id: &str) -> OutboxMessage {
    OutboxMessage {
        message_id: MessageId::new(format!("{execution}/{message_id}")),
        event_type: "workflow.declared".to_owned(),
        partition_key: format!("workflow:{execution}"),
        payload: serde_json::json!({ "workflow_run_id": execution.as_str() }),
        available_at: OffsetDateTime::UNIX_EPOCH,
        attempts: 0,
        published_at: None,
        last_error: None,
    }
}

/// The decision a start produces, ready to append.
fn start_request(
    execution: &ExecutionId,
    expected: ExpectedVersion,
    message_id: &str,
) -> AppendRequest {
    let input = PendingMessage::input(start_command(execution), metadata(execution, message_id));
    let outputs = decide(
        &crate::initial_state(),
        &start_command(execution),
        &input.metadata.message_id,
        Now::at(OffsetDateTime::UNIX_EPOCH),
    )
    .expect("a start decides");
    let count = outputs.len() as u64;
    AppendRequest {
        expected_version: expected,
        input,
        outputs,
        projection: projection(execution, count + 1, StateType::Running),
        outbox: vec![outbox_row(execution, "outbox-1")],
        checkpoint: Some(("execution".to_owned(), Checkpoint::from_global_position(7))),
        attempts: Vec::new(),
    }
}

/// The queue this property's rows are claimable on.
///
/// Per execution, not a fixed name. `claim_attempt` takes the *oldest*
/// claimable row a filter matches, so on a store the properties share — and on
/// a PostgreSQL somebody else's run also used — a fixed queue means one
/// property claims another's row and the heartbeat that follows fails for a
/// reason that has nothing to do with the adapter.
fn queue_of(execution: &ExecutionId) -> String {
    format!("q-{execution}")
}

fn claimable(execution: &ExecutionId) -> AttemptRow {
    AttemptRow::claimable(
        AttemptKey::new(execution.clone(), "extract", 1),
        RuntimeKind::PythonTask,
        MessageId::new(format!("{execution}/cmd-1")),
    )
    .on_queue(queue_of(execution), "stage@1".to_owned())
}

/// A filter that sees only this execution's rows.
fn mine(execution: &ExecutionId) -> ClaimFilter {
    ClaimFilter::for_queues(&[queue_of(execution)])
}

/// An append whose only content is attempt rows.
fn dispatch(execution: &ExecutionId, message_id: &str, rows: Vec<AttemptRow>) -> AppendRequest {
    AppendRequest {
        expected_version: ExpectedVersion::Any,
        input: PendingMessage::input(
            WorkflowMessage::Command(WorkflowCommand::PauseExecution),
            metadata(execution, message_id),
        ),
        outputs: Vec::new(),
        projection: projection(execution, 1, StateType::Running),
        outbox: Vec::new(),
        checkpoint: None,
        attempts: rows,
    }
}

/// Run every property against one store.
///
/// # Panics
///
/// On the first property this store does not satisfy, naming it and the store.
pub async fn assert_contract(name: &str, store: &dyn WorkflowStore) {
    a_decision_only_lands_at_the_version_its_author_read(name, store).await;
    starting_one_execution_twice_produces_one_execution_and_one_conflict(name, store).await;
    a_redelivered_input_returns_what_it_produced(name, store).await;
    the_six_pieces_of_one_decision_land_together(name, store).await;
    a_refused_append_writes_none_of_them(name, store).await;
    publishing_an_outbox_row_is_safe_to_repeat(name, store).await;
    a_stream_read_back_replays_to_the_state_it_recorded(name, store).await;
    a_message_too_large_to_store_is_refused(name, store).await;
    two_claimants_racing_for_one_attempt_produce_one_claim(name, store).await;
    a_claimant_only_takes_what_it_said_it_could_run(name, store).await;
    a_lost_claim_expires_and_the_next_claimant_takes_it_over(name, store).await;
    a_heartbeat_keeps_a_long_step_from_being_taken_over(name, store).await;
    a_settled_attempt_is_out_of_every_claimants_view(name, store).await;
    a_retry_is_not_claimable_before_its_delay(name, store).await;
    a_cursor_advances_on_its_own_for_a_message_the_inbox_knew(name, store).await;
}

macro_rules! ok {
    ($name:expr, $expression:expr, $what:expr) => {
        match $expression.await {
            Ok(value) => value,
            Err(error) => panic!("{}: {}: {error}", $name, $what),
        }
    };
}

/// Somebody who read the stream before another append must not overwrite it.
pub async fn a_decision_only_lands_at_the_version_its_author_read(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("versions");
    let outcome = ok!(
        name,
        store.append(
            &execution,
            start_request(&execution, ExpectedVersion::NoStream, "m-1")
        ),
        "the first append"
    );
    assert!(!outcome.is_duplicate(), "{name}");
    let version = outcome.version();

    let stale = store
        .append(
            &execution,
            start_request(&execution, ExpectedVersion::Exact(0), "m-2"),
        )
        .await
        .expect_err("a stale expectation");
    assert!(
        matches!(stale, StoreError::VersionConflict { expected: 0, actual } if actual == version),
        "{name}: {stale}"
    );

    ok!(
        name,
        store.append(
            &execution,
            AppendRequest {
                expected_version: ExpectedVersion::Exact(version),
                input: PendingMessage::input(
                    WorkflowMessage::Command(WorkflowCommand::PauseExecution),
                    metadata(&execution, "m-3"),
                ),
                outputs: vec![PendingMessage::output(
                    WorkflowMessage::Event(WorkflowEvent::ExecutionPaused),
                    metadata(&execution, "m-3-out"),
                )],
                projection: projection(&execution, version + 2, StateType::Paused),
                outbox: Vec::new(),
                checkpoint: None,
                attempts: Vec::new(),
            },
        ),
        "an append at the current version"
    );
}

/// Two API replicas, one click: one execution and one conflict.
pub async fn starting_one_execution_twice_produces_one_execution_and_one_conflict(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("start-race");
    ok!(
        name,
        store.append(
            &execution,
            start_request(&execution, ExpectedVersion::NoStream, "a")
        ),
        "the first start"
    );
    let second = store
        .append(
            &execution,
            start_request(&execution, ExpectedVersion::NoStream, "b"),
        )
        .await
        .expect_err("a second start");
    assert!(
        matches!(second, StoreError::VersionConflict { expected: 0, .. }),
        "{name}: {second}"
    );
}

/// At-least-once delivery is the contract: a redelivery is not a conflict.
pub async fn a_redelivered_input_returns_what_it_produced(name: &str, store: &dyn WorkflowStore) {
    let execution = fresh("inbox");
    let first = ok!(
        name,
        store.append(
            &execution,
            start_request(&execution, ExpectedVersion::NoStream, "same")
        ),
        "the first delivery"
    );
    let again = ok!(
        name,
        store.append(
            &execution,
            start_request(&execution, ExpectedVersion::NoStream, "same")
        ),
        "the redelivery"
    );

    assert!(again.is_duplicate(), "{name}");
    // Where the *input* landed, which is where its previous result is looked
    // up — not where the stream happens to be now.
    assert_eq!(again.version(), 1, "{name}");
    let slice = ok!(name, store.load(&execution), "a load");
    assert_eq!(
        slice.version,
        first.version(),
        "{name}: a redelivery appended something"
    );
}

pub async fn the_six_pieces_of_one_decision_land_together(name: &str, store: &dyn WorkflowStore) {
    let execution = fresh("atomic");
    ok!(
        name,
        store.append(
            &execution,
            start_request(&execution, ExpectedVersion::NoStream, "m-1")
        ),
        "an append"
    );

    let slice = ok!(name, store.load(&execution), "a load");
    assert_eq!(
        slice.messages[0].direction,
        crate::Direction::Input,
        "{name}"
    );
    assert!(slice.messages.len() > 1, "{name}: the outputs landed too");
    assert!(
        ok!(name, store.projection(&execution), "a projection").is_some(),
        "{name}: the projection landed with the stream"
    );
    // By id, not by count: the suite shares a store with its other properties,
    // and on PostgreSQL with somebody else's run as well.
    let mine = MessageId::new(format!("{execution}/outbox-1"));
    assert!(
        ok!(name, store.pending_outbox(1000), "the outbox")
            .iter()
            .any(|row| row.message_id == mine),
        "{name}: the outbox row landed with the decision"
    );
    assert_eq!(
        ok!(name, store.checkpoint("execution"), "a checkpoint"),
        Some(Checkpoint::from_global_position(7)),
        "{name}"
    );
}

pub async fn a_refused_append_writes_none_of_them(name: &str, store: &dyn WorkflowStore) {
    let execution = fresh("all-or-nothing");
    let refused = store
        .append(
            &execution,
            start_request(&execution, ExpectedVersion::Exact(42), "m-1"),
        )
        .await;
    assert!(refused.is_err(), "{name}");

    assert!(
        ok!(name, store.load(&execution), "a load").is_empty(),
        "{name}"
    );
    assert!(
        ok!(name, store.projection(&execution), "a projection").is_none(),
        "{name}: a refused append left a projection behind"
    );
    let mine = MessageId::new(format!("{execution}/outbox-1"));
    assert!(
        !ok!(name, store.pending_outbox(1000), "the outbox")
            .iter()
            .any(|row| row.message_id == mine),
        "{name}: a refused append left an outbox row behind"
    );
}

/// The publisher crashes between the send and the mark often enough that this
/// is the normal case, not the edge one.
pub async fn publishing_an_outbox_row_is_safe_to_repeat(name: &str, store: &dyn WorkflowStore) {
    let execution = fresh("outbox");
    ok!(
        name,
        store.append(
            &execution,
            start_request(&execution, ExpectedVersion::NoStream, "m-1")
        ),
        "an append"
    );

    let mine = MessageId::new(format!("{execution}/outbox-1"));
    let pending = ok!(name, store.pending_outbox(1000), "the outbox");
    assert!(
        pending.iter().any(|row| row.message_id == mine),
        "{name}: the row this decision wrote is not pending"
    );
    for _ in 0..2 {
        ok!(
            name,
            store.mark_published(std::slice::from_ref(&mine), OffsetDateTime::UNIX_EPOCH),
            "marking published"
        );
    }
    assert!(
        !ok!(name, store.pending_outbox(1000), "the outbox")
            .iter()
            .any(|row| row.message_id == mine),
        "{name}: a marked row is still pending"
    );
}

pub async fn a_stream_read_back_replays_to_the_state_it_recorded(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("replay");
    ok!(
        name,
        store.append(
            &execution,
            start_request(&execution, ExpectedVersion::NoStream, "m-1")
        ),
        "an append"
    );

    let slice = ok!(name, store.load(&execution), "a load");
    let state = replay(slice.events());
    let run = state
        .active()
        .unwrap_or_else(|| panic!("{name}: the stream replayed to no execution"));
    assert_eq!(run.state.state_type, StateType::Running, "{name}");
    assert_eq!(run.plan_id(), &plan().plan_id, "{name}");
    assert_eq!(
        run.step("extract").expect("the step").state.state_type,
        StateType::Pending,
        "{name}"
    );
}

pub async fn a_message_too_large_to_store_is_refused(name: &str, store: &dyn WorkflowStore) {
    let execution = fresh("payload");
    let huge = PendingMessage::output(
        WorkflowMessage::Event(WorkflowEvent::StepCompleted {
            step_id: "extract".to_owned(),
            attempt: 1,
            outputs: Vec::new(),
            result: Some(serde_json::Value::String("x".repeat(300 * 1024))),
        }),
        metadata(&execution, "m-big-out"),
    );
    let refused = store
        .append(
            &execution,
            AppendRequest {
                outputs: vec![huge],
                ..start_request(&execution, ExpectedVersion::NoStream, "m-big")
            },
        )
        .await
        .expect_err("a payload past the limit");
    assert!(
        matches!(refused, StoreError::PayloadTooLarge { .. }),
        "{name}: {refused}"
    );
    assert!(
        ok!(name, store.load(&execution), "a load").is_empty(),
        "{name}"
    );
}

/// On PostgreSQL this is `SELECT … FOR UPDATE SKIP LOCKED`; in memory it is the
/// mutex. The suite is what says both mean the same thing.
pub async fn two_claimants_racing_for_one_attempt_produce_one_claim(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("claim-race");
    ok!(
        name,
        store.append(
            &execution,
            dispatch(&execution, "m-1", vec![claimable(&execution)])
        ),
        "a dispatch"
    );

    let filter = mine(&execution);
    let first = ok!(
        name,
        store.claim_attempt(&filter, "worker-a", OffsetDateTime::UNIX_EPOCH),
        "the first claim"
    );
    let second = ok!(
        name,
        store.claim_attempt(&filter, "worker-b", OffsetDateTime::UNIX_EPOCH),
        "the second claim"
    );

    assert!(first.is_some(), "{name}: nobody claimed a claimable row");
    assert!(second.is_none(), "{name}: two claimants took one attempt");
    assert_eq!(
        first.expect("a claim").lease_owner.as_deref(),
        Some("worker-a"),
        "{name}"
    );
}

pub async fn a_claimant_only_takes_what_it_said_it_could_run(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("claim-filter");
    ok!(
        name,
        store.append(
            &execution,
            dispatch(&execution, "m-1", vec![claimable(&execution)])
        ),
        "a dispatch"
    );

    // A reactor listing `python_task` must not take work scheduled for
    // somebody's worker: that would run a registered function in the process
    // holding the object store's credentials.
    assert!(
        ok!(
            name,
            store.claim_attempt(
                &ClaimFilter::for_runtimes(&[RuntimeKind::PythonTask]),
                "reactor",
                OffsetDateTime::UNIX_EPOCH
            ),
            "a reactor's claim"
        )
        .is_none(),
        "{name}: a reactor took a worker's attempt"
    );
    assert!(
        ok!(
            name,
            store.claim_attempt(
                &ClaimFilter::for_queues(&["other".to_owned()]),
                "worker",
                OffsetDateTime::UNIX_EPOCH
            ),
            "another queue's claim"
        )
        .is_none(),
        "{name}: a worker took another queue's attempt"
    );
}

/// A killed pod. The work is picked up again within a poll or two, and the
/// worker that lost it is told at its next boundary.
pub async fn a_lost_claim_expires_and_the_next_claimant_takes_it_over(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("claim-expiry");
    ok!(
        name,
        store.append(
            &execution,
            dispatch(&execution, "m-1", vec![claimable(&execution)])
        ),
        "a dispatch"
    );
    let filter = mine(&execution);
    ok!(
        name,
        store.claim_attempt(&filter, "worker-a", OffsetDateTime::UNIX_EPOCH),
        "the first claim"
    );

    let past =
        OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(aiwatcher_jobs::LEASE_SECONDS + 1);
    let taken_over = ok!(
        name,
        store.claim_attempt(&filter, "worker-b", past),
        "the takeover"
    );
    assert_eq!(
        taken_over.expect("a claim").lease_owner.as_deref(),
        Some("worker-b"),
        "{name}"
    );

    let key = AttemptKey::new(execution.clone(), "extract", 1);
    let row = ok!(name, store.attempt(&key), "the row").expect("the row exists");
    assert!(!row.is_held_by("worker-a", past), "{name}");
    assert!(
        !ok!(name, store.heartbeat(&key, "worker-a", past), "a heartbeat"),
        "{name}: a lost lease was renewed"
    );
}

pub async fn a_heartbeat_keeps_a_long_step_from_being_taken_over(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("heartbeat");
    ok!(
        name,
        store.append(
            &execution,
            dispatch(&execution, "m-1", vec![claimable(&execution)])
        ),
        "a dispatch"
    );
    let filter = mine(&execution);
    ok!(
        name,
        store.claim_attempt(&filter, "worker-a", OffsetDateTime::UNIX_EPOCH),
        "a claim"
    );

    let key = AttemptKey::new(execution.clone(), "extract", 1);
    let half =
        OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(aiwatcher_jobs::LEASE_SECONDS / 2);
    assert!(
        ok!(name, store.heartbeat(&key, "worker-a", half), "a heartbeat"),
        "{name}"
    );

    let past_the_original =
        OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(aiwatcher_jobs::LEASE_SECONDS + 1);
    assert!(
        ok!(
            name,
            store.claim_attempt(&filter, "worker-b", past_the_original),
            "a claim after the original lease"
        )
        .is_none(),
        "{name}: a renewed lease was taken over"
    );
}

pub async fn a_settled_attempt_is_out_of_every_claimants_view(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("claim-settled");
    ok!(
        name,
        store.append(
            &execution,
            dispatch(&execution, "m-1", vec![claimable(&execution)])
        ),
        "a dispatch"
    );
    ok!(
        name,
        store.append(
            &execution,
            dispatch(
                &execution,
                "m-2",
                vec![AttemptRow::settled(
                    AttemptKey::new(execution.clone(), "extract", 1),
                    RuntimeKind::PythonTask,
                    StateType::Completed,
                )],
            ),
        ),
        "a completion"
    );

    assert!(
        ok!(
            name,
            store.claim_attempt(&mine(&execution), "worker", OffsetDateTime::UNIX_EPOCH),
            "a claim"
        )
        .is_none(),
        "{name}: a finished attempt was claimed again"
    );
}

pub async fn a_retry_is_not_claimable_before_its_delay(name: &str, store: &dyn WorkflowStore) {
    let execution = fresh("claim-delay");
    let later = OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(30);
    ok!(
        name,
        store.append(
            &execution,
            dispatch(
                &execution,
                "m-1",
                vec![claimable(&execution).not_before(later)]
            )
        ),
        "a dispatch"
    );

    let filter = mine(&execution);
    assert!(
        ok!(
            name,
            store.claim_attempt(&filter, "worker", OffsetDateTime::UNIX_EPOCH),
            "an early claim"
        )
        .is_none(),
        "{name}: a backoff was ignored"
    );
    assert!(
        ok!(
            name,
            store.claim_attempt(&filter, "worker", later),
            "a claim"
        )
        .is_some(),
        "{name}"
    );
}

/// The one case a decision cannot cover: an input the inbox says was already
/// handled. Leaving the cursor behind would re-read it forever.
pub async fn a_cursor_advances_on_its_own_for_a_message_the_inbox_knew(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let processor = format!(
        "contract-{}",
        OffsetDateTime::now_utc().unix_timestamp_nanos()
    );
    assert!(
        ok!(name, store.checkpoint(&processor), "an unset checkpoint").is_none(),
        "{name}"
    );
    ok!(
        name,
        store.advance_checkpoint(&processor, Checkpoint::from_global_position(41)),
        "advancing a cursor"
    );
    assert_eq!(
        ok!(name, store.checkpoint(&processor), "a checkpoint"),
        Some(Checkpoint::from_global_position(41)),
        "{name}"
    );
}

/// Rewriting the same result must produce the same rows.
///
/// Not part of [`assert_contract`] because it needs a store nobody else is
/// using; the PostgreSQL adapter's own test calls it.
pub async fn appending_the_same_decision_twice_is_idempotent(
    name: &str,
    store: &dyn WorkflowStore,
) -> Result<()> {
    let execution = fresh("idempotent");
    let first = store
        .append(
            &execution,
            start_request(&execution, ExpectedVersion::NoStream, "m-1"),
        )
        .await?;
    let again = store
        .append(
            &execution,
            start_request(&execution, ExpectedVersion::NoStream, "m-1"),
        )
        .await?;
    assert!(again.is_duplicate(), "{name}");
    // The two report different versions on purpose: an append reports the new
    // head, and a duplicate reports where the *input* landed — the version its
    // previous result is looked up at, which stays true as the stream grows.
    assert_eq!(again.version(), 1, "{name}");
    assert_eq!(
        store.load(&execution).await?.version,
        first.version(),
        "{name}: the second append wrote something"
    );
    Ok(())
}
