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

use crate::claim::{AttemptKey, AttemptRow, AttemptWrite, ClaimFilter};
use crate::decide::{Now, decide, replay};
use crate::message::{
    MessageMetadata, OutboxMessage, PendingMessage, RunProjection, SCHEMA_VERSION, WorkflowCommand,
    WorkflowEvent, WorkflowMessage,
};
use crate::plan::{
    CachePolicy, DefinitionKind, DefinitionRevision, ExecutionPlan, PlanStep, PythonTaskSpec,
    RetryPolicy, RuntimeBinding, RuntimeKind,
};
use crate::schedule::rule::OverlapPolicy;
use crate::schedule::slot::{
    SlotAdmission, SlotAdmissionRequest, SlotKey, SlotOutcome, SlotSettlement,
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
    .on_queue(queue_of(execution), STAGE_TASK.to_owned())
}

/// A filter that sees only this execution's rows.
fn mine(execution: &ExecutionId) -> ClaimFilter {
    ClaimFilter::for_queues(&[queue_of(execution)], &[STAGE_TASK.to_owned()])
}

/// The `name@version` every fixture row pins.
const STAGE_TASK: &str = "stage@1";

/// An append whose only content is attempt rows.
fn dispatch(execution: &ExecutionId, message_id: &str, rows: Vec<AttemptWrite>) -> AppendRequest {
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

/// An append that puts one execution into a terminal state.
///
/// The projection is what a sweep reads, so this is the smallest thing that
/// makes a run finished from the store's point of view — the stream behind it
/// is the same start, and no property here is about how it got there.
fn finish(execution: &ExecutionId, message_id: &str) -> AppendRequest {
    AppendRequest {
        expected_version: ExpectedVersion::Any,
        input: PendingMessage::input(
            WorkflowMessage::Command(WorkflowCommand::PauseExecution),
            metadata(execution, message_id),
        ),
        outputs: Vec::new(),
        projection: projection(execution, 1, StateType::Completed),
        outbox: Vec::new(),
        checkpoint: None,
        attempts: Vec::new(),
    }
}

/// A cutoff every write so far is before. The sweep's own clock, moved rather
/// than the store's — nothing here may sleep for a retention window.
fn well_after_everything() -> OffsetDateTime {
    OffsetDateTime::now_utc() + time::Duration::hours(1)
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
    a_published_outbox_row_is_dropped_rather_than_kept(name, store).await;
    a_stream_read_back_replays_to_the_state_it_recorded(name, store).await;
    a_message_too_large_to_store_is_refused(name, store).await;
    two_claimants_racing_for_one_attempt_produce_one_claim(name, store).await;
    a_claimant_only_takes_what_it_said_it_could_run(name, store).await;
    a_lost_claim_expires_and_the_next_claimant_takes_it_over(name, store).await;
    a_heartbeat_keeps_a_long_step_from_being_taken_over(name, store).await;
    a_finished_attempt_leaves_no_row_behind(name, store).await;
    a_retry_is_not_claimable_before_its_delay(name, store).await;
    a_cursor_advances_on_its_own_for_a_message_the_inbox_knew(name, store).await;
    a_finished_execution_is_forgotten_and_a_running_one_is_not(name, store).await;
    an_execution_the_outbox_still_speaks_for_is_kept(name, store).await;
    two_replicas_that_both_find_a_slot_due_admit_one(name, store).await;
    a_settled_slot_is_never_admitted_again(name, store).await;
    a_slot_put_back_after_a_transient_failure_is_due_again(name, store).await;
    a_slot_whose_holder_vanished_is_taken_over_when_the_lease_expires(name, store).await;
    a_definition_with_a_run_that_has_not_finished_blocks_its_next_slot(name, store).await;
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
        "{name}: a published row is still pending"
    );
}

/// A published row is *gone*, not flagged.
///
/// The fact is on the event log once the sink has taken it, so a copy here
/// would answer no question — and it would grow with every step of every run
/// for as long as the deployment lives. This is the property that says the
/// outbox is a queue rather than a second history.
pub async fn a_published_outbox_row_is_dropped_rather_than_kept(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("outbox-drop");
    ok!(
        name,
        store.append(
            &execution,
            start_request(&execution, ExpectedVersion::NoStream, "m-1")
        ),
        "an append"
    );

    let mine = MessageId::new(format!("{execution}/outbox-1"));
    let before = ok!(name, store.pending_outbox(1000), "the outbox").len();
    ok!(
        name,
        store.mark_published(std::slice::from_ref(&mine), OffsetDateTime::UNIX_EPOCH),
        "publishing"
    );

    let after = ok!(name, store.pending_outbox(1000), "the outbox");
    assert_eq!(
        after.len(),
        before - 1,
        "{name}: publishing one row left the outbox the same size"
    );
    assert!(
        after.iter().all(|row| row.published_at.is_none()),
        "{name}: a row this store still holds has been published, which means it kept it"
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
            dispatch(
                &execution,
                "m-1",
                vec![AttemptWrite::Dispatch(claimable(&execution))]
            )
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
            dispatch(
                &execution,
                "m-1",
                vec![AttemptWrite::Dispatch(claimable(&execution))]
            )
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
                &ClaimFilter::for_queues(&["other".to_owned()], &[STAGE_TASK.to_owned()]),
                "worker",
                OffsetDateTime::UNIX_EPOCH
            ),
            "another queue's claim"
        )
        .is_none(),
        "{name}: a worker took another queue's attempt"
    );
    assert!(
        ok!(
            name,
            store.claim_attempt(
                &ClaimFilter::for_queues(&[queue_of(&execution)], &["stage@2".to_owned()]),
                "worker",
                OffsetDateTime::UNIX_EPOCH
            ),
            "another version's claim"
        )
        .is_none(),
        "{name}: a worker took an attempt pinning code it does not have"
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
            dispatch(
                &execution,
                "m-1",
                vec![AttemptWrite::Dispatch(claimable(&execution))]
            )
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
            dispatch(
                &execution,
                "m-1",
                vec![AttemptWrite::Dispatch(claimable(&execution))]
            )
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

/// A finished attempt is retired, not stored in a terminal state.
///
/// Two assertions, and the second is the one that costs something to break: a
/// terminal row is invisible to a claimant either way, so keeping one looks
/// correct until the table is the size of the history. Section 43.34.
pub async fn a_finished_attempt_leaves_no_row_behind(name: &str, store: &dyn WorkflowStore) {
    let execution = fresh("claim-settled");
    ok!(
        name,
        store.append(
            &execution,
            dispatch(
                &execution,
                "m-1",
                vec![AttemptWrite::Dispatch(claimable(&execution))]
            )
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
                vec![AttemptWrite::Retire(AttemptKey::new(
                    execution.clone(),
                    "extract",
                    1,
                ))],
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
    assert!(
        ok!(
            name,
            store.attempt(&AttemptKey::new(execution.clone(), "extract", 1)),
            "reading the finished attempt"
        )
        .is_none(),
        "{name}: a finished attempt was kept as a row, so the claim table grows with the history"
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
                vec![AttemptWrite::Dispatch(
                    claimable(&execution).not_before(later)
                )]
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
/// Retention forgets a run that finished and keeps one that has not.
///
/// The second half is the point. A sweep that deleted by age alone would delete
/// the run somebody is watching, and nothing in this system decides a run has
/// died — a producer may have been killed or may be thinking for twenty
/// minutes, and age tells the two apart in neither direction.
///
/// The attempt rows go with it, which is the cost that was actually growing:
/// the claim table is what every claimant scans.
pub async fn a_finished_execution_is_forgotten_and_a_running_one_is_not(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let finished = fresh("pruned");
    let running = fresh("kept");
    for execution in [&finished, &running] {
        ok!(
            name,
            store.append(
                execution,
                start_request(execution, ExpectedVersion::NoStream, "m-1")
            ),
            "a start"
        );
        // A start leaves an outbox row, and an unpublished row keeps its
        // execution out of every sweep. That is the *next* property; this one
        // is about the state, so the row is drained first.
        ok!(
            name,
            store.mark_published(
                &[MessageId::new(format!("{execution}/outbox-1"))],
                OffsetDateTime::now_utc()
            ),
            "publishing the start's fact"
        );
    }
    ok!(
        name,
        store.append(
            &finished,
            dispatch(
                &finished,
                "m-2",
                vec![AttemptWrite::Dispatch(claimable(&finished))],
            )
        ),
        "a dispatch"
    );
    ok!(
        name,
        store.append(&finished, finish(&finished, "m-3")),
        "an end"
    );

    let pruned = ok!(
        name,
        store.prune(well_after_everything(), 100),
        "a retention sweep"
    );
    assert!(pruned.executions >= 1, "{name}: swept nothing");
    assert!(pruned.attempts >= 1, "{name}: kept the claim rows");

    assert!(
        ok!(
            name,
            store.projection(&finished),
            "reading the finished run"
        )
        .is_none(),
        "{name}: a finished execution survived its retention window"
    );
    assert!(
        ok!(name, store.load(&finished), "loading the finished run")
            .events()
            .next()
            .is_none(),
        "{name}: the stream outlived the projection that indexed it"
    );
    assert!(
        ok!(
            name,
            store.attempt(&AttemptKey::new(finished.clone(), "extract", 1)),
            "reading the attempt row"
        )
        .is_none(),
        "{name}: an attempt row outlived the execution that authorised it"
    );
    assert!(
        ok!(name, store.projection(&running), "reading the running run").is_some(),
        "{name}: a running execution was deleted for being old"
    );
}

/// A fact that has not reached the log keeps its explanation alive.
///
/// [`aiwatcher_jobs::ORDERING`] in a sixth place: the durable copy first, and
/// only then the thing it was derived from. Deleting the other way round leaves
/// the publisher a message with no decision behind it and the log a gap nothing
/// records.
pub async fn an_execution_the_outbox_still_speaks_for_is_kept(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("outbox-retention");
    ok!(
        name,
        store.append(
            &execution,
            start_request(&execution, ExpectedVersion::NoStream, "m-1")
        ),
        "a start"
    );
    ok!(
        name,
        store.append(&execution, finish(&execution, "m-2")),
        "an end"
    );

    let kept = ok!(
        name,
        store.prune(well_after_everything(), 100),
        "a sweep with the fact still pending"
    );
    let _ = kept;
    assert!(
        ok!(name, store.projection(&execution), "reading it back").is_some(),
        "{name}: forgot an execution whose fact had not reached the log"
    );

    ok!(
        name,
        store.mark_published(
            &[MessageId::new(format!("{execution}/outbox-1"))],
            OffsetDateTime::now_utc()
        ),
        "publishing"
    );
    ok!(
        name,
        store.prune(well_after_everything(), 100),
        "a sweep afterwards"
    );
    assert!(
        ok!(name, store.projection(&execution), "reading it back").is_none(),
        "{name}: kept an execution whose fact is on the log"
    );
}

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

/// Two replicas both find one slot due, and one of them starts it.
///
/// The derived execution id already makes the *start* idempotent, so this is
/// not about two runs of one slot — it is about the decision. Exactly one
/// caller is admitted, and the other is told the slot is held rather than
/// given a second lease on it.
pub async fn two_replicas_that_both_find_a_slot_due_admit_one(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let definition = format!("{name}-two-replicas-{}", stamp());
    let slot = OffsetDateTime::UNIX_EPOCH + time::Duration::days(1);
    let key = SlotKey::new(DefinitionKind::CurationPipeline, definition, slot);

    let first = ok!(
        name,
        store.admit_slot(&admission(&key, "a", slot)),
        "the first"
    );
    let second = ok!(
        name,
        store.admit_slot(&admission(&key, "b", slot)),
        "the second"
    );

    assert_eq!(first, SlotAdmission::Admitted, "{name}");
    assert!(
        matches!(second, SlotAdmission::Held { .. }),
        "{name}: the second replica was given the slot too: {second:?}"
    );
}

/// A settled slot is never taken again — by anybody, ever.
///
/// This is what makes a replay of an interval free: the tick re-derives the
/// same slots after a crash, and every one of them comes back settled.
pub async fn a_settled_slot_is_never_admitted_again(name: &str, store: &dyn WorkflowStore) {
    let definition = format!("{name}-settled-{}", stamp());
    let slot = OffsetDateTime::UNIX_EPOCH + time::Duration::days(2);
    let key = SlotKey::new(DefinitionKind::CurationPipeline, definition, slot);

    ok!(
        name,
        store.admit_slot(&admission(&key, "a", slot)),
        "a claim"
    );
    ok!(
        name,
        store.settle_slot(
            &key,
            "a",
            SlotSettlement::Started {
                execution_id: "run-1".to_owned(),
            },
            slot,
        ),
        "a settlement"
    );

    // Long after the lease would have expired, which is the point: a settled
    // slot is not an expired claim.
    let later = slot + time::Duration::seconds(aiwatcher_jobs::LEASE_SECONDS * 10);
    let again = ok!(
        name,
        store.admit_slot(&admission(&key, "b", later)),
        "a replay"
    );
    assert_eq!(
        again,
        SlotAdmission::Settled {
            outcome: SlotOutcome::Started
        },
        "{name}"
    );
}

/// A transient failure leaves the slot due rather than consuming it.
///
/// Review R2. The release before this one wrote every failure down as
/// `refused` and then moved the global cursor past the slot, so a store that
/// was unreachable for ten seconds at 09:00 cost the day's run and left a note
/// saying it had been refused.
pub async fn a_slot_put_back_after_a_transient_failure_is_due_again(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let definition = format!("{name}-try-again-{}", stamp());
    let slot = OffsetDateTime::UNIX_EPOCH + time::Duration::days(3);
    let key = SlotKey::new(DefinitionKind::CurationPipeline, definition, slot);

    ok!(
        name,
        store.admit_slot(&admission(&key, "a", slot)),
        "a claim"
    );
    ok!(
        name,
        store.settle_slot(
            &key,
            "a",
            SlotSettlement::TryAgain {
                detail: "the object store was unreachable".to_owned(),
            },
            slot,
        ),
        "a release"
    );

    // Immediately, not after the lease: nothing holds it.
    let again = ok!(
        name,
        store.admit_slot(&admission(&key, "b", slot)),
        "the next tick"
    );
    assert_eq!(
        again,
        SlotAdmission::Admitted,
        "{name}: a transient failure consumed the slot"
    );
}

/// A tick that died holding a slot does not keep it.
pub async fn a_slot_whose_holder_vanished_is_taken_over_when_the_lease_expires(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let definition = format!("{name}-expired-{}", stamp());
    let slot = OffsetDateTime::UNIX_EPOCH + time::Duration::days(4);
    let key = SlotKey::new(DefinitionKind::CurationPipeline, definition, slot);

    ok!(
        name,
        store.admit_slot(&admission(&key, "dead", slot)),
        "a claim"
    );
    let stale = slot + time::Duration::seconds(aiwatcher_jobs::LEASE_SECONDS + 1);
    let taken = ok!(
        name,
        store.admit_slot(&admission(&key, "alive", stale)),
        "a takeover"
    );
    assert_eq!(taken, SlotAdmission::Admitted, "{name}");

    // And the process that died no longer speaks for it.
    ok!(
        name,
        store.settle_slot(
            &key,
            "dead",
            SlotSettlement::Refused {
                detail: "from the grave".to_owned(),
            },
            stale,
        ),
        "a settlement from the previous holder"
    );
    let firings = ok!(
        name,
        store.recent_slots(key.definition_kind, &key.definition_name, 10),
        "the firings"
    );
    assert_eq!(
        firings.first().and_then(|record| record.outcome),
        None,
        "{name}: a caller whose lease was taken over settled the slot anyway"
    );
}

/// `overlap = skip` is answered from this store, in the transaction that takes
/// the slot.
///
/// Review R1, and the property the previous implementation could not have: it
/// asked an asynchronous read model that is **empty in the role the tick runs
/// in**, so skip never skipped there. `allow` is checked in the same property,
/// because the two answers have to come from one place to be worth anything.
pub async fn a_definition_with_a_run_that_has_not_finished_blocks_its_next_slot(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("overlap");
    ok!(
        name,
        store.append(
            &execution,
            start_request(&execution, ExpectedVersion::NoStream, "overlap-1")
        ),
        "a run that is going"
    );
    let projection = ok!(name, store.projection(&execution), "the projection")
        .expect("a projection the append wrote");
    assert!(
        !projection.state.state_type.is_terminal(),
        "{name}: this property needs a run that has not finished"
    );

    // The definition name comes from the fixture and is shared, so the *slot*
    // is what makes this property's key its own. Against PostgreSQL these run
    // over a database other runs also used — which is the point of running
    // them there — and a fixed slot found the previous run's unsettled row
    // instead of the overlap it was asking about.
    let slot = OffsetDateTime::now_utc();
    let key = SlotKey::new(
        DefinitionKind::CurationPipeline,
        projection.definition_name.clone(),
        slot,
    );

    let blocked = ok!(
        name,
        store.admit_slot(&admission(&key, "a", slot)),
        "a skip"
    );
    let SlotAdmission::Blocked { execution_id } = blocked else {
        panic!("{name}: the run that is still going did not block the slot: {blocked:?}");
    };
    // *Which* run blocks it is not the property — every fixture in this suite
    // shares one definition name, so the answer is whichever of them is still
    // going. What is the property is that the store named a real one: a
    // blocking id nothing backs would be an answer invented rather than read,
    // which is exactly what the read model was giving before (review R1).
    let blocker = ok!(
        name,
        store.projection(&ExecutionId::new(execution_id.clone())),
        "the blocking run"
    )
    .unwrap_or_else(|| panic!("{name}: blocked by {execution_id}, which has no projection"));
    assert!(
        !blocker.state.state_type.is_terminal(),
        "{name}: blocked by {execution_id}, which has finished"
    );
    assert_eq!(
        blocker.definition_name, projection.definition_name,
        "{name}: blocked by a run of a different definition"
    );

    // The other half: the same state, the other policy.
    let allowed = ok!(
        name,
        store.admit_slot(&SlotAdmissionRequest {
            overlap: OverlapPolicy::Allow,
            ..admission(&key, "a", slot)
        }),
        "an allow"
    );
    assert_eq!(
        allowed,
        SlotAdmission::Admitted,
        "{name}: `allow` was blocked by a run that is still going"
    );
}

fn admission(key: &SlotKey, owner: &str, now: OffsetDateTime) -> SlotAdmissionRequest {
    SlotAdmissionRequest {
        key: key.clone(),
        owner: owner.to_owned(),
        overlap: OverlapPolicy::Skip,
        now,
    }
}

fn stamp() -> i128 {
    OffsetDateTime::now_utc().unix_timestamp_nanos()
}
