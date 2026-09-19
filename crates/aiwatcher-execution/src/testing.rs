// Assertions, in a module that only a test compiles. `clippy.toml`'s
// allowances reach `#[cfg(test)]` modules and this is not one — it is behind a
// feature so a *second crate's* tests can call it, which is the whole point.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! The storage contract, written once so every adapter proves the same thing.
//!
//! An adapter that passes a *different* suite is an adapter that is correct
//! about something else — and the two that ship here are correct in different
//! ways by construction, since one holds everything under a mutex and the
//! other under a transaction. That is exactly why the properties live here and
//! not beside either of them.
//!
//! Each property takes a store and a name, and every one of them mints its own
//! execution id. That matters for a shared PostgreSQL, where the suite runs
//! against a database somebody else's run also used: a property that reused a
//! fixed id would pass alone and fail in CI.
//!
//! Behind the `testing` feature, so none of this reaches a production build.

use std::collections::BTreeMap;

use time::{Duration, OffsetDateTime};

use aiwatcher_core::{CausationId, Checkpoint, CorrelationId, MessageId};

use crate::claim::{AttemptKey, AttemptRow, AttemptWrite, ClaimFilter};
use crate::decide::{Now, decide, replay};
use crate::hosted::{LeaseOutcome, Timer, TimerWrite};
use crate::message::HostedMessage;
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
use crate::scope::{ExecutionOwnership, ExecutionScope, ProjectStart};
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
        payloads: crate::message::PayloadPolicy::External,
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
        payloads: Default::default(),
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
        timers: Vec::new(),
        attempts: Vec::new(),
        ownership: None,
    }
}

/// One timer, as a worker would have composed it.
fn timer(execution: &ExecutionId, timer_id: &str, due_at: OffsetDateTime) -> Timer {
    Timer {
        execution: execution.clone(),
        timer_id: timer_id.to_owned(),
        due_at,
        // `agentic`'s own name for what a saga's timeout produces. The point of
        // storing the whole message is that this string is the worker's and
        // never this engine's.
        message: HostedMessage {
            message_type: "saga.timeout_fired".to_owned(),
            metadata: serde_json::json!({ "timeout_id": timer_id }),
            payload: None,
        },
    }
}

/// An append that only touches the timer table.
fn timer_request(
    execution: &ExecutionId,
    message_id: &str,
    timers: Vec<TimerWrite>,
) -> AppendRequest {
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
        timers,
        attempts: Vec::new(),
        ownership: None,
    }
}

/// An append that records one attempt's terminal outcome.
fn outcome_request(
    execution: &ExecutionId,
    version: u64,
    message_id: &str,
    step_id: &str,
    attempt: u32,
) -> AppendRequest {
    AppendRequest {
        expected_version: ExpectedVersion::Exact(version),
        input: PendingMessage::input(
            WorkflowMessage::Command(WorkflowCommand::PauseExecution),
            metadata(execution, message_id),
        ),
        outputs: vec![PendingMessage::output(
            WorkflowMessage::Event(WorkflowEvent::StepCompleted {
                step_id: step_id.to_owned(),
                attempt,
                outputs: Vec::new(),
                result: None,
            }),
            metadata(execution, &format!("{message_id}-out")),
        )],
        projection: projection(execution, version + 2, StateType::Running),
        outbox: Vec::new(),
        checkpoint: None,
        timers: Vec::new(),
        attempts: Vec::new(),
        ownership: None,
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
        timers: Vec::new(),
        attempts: rows,
        ownership: None,
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
        timers: Vec::new(),
        attempts: Vec::new(),
        ownership: None,
    }
}

/// A project nothing else in this suite uses.
///
/// Fresh per call, for [`fresh`]'s reason: these properties run against a
/// PostgreSQL somebody else's run also used, and a fixed scope would make one
/// property's isolation depend on another's leftovers.
#[must_use]
pub fn project() -> aiwatcher_iam::ProjectScope {
    aiwatcher_iam::ProjectScope {
        organization: aiwatcher_iam::OrganizationId::new(),
        project: aiwatcher_iam::ProjectId::new(),
    }
}

/// The record a start in `scope` establishes, for a caller outside this module.
///
/// The adapter-specific tests beside the suite need the same fixture the
/// properties use — an adapter proving "the record survives a restart" against
/// a record of its own would be proving it about something else.
#[must_use]
pub fn ownership_for(scope: aiwatcher_iam::ProjectScope, subject: &str) -> ExecutionOwnership {
    owner(scope, subject)
}

/// A start that creates a project execution, for the same caller.
#[must_use]
pub fn owned_start_for(
    execution: &ExecutionId,
    message_id: &str,
    owner: ExecutionOwnership,
) -> AppendRequest {
    owned_start(execution, message_id, owner)
}

/// The record a start in `scope` establishes.
fn owner(scope: aiwatcher_iam::ProjectScope, subject: &str) -> ExecutionOwnership {
    ExecutionOwnership::of(
        &ProjectStart::new(
            scope,
            aiwatcher_iam::Principal::new("https://id.example", subject).expect("a principal"),
        ),
        &plan(),
    )
}

/// A start that creates a project execution, owner and all.
fn owned_start(
    execution: &ExecutionId,
    message_id: &str,
    owner: ExecutionOwnership,
) -> AppendRequest {
    AppendRequest {
        ownership: Some(owner),
        ..start_request(execution, ExpectedVersion::NoStream, message_id)
    }
}

/// The same store, bound to `scope`.
fn bound(
    name: &str,
    store: &dyn WorkflowStore,
    scope: aiwatcher_iam::ProjectScope,
) -> std::sync::Arc<dyn WorkflowStore> {
    match store.for_project(scope) {
        Ok(bound) => bound,
        Err(error) => panic!("{name}: binding a store to a project: {error}"),
    }
}

/// Whether a refusal is the scope boundary saying no.
fn is_out_of_scope(error: &StoreError) -> bool {
    matches!(error, StoreError::OutOfScope(_))
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
    paging_a_stream_reaches_every_message_loading_it_whole_does(name, store).await;
    a_hosted_message_survives_the_store_it_was_written_to(name, store).await;
    one_decider_at_a_time_holds_a_hosted_run(name, store).await;
    a_decider_that_stopped_renewing_is_taken_over_and_the_takeover_says_whose(name, store).await;
    a_released_lease_is_free_before_it_would_have_run_out(name, store).await;
    a_timer_is_due_when_its_time_comes_and_not_before(name, store).await;
    a_fired_timer_leaves_no_row_to_fire_again(name, store).await;
    one_attempt_s_outcome_is_found_without_reading_its_neighbours(name, store).await;
    a_message_too_large_to_store_is_refused(name, store).await;
    two_claimants_racing_for_one_attempt_produce_one_claim(name, store).await;
    a_claimant_only_takes_what_it_said_it_could_run(name, store).await;
    an_exact_attempt_filter_never_claims_a_neighbour(name, store).await;
    a_pods_attempt_is_read_by_its_runtime_and_taken_only_by_its_key(name, store).await;
    a_lost_claim_expires_and_the_next_claimant_takes_it_over(name, store).await;
    a_heartbeat_keeps_a_long_step_from_being_taken_over(name, store).await;
    a_lease_exactly_its_length_old_is_still_held_and_one_second_later_is_not(name, store).await;
    a_finished_attempt_leaves_no_row_behind(name, store).await;
    a_parked_attempt_keeps_its_row_and_loses_its_lease(name, store).await;
    a_retry_is_not_claimable_before_its_delay(name, store).await;
    an_attempt_waiting_for_a_reactor_is_counted_by_its_runtime_and_left_alone(name, store).await;
    a_cursor_advances_on_its_own_for_a_message_the_inbox_knew(name, store).await;
    a_finished_execution_is_forgotten_and_a_running_one_is_not(name, store).await;
    an_execution_the_outbox_still_speaks_for_is_kept(name, store).await;
    two_replicas_that_both_find_a_slot_due_admit_one(name, store).await;
    a_settled_slot_is_never_admitted_again(name, store).await;
    a_slot_put_back_after_a_transient_failure_is_due_again(name, store).await;
    a_slot_whose_holder_vanished_is_taken_over_when_the_lease_expires(name, store).await;
    a_definition_with_a_run_that_has_not_finished_blocks_its_next_slot(name, store).await;
    an_execution_is_owned_by_the_project_that_started_it_and_reads_back_that_way(name, store).await;
    an_unscoped_path_reaches_nothing_about_a_project_s_execution(name, store).await;
    a_project_reaches_nothing_about_another_project_s_execution(name, store).await;
    a_project_may_not_adopt_an_execution_nobody_owns(name, store).await;
    a_repeated_start_naming_another_owner_does_not_repoint_the_execution(name, store).await;
    the_same_start_twice_in_one_project_is_one_execution_and_one_owner(name, store).await;
    a_global_claimant_never_takes_a_project_s_attempt(name, store).await;
    a_project_claimant_never_takes_a_global_attempt(name, store).await;
    a_project_s_timers_and_outbox_rows_reach_only_its_own_side(name, store).await;
    a_retention_sweep_forgets_its_own_side_and_leaves_the_other_alone(name, store).await;
    a_store_binds_to_one_project_and_refuses_a_second(name, store).await;
    what_is_instance_wide_is_refused_by_name_rather_than_answered(name, store).await;
    the_projects_that_have_run_here_are_listed_as_scopes_and_never_as_rows(name, store).await;
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
                timers: Vec::new(),
                attempts: Vec::new(),
                ownership: None,
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

/// A page is a window on the same stream, not a second answer about it.
///
/// Written against the port rather than against one adapter, because the three
/// reach it three different ways — a slice, a file read whole, and a `limit`
/// with its own `max(stream_version)` query — and an adapter that paged
/// *differently* would be correct about something else. The version is checked
/// on every page for the same reason it is queried separately in `postgres`: it
/// is what tells a reader a short page is the last one rather than a truncated
/// one.
pub async fn paging_a_stream_reaches_every_message_loading_it_whole_does(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("paging");
    ok!(
        name,
        store.append(
            &execution,
            start_request(&execution, ExpectedVersion::NoStream, "m-1")
        ),
        "an append"
    );

    let whole = ok!(name, store.load(&execution), "a load");
    assert!(
        whole.messages.len() > 1,
        "{name}: this property needs a stream worth paging"
    );

    // One at a time, which is the size that finds an off-by-one in `after`.
    let mut walked = Vec::new();
    let mut after = 0;
    loop {
        let page = ok!(name, store.load_page(&execution, after, 1), "a page");
        assert_eq!(
            page.version, whole.version,
            "{name}: a page reports the stream's version, not its own end"
        );
        let Some(last) = page.messages.last() else {
            break;
        };
        assert!(page.messages.len() <= 1, "{name}: a page honours its limit");
        after = last.stream_version;
        walked.extend(page.messages);
    }
    assert_eq!(walked, whole.messages, "{name}: paged and whole agree");

    // A limit past the end is the ordinary case for a short stream, and asking
    // from where the stream got to answers nothing rather than the first page
    // again.
    let all_at_once = ok!(name, store.load_page(&execution, 0, 1_000), "one big page");
    assert_eq!(all_at_once.messages, whole.messages, "{name}");
    let past_the_end = ok!(
        name,
        store.load_page(&execution, whole.version, 10),
        "a page after the end"
    );
    assert!(
        past_the_end.messages.is_empty(),
        "{name}: nothing follows the last message"
    );
    assert_eq!(past_the_end.version, whole.version, "{name}");
}

/// A worker's own message is a third kind, and every adapter has to hold one.
///
/// The one that could fail alone is `postgres`, whose `workflow_messages.kind`
/// carries a check constraint — `0007` widens it, and a build that added the
/// arm without the migration would pass every other property here and refuse
/// the first real append. Which is why this is in the shared suite rather than
/// beside that adapter: the *rule* is that a hosted message round-trips, and an
/// adapter proving it about itself proves something narrower.
pub async fn a_hosted_message_survives_the_store_it_was_written_to(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("hosted");
    ok!(
        name,
        store.append(
            &execution,
            start_request(&execution, ExpectedVersion::NoStream, "m-1")
        ),
        "a start"
    );

    let written = crate::message::HostedMessage {
        message_type: "TurnCompleted".to_owned(),
        metadata: serde_json::json!({ "node": "summarizer", "attempt": 2 }),
        payload: Some(crate::message::PayloadRef {
            reference: "agentic://turn/4".to_owned(),
            digest: "c".repeat(64),
            size: 900,
            policy: crate::message::PayloadPolicy::Sealed,
        }),
    };
    let before = ok!(name, store.load(&execution), "a load").version;
    ok!(
        name,
        store.append(
            &execution,
            AppendRequest {
                expected_version: ExpectedVersion::Exact(before),
                input: PendingMessage::input(
                    WorkflowMessage::Hosted(crate::message::HostedMessage {
                        message_type: crate::hosted::HOSTED_APPEND.to_owned(),
                        metadata: serde_json::json!({ "messages": 1 }),
                        payload: None,
                    }),
                    metadata(&execution, "append-1"),
                ),
                outputs: vec![PendingMessage::output(
                    WorkflowMessage::Hosted(written.clone()),
                    metadata(&execution, "hosted-1"),
                )],
                projection: projection(&execution, before + 2, StateType::Running),
                outbox: Vec::new(),
                checkpoint: None,
                timers: Vec::new(),
                attempts: Vec::new(),
                ownership: None,
            }
        ),
        "a hosted append"
    );

    let slice = ok!(name, store.load(&execution), "a reload");
    let read = slice
        .messages
        .iter()
        .filter_map(|recorded| recorded.message.hosted())
        .find(|hosted| hosted.message_type == "TurnCompleted")
        .unwrap_or_else(|| panic!("{name}: the hosted message did not come back"));
    assert_eq!(read, &written, "{name}");

    // And it stayed out of the fold: the run is where the start left it.
    let state = replay(slice.events());
    let run = state
        .active()
        .unwrap_or_else(|| panic!("{name}: the stream replayed to no execution"));
    assert_eq!(run.state.state_type, StateType::Running, "{name}");
}

/// One decider at a time, and the refusal says who has it.
///
/// `ProcessorLock`'s semantics, proved against the port. It is deliberately not
/// proved against one adapter: `memory` holds a map, `file` reads a file under
/// its own gate and `postgres` settles it inside a single upsert whose `where`
/// is the whole rule, and the only thing that makes those one design is a suite
/// all three answer.
pub async fn one_decider_at_a_time_holds_a_hosted_run(name: &str, store: &dyn WorkflowStore) {
    let execution = fresh("lease");
    let start = OffsetDateTime::UNIX_EPOCH;

    let taken = ok!(
        name,
        store.take_decider_lease(&execution, "worker-a", start),
        "a first claim"
    );
    let lease = taken
        .taken()
        .unwrap_or_else(|| panic!("{name}: the first claim was refused"));
    assert_eq!(lease.holder, "worker-a", "{name}");
    assert_eq!(
        lease.previous_holder, None,
        "{name}: nobody was interrupted"
    );

    // A second decider, while the first still holds it.
    let refused = ok!(
        name,
        store.take_decider_lease(&execution, "worker-b", start + Duration::seconds(1)),
        "a second claim"
    );
    match refused {
        LeaseOutcome::Held { holder, expires_at } => {
            assert_eq!(holder, "worker-a", "{name}");
            assert_eq!(
                expires_at,
                start + Duration::seconds(aiwatcher_jobs::LEASE_SECONDS),
                "{name}: the refusal says when it is worth asking again"
            );
        }
        LeaseOutcome::Taken(_) => panic!("{name}: two deciders held one run"),
    }

    // The holder renewing is the same call, and it moves the clock rather than
    // reading as a takeover of itself.
    let renewed = ok!(
        name,
        store.take_decider_lease(&execution, "worker-a", start + Duration::seconds(200)),
        "a renewal"
    );
    let renewed = renewed
        .taken()
        .unwrap_or_else(|| panic!("{name}: the holder could not renew"));
    assert_eq!(renewed.claimed_at, start + Duration::seconds(200), "{name}");
    assert_eq!(
        renewed.previous_holder, None,
        "{name}: a heartbeat is not a takeover"
    );

    // And a reader gets the same answer the claimants did.
    let read = ok!(name, store.decider_lease(&execution), "a read")
        .unwrap_or_else(|| panic!("{name}: the lease was not there"));
    assert_eq!(read.holder, "worker-a", "{name}");
}

/// A worker that stopped renewing loses it, and its replacement is told whose
/// it was.
pub async fn a_decider_that_stopped_renewing_is_taken_over_and_the_takeover_says_whose(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("takeover");
    let start = OffsetDateTime::UNIX_EPOCH;
    let after = start + Duration::seconds(aiwatcher_jobs::LEASE_SECONDS + 1);

    ok!(
        name,
        store.take_decider_lease(&execution, "worker-a", start),
        "a first claim"
    );
    let taken = ok!(
        name,
        store.take_decider_lease(&execution, "worker-b", after),
        "a takeover"
    );
    let lease = taken
        .taken()
        .unwrap_or_else(|| panic!("{name}: an expired lease was not taken over"));
    assert_eq!(lease.holder, "worker-b", "{name}");
    // The signal a takeover needs, and it cannot be read off `holder`: that
    // column already names the replacement. `AttemptRow::previous_owner`, for a
    // whole run.
    assert_eq!(
        lease.previous_holder.as_deref(),
        Some("worker-a"),
        "{name}: a takeover says who it interrupted"
    );

    // And the one it replaced no longer holds anything, however sure it is.
    let read = ok!(name, store.decider_lease(&execution), "a read")
        .unwrap_or_else(|| panic!("{name}: the lease was not there"));
    assert!(!read.held_by("worker-a", after), "{name}");
    assert!(read.held_by("worker-b", after), "{name}");
}

/// Releasing is what makes a replacement start now rather than in five minutes.
pub async fn a_released_lease_is_free_before_it_would_have_run_out(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("release");
    let start = OffsetDateTime::UNIX_EPOCH;
    let soon = start + Duration::seconds(2);

    ok!(
        name,
        store.take_decider_lease(&execution, "worker-a", start),
        "a claim"
    );
    // Not yours to give up. Without this a worker that had already been taken
    // over could release its replacement's lease.
    assert!(
        !ok!(
            name,
            store.release_decider_lease(&execution, "worker-b", soon),
            "somebody else releasing"
        ),
        "{name}: a lease is released by the one holding it"
    );
    assert!(
        ok!(
            name,
            store.release_decider_lease(&execution, "worker-a", soon),
            "the holder releasing"
        ),
        "{name}"
    );
    assert!(
        ok!(name, store.decider_lease(&execution), "a read").is_none(),
        "{name}: a released lease is gone rather than expiring quietly"
    );

    // And the next decider takes it immediately, long before the lease would
    // have run out on its own.
    let taken = ok!(
        name,
        store.take_decider_lease(&execution, "worker-b", soon),
        "the next claim"
    );
    assert!(taken.taken().is_some(), "{name}");
}

/// Every timer due at `now`, however many other runs left behind.
///
/// `due_timers` is bounded and reads across every execution, and on a shared
/// PostgreSQL the table holds each earlier run's timers at the same fixed
/// instants — ordered by `due_at` alone, so a tie comes back in no stated
/// order. A fixed bound is filled by those before it reaches this run's, and
/// the property then fails, or passes having checked nothing, by chance. Asked
/// again with a larger bound until the store returns fewer than it was allowed,
/// the answer is the whole backlog at that instant.
async fn everything_due(name: &str, store: &dyn WorkflowStore, now: OffsetDateTime) -> Vec<Timer> {
    let mut limit = 64;
    loop {
        let due = ok!(name, store.due_timers(now, limit), "the timers due");
        assert!(
            due.len() <= limit,
            "{name}: asked for at most {limit} timers and was handed {}",
            due.len()
        );
        if due.len() < limit {
            return due;
        }
        limit *= 2;
    }
}

/// The part of an answer that belongs to one execution, in the order it came.
fn timers_of_this_run<'a>(execution: &ExecutionId, due: &'a [Timer]) -> Vec<&'a Timer> {
    due.iter()
        .filter(|found| found.execution.as_str() == execution.as_str())
        .collect()
}

/// A deferred append is due at its time, across every execution at once.
///
/// The whole reason a timer is a row rather than an event in the stream it
/// belongs to: one tick has to find what is due without opening a stream per
/// run. Written against the port, because the four adapters answer it four
/// ways — a map, a file read whole, an indexed `due_at`, and a lifted column.
///
/// What is due is read across every execution, as a tick reads it, and then
/// narrowed to this one's: a database other runs have used holds their timers
/// at these same instants, due whenever these are.
pub async fn a_timer_is_due_when_its_time_comes_and_not_before(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("timers");
    let start = OffsetDateTime::UNIX_EPOCH;
    ok!(
        name,
        store.append(
            &execution,
            timer_request(
                &execution,
                "m-1",
                vec![
                    TimerWrite::Schedule(timer(&execution, "soon", start + Duration::seconds(10))),
                    TimerWrite::Schedule(timer(
                        &execution,
                        "later",
                        start + Duration::seconds(600)
                    )),
                ]
            )
        ),
        "scheduling two timers"
    );
    let ids = |timers: &[&Timer]| {
        timers
            .iter()
            .map(|found| found.timer_id.clone())
            .collect::<Vec<_>>()
    };

    let due = everything_due(name, store, start).await;
    assert!(
        timers_of_this_run(&execution, &due).is_empty(),
        "{name}: a timer is not due before its time"
    );
    let due = everything_due(name, store, start + Duration::seconds(11)).await;
    let mine = timers_of_this_run(&execution, &due);
    assert_eq!(ids(&mine), ["soon"], "{name}");
    // The message comes back as the worker composed it. An engine that
    // assembled one here would be deciding what a timeout means.
    assert_eq!(mine[0].message.message_type, "saga.timeout_fired", "{name}");

    // Both, oldest first, once the second is due as well — and the whole
    // answer oldest first, not only the part of it that is this run's.
    let due = everything_due(name, store, start + Duration::seconds(3_600)).await;
    assert!(
        due.windows(2).all(|pair| pair[0].due_at <= pair[1].due_at),
        "{name}: a backlog drains in the order it accumulated"
    );
    assert_eq!(
        ids(&timers_of_this_run(&execution, &due)),
        ["soon", "later"],
        "{name}: a backlog drains in the order it accumulated"
    );

    // And a run's own page shows what it is still waiting on.
    let waiting = ok!(name, store.timers_of(&execution), "the run's timers");
    assert_eq!(waiting.len(), 2, "{name}");
}

/// Fired and cancelled timers leave nothing behind, and rescheduling is one row.
pub async fn a_fired_timer_leaves_no_row_to_fire_again(name: &str, store: &dyn WorkflowStore) {
    let execution = fresh("fired");
    let start = OffsetDateTime::UNIX_EPOCH;
    let due = start + Duration::seconds(5);

    ok!(
        name,
        store.append(
            &execution,
            timer_request(
                &execution,
                "m-1",
                vec![
                    TimerWrite::Schedule(timer(&execution, "once", due)),
                    TimerWrite::Schedule(timer(&execution, "withdrawn", due)),
                ]
            )
        ),
        "scheduling"
    );
    // Scheduling the same id again is one timer, which is what makes a
    // decider's retry safe.
    ok!(
        name,
        store.append(
            &execution,
            timer_request(
                &execution,
                "m-2",
                vec![TimerWrite::Schedule(timer(&execution, "once", due))]
            )
        ),
        "scheduling the same id again"
    );
    assert_eq!(
        ok!(name, store.timers_of(&execution), "after a repeat").len(),
        2,
        "{name}: the same id twice is one timer"
    );

    ok!(
        name,
        store.append(
            &execution,
            timer_request(
                &execution,
                "m-3",
                vec![
                    TimerWrite::Fire("once".to_owned()),
                    TimerWrite::Cancel("withdrawn".to_owned()),
                ]
            )
        ),
        "firing and cancelling"
    );
    assert!(
        timers_of_this_run(&execution, &everything_due(name, store, due).await).is_empty(),
        "{name}: a fired timer is not a row"
    );
    assert!(
        ok!(name, store.timers_of(&execution), "the run's timers").is_empty(),
        "{name}"
    );

    // Cancelling one that was never there is the ordinary thing a saga does
    // when it already handled the thing it was waiting for.
    ok!(
        name,
        store.append(
            &execution,
            timer_request(
                &execution,
                "m-4",
                vec![TimerWrite::Cancel("never".to_owned())]
            )
        ),
        "cancelling nothing"
    );
}

/// A worker whose reply was lost is answered from the history, not from a scan.
///
/// The read that loaded the *whole* execution stream before this existed — the
/// performance follow-up the worker protocol review left behind. It is a port
/// method so that `postgres` and `duckdb` answer it from an index while the
/// other two walk what they already hold, and so that all four agree on which
/// events count as an outcome.
pub async fn one_attempt_s_outcome_is_found_without_reading_its_neighbours(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("outcome");
    ok!(
        name,
        store.append(
            &execution,
            start_request(&execution, ExpectedVersion::NoStream, "m-1")
        ),
        "a start"
    );
    let before = ok!(name, store.load(&execution), "a load").version;
    ok!(
        name,
        store.append(
            &execution,
            outcome_request(&execution, before, "m-2", "extract", 1)
        ),
        "an outcome"
    );

    let found = ok!(
        name,
        store.recorded_outcome(&AttemptKey::new(execution.clone(), "extract", 1)),
        "the recorded outcome"
    );
    assert!(
        matches!(
            found,
            Some(WorkflowEvent::StepCompleted { ref step_id, attempt, .. })
                if step_id == "extract" && attempt == 1
        ),
        "{name}: {found:?}"
    );

    // A neighbour's attempt and a neighbour's step are not this one's. Getting
    // that wrong would acknowledge a report nothing ever recorded.
    assert!(
        ok!(
            name,
            store.recorded_outcome(&AttemptKey::new(execution.clone(), "extract", 2)),
            "another attempt"
        )
        .is_none(),
        "{name}"
    );
    assert!(
        ok!(
            name,
            store.recorded_outcome(&AttemptKey::new(execution.clone(), "load", 1)),
            "another step"
        )
        .is_none(),
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

pub async fn an_exact_attempt_filter_never_claims_a_neighbour(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("exact-claim");
    let first = claimable(&execution);
    let mut second = first.clone();
    second.key.step_id = "second".to_owned();
    let now = OffsetDateTime::UNIX_EPOCH;
    ok!(
        name,
        store.append(
            &execution,
            dispatch(
                &execution,
                "exact-dispatch",
                vec![
                    AttemptWrite::Dispatch(first.clone()),
                    AttemptWrite::Dispatch(second.clone())
                ]
            )
        ),
        "dispatch neighbours"
    );
    let mut filter = mine(&execution);
    filter.attempt = Some(AttemptKey::new(execution.clone(), "absent", 1));
    assert!(
        ok!(
            name,
            store.claim_attempt(&filter, "worker", now),
            "absent target"
        )
        .is_none()
    );
    filter.attempt = Some(second.key.clone());
    filter.tasks.clear();
    assert!(ok!(name, store.claim_attempt(&filter, "worker", now), "no code").is_none());
    filter.tasks.push(STAGE_TASK.to_owned());
    let taken = ok!(
        name,
        store.claim_attempt(&filter, "worker", now),
        "exact target"
    )
    .expect("target available");
    assert_eq!(taken.key, second.key, "{name}: took a neighbour");
    assert!(
        ok!(
            name,
            store.claim_attempt(&filter, "competitor", now),
            "held target"
        )
        .is_none()
    );
    assert!(
        ok!(name, store.attempt(&first.key), "untouched neighbour")
            .expect("row")
            .lease_owner
            .is_none()
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

/// One boundary for every lease this store keeps.
///
/// [`aiwatcher_jobs::lease_expired`] is strict: a lease exactly
/// [`aiwatcher_jobs::LEASE_SECONDS`] old is still held, and one second later it
/// is not. The other adapters ask that function; PostgreSQL writes the
/// comparison into its statements, and a `<=` there took a row at exactly five
/// minutes that `unclaimed_attempts` — written with `<` — still counted as
/// held, while its `>` refused the holder's heartbeat at the instant
/// [`AttemptRow::is_held_by`] said the row was still its own. Pinned at the one
/// second where the two readings differ: at any other instant they agree, which
/// is why every other property here passed either way. The decider lease and a
/// schedule slot are the same rule over a whole run and one firing, so they are
/// pinned at the same second.
pub async fn a_lease_exactly_its_length_old_is_still_held_and_one_second_later_is_not(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("lease-boundary");
    let start = OffsetDateTime::UNIX_EPOCH;
    let boundary = start + Duration::seconds(aiwatcher_jobs::LEASE_SECONDS);
    let after = boundary + Duration::seconds(1);

    // Counted at both instants before anything is dispatched: on a shared
    // PostgreSQL the table holds other runs' rows, and at a fixed instant
    // their contribution is a fixed number.
    let before_boundary = ok!(name, store.unclaimed_attempts(boundary), "a count before");
    let before_after = ok!(
        name,
        store.unclaimed_attempts(after),
        "a later count before"
    );

    // A reactor's row, so the count sees it, and a worker's for the renewal.
    let counted = AttemptRow::claimable(
        AttemptKey::new(execution.clone(), "counted", 1),
        RuntimeKind::FlowPhp,
        MessageId::new(format!("{execution}/cmd-counted")),
    );
    let renewed = claimable(&execution);
    ok!(
        name,
        store.append(
            &execution,
            dispatch(
                &execution,
                "boundary-dispatch",
                vec![
                    AttemptWrite::Dispatch(counted.clone()),
                    AttemptWrite::Dispatch(renewed.clone()),
                ],
            )
        ),
        "a dispatch"
    );
    let exact = ClaimFilter {
        attempt: Some(counted.key.clone()),
        ..ClaimFilter::for_runtimes(&[RuntimeKind::FlowPhp])
    };
    ok!(
        name,
        store.claim_attempt(&exact, "reactor-a", start),
        "a claim"
    )
    .unwrap_or_else(|| panic!("{name}: the exact claim found nothing to take"));
    ok!(
        name,
        store.claim_attempt(&mine(&execution), "worker-a", start),
        "a worker's claim"
    )
    .unwrap_or_else(|| panic!("{name}: the worker's claim found nothing to take"));

    // Exactly LEASE_SECONDS old: still held, by every reading of it.
    assert!(
        ok!(
            name,
            store.claim_attempt(&exact, "reactor-b", boundary),
            "a claim at the boundary"
        )
        .is_none(),
        "{name}: a lease exactly LEASE_SECONDS old was taken over"
    );
    let at_boundary = ok!(
        name,
        store.unclaimed_attempts(boundary),
        "a count at the boundary"
    );
    assert_eq!(
        grew(&before_boundary, &at_boundary, RuntimeKind::FlowPhp),
        0,
        "{name}: a lease exactly LEASE_SECONDS old was counted as waiting for a claimant"
    );
    assert!(
        ok!(
            name,
            store.heartbeat(&renewed.key, "worker-a", boundary),
            "a heartbeat at the boundary"
        ),
        "{name}: the holder could not renew a lease in its last second"
    );

    // One second later: free, by every reading of it — and the count is asked
    // before the claim, which would make it held again.
    let at_after = ok!(
        name,
        store.unclaimed_attempts(after),
        "a count after the boundary"
    );
    assert_eq!(
        grew(&before_after, &at_after, RuntimeKind::FlowPhp),
        1,
        "{name}: a lease one second past LEASE_SECONDS was not counted as waiting"
    );
    let taken = ok!(
        name,
        store.claim_attempt(&exact, "reactor-b", after),
        "a claim after the boundary"
    )
    .unwrap_or_else(|| panic!("{name}: a lease one second past LEASE_SECONDS was not taken over"));
    assert_eq!(taken.lease_owner.as_deref(), Some("reactor-b"), "{name}");
    assert_eq!(
        taken.previous_owner.as_deref(),
        Some("reactor-a"),
        "{name}: a takeover says who it interrupted"
    );
    // Except the one renewed at the boundary, which is held from there.
    assert!(
        ok!(
            name,
            store.claim_attempt(&mine(&execution), "worker-b", after),
            "a claim of the renewed row"
        )
        .is_none(),
        "{name}: a lease renewed in its last second was taken over the second after"
    );

    // The decider lease: the same rule, over a whole run.
    let decided = fresh("lease-boundary-decider");
    ok!(
        name,
        store.take_decider_lease(&decided, "worker-a", start),
        "a decider's claim"
    );
    match ok!(
        name,
        store.take_decider_lease(&decided, "worker-b", boundary),
        "a decider's claim at the boundary"
    ) {
        LeaseOutcome::Held { holder, .. } => assert_eq!(holder, "worker-a", "{name}"),
        LeaseOutcome::Taken(_) => {
            panic!("{name}: a decider lease exactly LEASE_SECONDS old was taken over")
        }
    }
    let taken = ok!(
        name,
        store.take_decider_lease(&decided, "worker-b", after),
        "a decider's claim after the boundary"
    );
    assert!(
        taken
            .taken()
            .is_some_and(|lease| lease.holder == "worker-b"),
        "{name}: a decider lease one second past LEASE_SECONDS was not taken over"
    );
    let released = fresh("lease-boundary-release");
    ok!(
        name,
        store.take_decider_lease(&released, "worker-a", start),
        "a decider's claim to release"
    );
    assert!(
        ok!(
            name,
            store.release_decider_lease(&released, "worker-a", boundary),
            "a release at the boundary"
        ),
        "{name}: the holder could not release a decider lease in its last second"
    );

    // And a schedule slot: the same rule, over one firing.
    let slot = OffsetDateTime::UNIX_EPOCH + Duration::days(6);
    let key = SlotKey::new(
        DefinitionKind::CurationPipeline,
        format!("{name}-lease-boundary-{}", stamp()),
        slot,
    );
    ok!(
        name,
        store.admit_slot(&admission(&key, "tick-a", slot)),
        "a slot's claim"
    );
    assert_eq!(
        ok!(
            name,
            store.admit_slot(&admission(
                &key,
                "tick-b",
                slot + Duration::seconds(aiwatcher_jobs::LEASE_SECONDS)
            )),
            "a slot's claim at the boundary"
        ),
        SlotAdmission::Held {
            owner: "tick-a".to_owned()
        },
        "{name}: a slot lease exactly LEASE_SECONDS old was taken over"
    );
    assert_eq!(
        ok!(
            name,
            store.admit_slot(&admission(
                &key,
                "tick-b",
                slot + Duration::seconds(aiwatcher_jobs::LEASE_SECONDS + 1)
            )),
            "a slot's claim after the boundary"
        ),
        SlotAdmission::Admitted,
        "{name}: a slot lease one second past LEASE_SECONDS was not taken over"
    );
}

/// A finished attempt is retired, not stored in a terminal state.
///
/// Two assertions, and the second is the one that costs something to break: a
/// terminal row is invisible to a claimant either way, so keeping one looks
/// correct until the table is the size of the history.
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

pub async fn a_parked_attempt_keeps_its_row_and_loses_its_lease(
    name: &str,
    store: &dyn WorkflowStore,
) {
    // The third shape, and the only one that changes a row instead of adding or
    // removing one. Both halves matter and each fails differently: kept but
    // still leased, the answer's new attempt would be racing a lease nobody is
    // renewing; released but retired, the attempt that asked would be gone and
    // with it the question and who answered it.
    let execution = fresh("claim-parked");
    let key = AttemptKey::new(execution.clone(), "extract", 1);
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
    let claimed = ok!(
        name,
        store.claim_attempt(&mine(&execution), "worker", OffsetDateTime::UNIX_EPOCH),
        "a claim"
    );
    assert!(claimed.is_some(), "{name}: nothing was claimable to park");

    ok!(
        name,
        store.append(
            &execution,
            dispatch(&execution, "m-2", vec![AttemptWrite::Park(key.clone())]),
        ),
        "a park"
    );

    let row = ok!(name, store.attempt(&key), "reading the parked attempt").unwrap_or_else(|| {
        panic!("{name}: a parked attempt lost its row, so the question went with it")
    });
    assert_eq!(
        row.state,
        StateType::AwaitingInput,
        "{name}: a parked attempt is not in the state that says so"
    );
    assert!(
        row.lease_owner.is_none() && row.claimed_at.is_none(),
        "{name}: a worker that stopped to ask is still holding a pod for the answer"
    );

    // Not claimable at any hour. A released lease is not an invitation: this
    // one resumes on the answer it asked for, and the answer dispatches attempt
    // two under its own key.
    let long_after =
        OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(aiwatcher_jobs::LEASE_SECONDS * 4);
    assert!(
        ok!(
            name,
            store.claim_attempt(&mine(&execution), "somebody-else", long_after),
            "a later claim"
        )
        .is_none(),
        "{name}: a question a worker is waiting on was picked up because time passed"
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

/// A pod's attempt, from both sides of ADR_0029: the launcher finds it by its
/// runtime without taking it, and only the claim naming its key takes it.
///
/// Every read is narrowed to this execution: on a shared PostgreSQL the table
/// holds other runs' rows. The row is retired at the end for the same reason.
pub async fn a_pods_attempt_is_read_by_its_runtime_and_taken_only_by_its_key(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("pod-claim");
    let now = OffsetDateTime::UNIX_EPOCH;
    let pod = AttemptRow::claimable(
        AttemptKey::new(execution.clone(), "stage", 1),
        RuntimeKind::ContainerJob,
        MessageId::new(format!("{execution}/cmd-pod")),
    )
    .on_queue(queue_of(&execution), STAGE_TASK.to_owned());
    ok!(
        name,
        store.append(
            &execution,
            dispatch(
                &execution,
                "pod-dispatch",
                vec![AttemptWrite::Dispatch(pod.clone())]
            )
        ),
        "a dispatch"
    );
    let this_run = |rows: Vec<AttemptRow>| -> Vec<AttemptKey> {
        rows.into_iter()
            .filter(|row| row.key.execution_id == execution)
            .map(|row| row.key)
            .collect()
    };

    let found = ok!(
        name,
        store.claimable_attempts(RuntimeKind::ContainerJob, now, 1_000),
        "a read"
    );
    assert_eq!(
        this_run(found),
        vec![pod.key.clone()],
        "{name}: the launcher did not find the pod's attempt"
    );
    let other = ok!(
        name,
        store.claimable_attempts(RuntimeKind::PythonTask, now, 1_000),
        "a read of another runtime"
    );
    assert!(
        this_run(other).is_empty(),
        "{name}: a read by one runtime returned another's row"
    );

    // A worker holding the pod's queue and its code, naming no key.
    assert!(
        ok!(
            name,
            store.claim_attempt(&mine(&execution), "worker", now),
            "a keyless claim"
        )
        .is_none(),
        "{name}: a worker naming no key took a pod's attempt"
    );
    let the_pod = ClaimFilter {
        attempt: Some(pod.key.clone()),
        ..mine(&execution)
    };
    let taken = ok!(
        name,
        store.claim_attempt(&the_pod, "pod-1", now),
        "the pod's claim"
    )
    .unwrap_or_else(|| panic!("{name}: the pod could not claim its own attempt"));
    assert_eq!(taken.key, pod.key, "{name}");

    // Held, it is no longer an attempt to start a pod for.
    let held = ok!(
        name,
        store.claimable_attempts(RuntimeKind::ContainerJob, now + Duration::seconds(1), 1_000),
        "a read while held"
    );
    assert!(
        this_run(held).is_empty(),
        "{name}: an attempt a pod holds was read as one to launch"
    );

    ok!(
        name,
        store.append(
            &execution,
            dispatch(
                &execution,
                "pod-retire",
                vec![AttemptWrite::Retire(pod.key.clone())]
            )
        ),
        "a retirement"
    );
}

/// A count of the claim table, and nothing more than a count.
///
/// What the work role asks on a timer to find attempts no registered executor
/// performs — so it must count exactly what a reactor would take, and must
/// never be the thing that takes it. Every count is compared with one taken
/// before the dispatch *at the same instant*: on a shared PostgreSQL the table
/// holds other runs' rows, and at a fixed instant their contribution is a
/// fixed number.
pub async fn an_attempt_waiting_for_a_reactor_is_counted_by_its_runtime_and_left_alone(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("unclaimed");
    let now = OffsetDateTime::UNIX_EPOCH;
    let expired = now + Duration::seconds(aiwatcher_jobs::LEASE_SECONDS + 1);
    let before_now = ok!(name, store.unclaimed_attempts(now), "a count before");
    let before_expired = ok!(
        name,
        store.unclaimed_attempts(expired),
        "a later count before"
    );

    let for_a_reactor = |step: &str, runtime: RuntimeKind| {
        AttemptRow::claimable(
            AttemptKey::new(execution.clone(), step, 1),
            runtime,
            MessageId::new(format!("{execution}/cmd-{step}")),
        )
    };
    let waiting = for_a_reactor("waiting", RuntimeKind::FlowPhp);
    let held = for_a_reactor("held", RuntimeKind::FlowPhp);
    let asked = for_a_reactor("asked", RuntimeKind::FlowPhp);
    let backing_off = for_a_reactor("backing-off", RuntimeKind::DataFusion)
        .not_before(now + Duration::seconds(30));
    let pulled = claimable(&execution);
    ok!(
        name,
        store.append(
            &execution,
            dispatch(
                &execution,
                "unclaimed-dispatch",
                [&waiting, &held, &asked, &backing_off, &pulled]
                    .map(|row| AttemptWrite::Dispatch(row.clone()))
                    .to_vec(),
            )
        ),
        "a dispatch"
    );

    // One held by a live lease, and one that stopped to ask.
    let exact = ClaimFilter {
        attempt: Some(held.key.clone()),
        ..ClaimFilter::for_runtimes(&[RuntimeKind::FlowPhp])
    };
    let taken = ok!(name, store.claim_attempt(&exact, "reactor", now), "a claim")
        .unwrap_or_else(|| panic!("{name}: the exact claim found nothing to take"));
    ok!(
        name,
        store.append(
            &execution,
            dispatch(
                &execution,
                "unclaimed-park",
                vec![AttemptWrite::Park(asked.key.clone())]
            )
        ),
        "a park"
    );

    let at_now = ok!(name, store.unclaimed_attempts(now), "a count");
    assert_eq!(
        grew(&before_now, &at_now, RuntimeKind::FlowPhp),
        1,
        "{name}: only the unheld flow row is waiting; a live lease and a question are not"
    );
    assert_eq!(
        grew(&before_now, &at_now, RuntimeKind::DataFusion),
        1,
        "{name}: a retry inside its delay is waiting for a claimant all the same"
    );
    assert_eq!(
        grew(&before_now, &at_now, RuntimeKind::PythonTask),
        0,
        "{name}: a worker's row was counted as a reactor's"
    );
    let at_expiry = ok!(name, store.unclaimed_attempts(expired), "a later count");
    assert_eq!(
        grew(&before_expired, &at_expiry, RuntimeKind::FlowPhp),
        2,
        "{name}: a lease that ran out is waiting for a claimant again"
    );

    // Counting took nothing and changed nothing — least of all the lease it
    // counted as expired.
    assert_eq!(
        ok!(name, store.attempt(&waiting.key), "the waiting row").as_ref(),
        Some(&waiting),
        "{name}: counting rewrote a row it only had to count"
    );
    let still = ok!(name, store.attempt(&held.key), "the held row")
        .unwrap_or_else(|| panic!("{name}: the held row is gone"));
    assert_eq!(
        (still.lease_owner.as_deref(), still.claimed_at),
        (Some("reactor"), taken.claimed_at),
        "{name}: counting past a lease's expiry took the attempt over"
    );
    assert_eq!(
        ok!(name, store.attempt(&asked.key), "the parked row").map(|row| row.state),
        Some(StateType::AwaitingInput),
        "{name}: counting woke a question"
    );

    // And a retired attempt is waiting for nobody.
    ok!(
        name,
        store.append(
            &execution,
            dispatch(
                &execution,
                "unclaimed-retire",
                [waiting, held, asked, backing_off, pulled]
                    .map(|row| AttemptWrite::Retire(row.key))
                    .to_vec(),
            )
        ),
        "retiring them"
    );
    let retired = ok!(name, store.unclaimed_attempts(now), "a count after");
    for runtime in [RuntimeKind::FlowPhp, RuntimeKind::DataFusion] {
        assert_eq!(
            grew(&before_now, &retired, runtime),
            0,
            "{name}: a retired {} attempt is still counted as waiting",
            runtime.as_str()
        );
    }
}

/// How much one runtime's count moved between two counts at one instant.
fn grew(
    before: &BTreeMap<RuntimeKind, u64>,
    after: &BTreeMap<RuntimeKind, u64>,
    runtime: RuntimeKind,
) -> i128 {
    let count = |counts: &BTreeMap<RuntimeKind, u64>| {
        i128::from(counts.get(&runtime).copied().unwrap_or(0))
    };
    count(after) - count(before)
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
/// The release before this one wrote every failure down as `refused` and then
/// moved the global cursor past the slot, so a store that was unreachable for
/// ten seconds at 09:00 cost the day's run and left a note saying it had been
/// refused.
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
/// The property the previous implementation could not have: it asked an
/// asynchronous read model that is **empty in the role the tick runs in**, so
/// skip never skipped there. `allow` is checked in the same property,
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
    // which is exactly what the read model was giving before.
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

// ── Scope: who owns an execution, and who may not reach it ───────────────────
//
// These are the properties that make the reactor, the worker, the launcher, the
// timer tick, the outbox publisher and the retention sweep this binary already
// runs safe to leave as they are. Each one is a thing a global path would
// otherwise do to a project's run by accident, so each is asserted from both
// sides: the unscoped store refusing, and the bound store answering.

/// The record is written with the execution and reads back as what started it.
pub async fn an_execution_is_owned_by_the_project_that_started_it_and_reads_back_that_way(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("owned");
    let scope = project();
    let mine = bound(name, store, scope);
    let record = owner(scope, "alice");
    ok!(
        name,
        mine.append(&execution, owned_start(&execution, "start", record.clone())),
        "a project start"
    );

    assert_eq!(
        ok!(name, mine.ownership(&execution), "reading the owner"),
        Some(record.clone()),
        "{name}: the owner is the record the start established"
    );
    // And it names what was started, so a dispatcher can decide before it reads
    // the stream.
    assert_eq!(record.definition.plan_id, plan().plan_id, "{name}");
    assert!(
        ok!(name, mine.load(&execution), "loading the stream").version > 0,
        "{name}: the stream landed with the record"
    );
    assert!(
        ok!(name, mine.projection(&execution), "the projection").is_some(),
        "{name}"
    );
}

/// Every global door refuses a project's execution, rather than answering it.
pub async fn an_unscoped_path_reaches_nothing_about_a_project_s_execution(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("hidden");
    let scope = project();
    let mine = bound(name, store, scope);
    ok!(
        name,
        mine.append(
            &execution,
            owned_start(&execution, "start", owner(scope, "alice"))
        ),
        "a project start"
    );

    // Reads first: a known id must not be a way in. The refusal says so rather
    // than answering an empty stream, which would read as "no such run" and
    // send somebody to start a second one under an id that is taken.
    for (what, refused) in [
        ("load", store.load(&execution).await.err()),
        ("load_page", store.load_page(&execution, 0, 10).await.err()),
        ("projection", store.projection(&execution).await.err()),
        ("ownership", store.ownership(&execution).await.err()),
        ("timers_of", store.timers_of(&execution).await.err()),
        ("decider_lease", store.decider_lease(&execution).await.err()),
        (
            "attempt",
            store
                .attempt(&AttemptKey::new(execution.clone(), "extract", 1))
                .await
                .err(),
        ),
        (
            "recorded_outcome",
            store
                .recorded_outcome(&AttemptKey::new(execution.clone(), "extract", 1))
                .await
                .err(),
        ),
    ] {
        let Some(error) = refused else {
            panic!("{name}: the unscoped store answered {what} for a project's execution");
        };
        assert!(is_out_of_scope(&error), "{name}: {what}: {error}");
        assert!(
            error.says_the_same_next_time(),
            "{name}: {what}: a boundary is not a bad moment"
        );
    }

    // Then the writes. A command, and a start of the *same* id: both are the
    // unscoped path reaching a run it does not own.
    let refused = store
        .append(
            &execution,
            dispatch(&execution, "global-command", Vec::new()),
        )
        .await
        .expect_err("an unscoped command on a project's execution");
    assert!(is_out_of_scope(&refused), "{name}: {refused}");

    let refused = store
        .append(
            &execution,
            start_request(&execution, ExpectedVersion::Any, "start"),
        )
        .await
        .expect_err("an unscoped start of a project's execution");
    assert!(is_out_of_scope(&refused), "{name}: {refused}");

    // And the lease, which is a write that reads like a read.
    let refused = store
        .take_decider_lease(&execution, "somebody", OffsetDateTime::UNIX_EPOCH)
        .await
        .expect_err("an unscoped lease on a project's execution");
    assert!(is_out_of_scope(&refused), "{name}: {refused}");
}

/// One project's id reaches nothing of another's.
pub async fn a_project_reaches_nothing_about_another_project_s_execution(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("neighbour");
    let (one, other) = (project(), project());
    let mine = bound(name, store, one);
    let theirs = bound(name, store, other);
    ok!(
        name,
        mine.append(
            &execution,
            owned_start(&execution, "start", owner(one, "alice"))
        ),
        "a project start"
    );

    let refused = theirs
        .load(&execution)
        .await
        .expect_err("another project's execution");
    assert!(is_out_of_scope(&refused), "{name}: {refused}");

    let refused = theirs
        .append(&execution, dispatch(&execution, "theirs", Vec::new()))
        .await
        .expect_err("another project's command");
    assert!(is_out_of_scope(&refused), "{name}: {refused}");

    // Including a start of their own under an id that is taken: the record says
    // whose it is, and a second scope's start of it is not this execution's.
    let refused = theirs
        .append(
            &execution,
            owned_start(&execution, "start", owner(other, "mallory")),
        )
        .await
        .expect_err("another project's start of a taken id");
    assert!(
        is_out_of_scope(&refused) || matches!(refused, StoreError::OwnershipConflict { .. }),
        "{name}: {refused}"
    );
}

/// An execution that exists and has no owner is a global run and stays one.
pub async fn a_project_may_not_adopt_an_execution_nobody_owns(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("global");
    ok!(
        name,
        store.append(
            &execution,
            start_request(&execution, ExpectedVersion::NoStream, "start")
        ),
        "a global start"
    );

    let scope = project();
    let mine = bound(name, store, scope);
    for (what, refused) in [
        ("load", mine.load(&execution).await.err()),
        ("projection", mine.projection(&execution).await.err()),
        (
            "append",
            mine.append(&execution, dispatch(&execution, "adopt", Vec::new()))
                .await
                .err(),
        ),
        (
            "ownership",
            mine.append(
                &execution,
                owned_start(&execution, "adopt-start", owner(scope, "mallory")),
            )
            .await
            .err(),
        ),
    ] {
        let Some(error) = refused else {
            panic!("{name}: a project store adopted a global execution through {what}");
        };
        assert!(is_out_of_scope(&error), "{name}: {what}: {error}");
    }

    // And the global path still has it, unchanged.
    assert!(
        ok!(name, store.load(&execution), "the global stream").version > 0,
        "{name}"
    );
}

/// A repeated start naming a different owner is refused, never a second owner.
pub async fn a_repeated_start_naming_another_owner_does_not_repoint_the_execution(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("immutable");
    let scope = project();
    let mine = bound(name, store, scope);
    let held = owner(scope, "alice");
    ok!(
        name,
        mine.append(&execution, owned_start(&execution, "start", held.clone())),
        "a project start"
    );

    // Same execution, same project, a different principal. The message id is
    // the same one the first start used — a redelivery by every other measure —
    // and it is still refused, because the answer to "whose run is this" may
    // not depend on who asked last.
    let refused = mine
        .append(
            &execution,
            owned_start(&execution, "start", owner(scope, "mallory")),
        )
        .await
        .expect_err("a second owner");
    assert!(
        matches!(refused, StoreError::OwnershipConflict { .. }),
        "{name}: {refused}"
    );
    assert!(refused.says_the_same_next_time(), "{name}");
    assert_eq!(
        ok!(name, mine.ownership(&execution), "the owner"),
        Some(held),
        "{name}: the record did not move"
    );
}

/// The same start twice is one execution, one owner and one history.
pub async fn the_same_start_twice_in_one_project_is_one_execution_and_one_owner(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("idempotent");
    let scope = project();
    let mine = bound(name, store, scope);
    let record = owner(scope, "alice");
    let first = ok!(
        name,
        mine.append(&execution, owned_start(&execution, "start", record.clone())),
        "the first start"
    );
    let again = ok!(
        name,
        mine.append(&execution, owned_start(&execution, "start", record.clone())),
        "the same start again"
    );
    assert!(
        again.is_duplicate(),
        "{name}: a redelivered start is a duplicate, not a second run"
    );
    // The stream did not grow. `Duplicate` reports where the *input* landed
    // and `Appended` reports the last version written, so the two numbers are
    // not the same question and comparing them would prove nothing.
    assert_eq!(
        ok!(name, mine.load(&execution), "the stream").version,
        first.version(),
        "{name}: the second start appended nothing"
    );
    assert_eq!(
        ok!(name, mine.ownership(&execution), "the owner"),
        Some(record),
        "{name}"
    );
}

/// The claim a global reactor, worker or launcher would make sees nothing of a
/// project's work.
pub async fn a_global_claimant_never_takes_a_project_s_attempt(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("project-claim");
    let scope = project();
    let mine = bound(name, store, scope);
    ok!(
        name,
        mine.append(
            &execution,
            owned_start(&execution, "start", owner(scope, "alice"))
        ),
        "a project start"
    );
    ok!(
        name,
        mine.append(
            &execution,
            dispatch(
                &execution,
                "dispatch",
                vec![AttemptWrite::Dispatch(claimable(&execution))]
            )
        ),
        "a dispatch"
    );

    let now = OffsetDateTime::UNIX_EPOCH;
    // The filter matches the row in every way a filter can. What refuses it is
    // the store's binding, which is the point: a claimant says what it can run,
    // not which executions it may reach.
    assert!(
        ok!(
            name,
            store.claim_attempt(&mine_filter(&execution), "global-worker", now),
            "an unscoped claim"
        )
        .is_none(),
        "{name}: the unscoped claimant took a project's attempt"
    );
    // The launcher's read and the stranded-work count are the same question
    // asked without claiming, and they answer the same way.
    assert!(
        ok!(
            name,
            store.claimable_attempts(RuntimeKind::PythonTask, now, 10),
            "an unscoped read"
        )
        .iter()
        .all(|row| row.key.execution_id != execution),
        "{name}: the unscoped launcher read a project's attempt"
    );
    // And by key, which is how a pod claims its own attempt (ADR_0029): the
    // refusal comes from the store's binding rather than from the row being
    // absent, so a known key is not a way past it either.
    let refused = store
        .attempt(&claimable(&execution).key)
        .await
        .expect_err("an unscoped read of a project's attempt row");
    assert!(is_out_of_scope(&refused), "{name}: {refused}");
    let refused = store
        .claim_attempt(
            &ClaimFilter {
                attempt: Some(claimable(&execution).key),
                ..mine_filter(&execution)
            },
            "global-pod",
            now,
        )
        .await;
    assert!(
        refused.as_ref().is_ok_and(Option::is_none) || refused.is_err(),
        "{name}: the unscoped claimant took a project's attempt by key"
    );

    // And the project's own claimant does take it.
    let taken = ok!(
        name,
        mine.claim_attempt(&mine_filter(&execution), "project-worker", now),
        "a project claim"
    );
    assert_eq!(
        taken.map(|row| row.key.execution_id),
        Some(execution),
        "{name}: the project's own claimant takes its own row"
    );
}

/// And the other way: a project claimant sees nothing of the global side.
pub async fn a_project_claimant_never_takes_a_global_attempt(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("global-claim");
    ok!(
        name,
        store.append(
            &execution,
            start_request(&execution, ExpectedVersion::NoStream, "start")
        ),
        "a global start"
    );
    ok!(
        name,
        store.append(
            &execution,
            dispatch(
                &execution,
                "dispatch",
                vec![AttemptWrite::Dispatch(claimable(&execution))]
            )
        ),
        "a dispatch"
    );

    let mine = bound(name, store, project());
    assert!(
        ok!(
            name,
            mine.claim_attempt(
                &mine_filter(&execution),
                "project-worker",
                OffsetDateTime::UNIX_EPOCH
            ),
            "a project claim"
        )
        .is_none(),
        "{name}: a project claimant took a global attempt"
    );
}

/// A timer and an outbox row belong to the side their execution does.
pub async fn a_project_s_timers_and_outbox_rows_reach_only_its_own_side(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let execution = fresh("project-rows");
    let scope = project();
    let mine = bound(name, store, scope);
    let due = OffsetDateTime::UNIX_EPOCH + Duration::seconds(10);
    ok!(
        name,
        mine.append(
            &execution,
            owned_start(&execution, "start", owner(scope, "alice"))
        ),
        "a project start"
    );
    ok!(
        name,
        mine.append(
            &execution,
            timer_request(
                &execution,
                "schedule",
                vec![TimerWrite::Schedule(timer(&execution, "t-1", due))]
            )
        ),
        "a timer"
    );

    let later = due + Duration::seconds(1);
    assert!(
        ok!(name, store.due_timers(later, 100), "the global tick")
            .iter()
            .all(|found| found.execution != execution),
        "{name}: the global timer tick found a project's deadline"
    );
    assert!(
        ok!(name, mine.due_timers(later, 100), "the project's tick")
            .iter()
            .any(|found| found.execution == execution),
        "{name}: the project's own tick did not find its deadline"
    );

    // The start wrote an outbox row. The global publisher must not put a
    // project's facts on the instance's log.
    assert!(
        ok!(name, store.pending_outbox(1000), "the global outbox")
            .iter()
            .all(|row| row.partition_key != format!("workflow:{execution}")),
        "{name}: the global publisher would have published a project's fact"
    );
    let ours = ok!(name, mine.pending_outbox(1000), "the project's outbox");
    assert!(
        ours.iter()
            .any(|row| row.partition_key == format!("workflow:{execution}")),
        "{name}: the project's own publisher did not see its row"
    );
}

/// A sweep forgets its own side's runs and leaves the other's history alone.
pub async fn a_retention_sweep_forgets_its_own_side_and_leaves_the_other_alone(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let theirs = fresh("project-kept");
    let scope = project();
    let mine = bound(name, store, scope);
    ok!(
        name,
        mine.append(
            &theirs,
            owned_start(&theirs, "start", owner(scope, "alice"))
        ),
        "a project start"
    );
    // Its outbox row would keep it anyway; published first, so the only thing
    // deciding its fate is the scope.
    let rows: Vec<MessageId> = ok!(name, mine.pending_outbox(1000), "the project's outbox")
        .into_iter()
        .map(|row| row.message_id)
        .collect();
    ok!(
        name,
        mine.mark_published(&rows, OffsetDateTime::UNIX_EPOCH),
        "publishing"
    );
    ok!(
        name,
        mine.append(&theirs, finish(&theirs, "finish")),
        "an ending"
    );

    let swept = ok!(
        name,
        store.prune(well_after_everything(), 100),
        "an unscoped sweep"
    );
    let _ = swept;
    assert!(
        ok!(name, mine.projection(&theirs), "the project's run").is_some(),
        "{name}: the unscoped retention sweep forgot a project's run"
    );

    // The project's own sweep does forget it.
    ok!(
        name,
        mine.prune(well_after_everything(), 100),
        "the project's sweep"
    );
    assert!(
        ok!(name, mine.projection(&theirs), "the project's run").is_none(),
        "{name}: the project's own sweep kept it"
    );
}

/// One store, one project. A second binding is refused rather than replacing.
pub async fn a_store_binds_to_one_project_and_refuses_a_second(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let (one, other) = (project(), project());
    let mine = bound(name, store, one);
    assert_eq!(
        mine.scope(),
        ExecutionScope::Project(one),
        "{name}: a bound store says what it is bound to"
    );
    assert_eq!(
        store.scope(),
        ExecutionScope::Global,
        "{name}: the store a deployment wires is the unscoped one"
    );
    // Binding to the same project again is the same store, so wiring that
    // resolves a scope twice costs nothing.
    let again = bound(name, mine.as_ref(), one);
    assert_eq!(again.scope(), ExecutionScope::Project(one), "{name}");

    let refused = mine
        .for_project(other)
        .expect_err("a second project is refused");
    assert!(
        matches!(refused, StoreError::NotInThisScope { .. }),
        "{name}: {refused}"
    );
}

/// What has no scoped form says so by name rather than answering globally.
pub async fn what_is_instance_wide_is_refused_by_name_rather_than_answered(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let mine = bound(name, store, project());
    for (what, refused) in [
        ("checkpoint", mine.checkpoint("execution").await.err()),
        (
            "advance_checkpoint",
            mine.advance_checkpoint("execution", Checkpoint::from_global_position(1))
                .await
                .err(),
        ),
        (
            "recent_slots",
            mine.recent_slots(DefinitionKind::CurationPipeline, "import", 5)
                .await
                .err(),
        ),
        // The set of projects is the instance's own question, and a bound
        // store answering it would be answering across the boundary it exists
        // to keep.
        ("project_scopes", mine.project_scopes(100).await.err()),
    ] {
        let Some(error) = refused else {
            panic!("{name}: a project-bound store answered {what} from the instance's own tables");
        };
        assert!(
            matches!(error, StoreError::NotInThisScope { .. }),
            "{name}: {what}: {error}"
        );
    }
}

/// The one cross-scope answer, and the whole of what it says.
///
/// A work role cannot bind a dispatcher to a project it has not been told
/// about, so something has to name them. What this proves is that naming them
/// is all it does: the scopes of every execution this store owns, and nothing
/// about what any of them ran.
pub async fn the_projects_that_have_run_here_are_listed_as_scopes_and_never_as_rows(
    name: &str,
    store: &dyn WorkflowStore,
) {
    let before = ok!(name, store.project_scopes(1_000), "listing projects");
    let (one, other) = (project(), project());
    for scope in [one, other] {
        let execution = ExecutionId::new(format!("scopes-{}", scope.project.0));
        let bound = bound(name, store, scope);
        ok!(
            name,
            bound.append(
                &execution,
                owned_start(&execution, "scopes-in", owner(scope, "starter")),
            ),
            "starting a project execution"
        );
    }

    let after = ok!(name, store.project_scopes(1_000), "listing projects again");
    for scope in [one, other] {
        assert!(
            after.contains(&scope) && !before.contains(&scope),
            "{name}: a project that has just run here is not listed"
        );
    }
    assert!(
        after.windows(2).all(|pair| pair[0] < pair[1]),
        "{name}: the list is read twice and must be ordered"
    );

    // A limit is a bound on one answer, not a filter that quietly drops a
    // project: a caller asking for one gets one, and the deployment past its
    // ceiling is told by counting rather than by a short list that looks whole.
    let bounded = ok!(name, store.project_scopes(1), "listing one project");
    assert_eq!(bounded.len(), 1, "{name}: a limit bounds the answer");
    assert_eq!(bounded[0], after[0], "{name}: and takes them in order");
}

/// The filter that matches this execution's dispatched row in every way.
fn mine_filter(execution: &ExecutionId) -> ClaimFilter {
    mine(execution)
}
