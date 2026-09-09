#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! A managed execution draws itself in the existing workflow tab, with
//! `Pending` nodes, before any panel work exists for it.
//!
//! This is the whole argument of ADR_0026 in one test. The decider produces
//! facts, the outbox turns them into envelopes, and the workflow fold that has
//! been serving the graph since ADR_0012 draws them — no second read path, no
//! PostgreSQL query, no new component. If this ever needs a special case in the
//! fold, the ADR was wrong.

use std::collections::BTreeMap;

use aiwatcher_core::{
    EventEnvelope, GlobalPosition, MessageId, ObservabilityContext, RecordedEvent, StreamPosition,
};
use aiwatcher_execution::message::MessageMetadata;
use aiwatcher_execution::plan::{
    CachePolicy, DefinitionKind, DefinitionRevision, PlanEdge, PlanStep, PythonTaskSpec,
    RetryPolicy, RuntimeBinding,
};
use aiwatcher_execution::store::memory::MemoryWorkflowStore;
use aiwatcher_execution::{
    ExecutionHandler, ExecutionId, ExecutionMode, ExecutionOwner, ExecutionPlan, FailureClass, Now,
    StepError, WorkflowCommand, WorkflowEvent, WorkflowMessage,
};
use aiwatcher_projector::readmodel::{ReadModel, RunStatus};
use aiwatcher_projector::workflows::{ExecutionFilter, NodeStatus, WorkflowConfig, WorkflowState};
use time::OffsetDateTime;

const EXECUTION: &str = "exec-house-1";

fn at(seconds: i64) -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(seconds)
}

fn step(id: &str) -> PlanStep {
    PlanStep {
        id: id.to_owned(),
        runtime: RuntimeBinding::PythonTask(PythonTaskSpec {
            task_ref: "stage@1".to_owned(),
            queue: "default".to_owned(),
            params: BTreeMap::new(),
        }),
        inputs: Vec::new(),
        outputs: Vec::new(),
        retry: RetryPolicy::default(),
        timeout_seconds: 600,
        cache: CachePolicy::Never,
    }
}

/// planner's house import: four stages, one pod each.
fn house_import() -> ExecutionPlan {
    ExecutionPlan::seal(
        DefinitionKind::Workflow,
        "house-import".to_owned(),
        DefinitionRevision("ab".repeat(32)),
        vec![
            step("acquire"),
            step("normalize"),
            step("analyze"),
            step("persist"),
        ],
        vec![
            PlanEdge {
                from: "acquire".to_owned(),
                to: "normalize".to_owned(),
            },
            PlanEdge {
                from: "normalize".to_owned(),
                to: "analyze".to_owned(),
            },
            PlanEdge {
                from: "analyze".to_owned(),
                to: "persist".to_owned(),
            },
        ],
    )
}

fn metadata(id: &str, seconds: i64) -> MessageMetadata {
    MessageMetadata {
        schema_version: aiwatcher_execution::message::SCHEMA_VERSION,
        message_id: MessageId::new(id),
        occurred_at: at(seconds),
        correlation_id: aiwatcher_core::CorrelationId::new(EXECUTION),
        causation_id: aiwatcher_core::CausationId::new(id),
        trace_id: None,
        span_id: None,
        step_id: None,
        attempt: None,
    }
}

/// Everything an execution published, in the order the outbox holds it.
async fn published(store: &MemoryWorkflowStore) -> Vec<RecordedEvent> {
    store
        .outbox()
        .await
        .into_iter()
        .enumerate()
        .map(|(index, row)| {
            let envelope: EventEnvelope =
                serde_json::from_value(row.payload).expect("an outbox row holds an envelope");
            let position = index as u64 + 1;
            envelope.record(
                StreamPosition::from(position),
                GlobalPosition::from(position),
                at(position as i64),
                None::<&ObservabilityContext>,
            )
        })
        .collect()
}

async fn runs(events: &[RecordedEvent]) -> ReadModel {
    let model = ReadModel::default();
    for event in events {
        model.apply(event).await;
    }
    model
}

fn fold(events: &[RecordedEvent]) -> WorkflowState {
    let mut state = WorkflowState::default();
    let config = WorkflowConfig::default();
    for event in events {
        state.apply(event, &config);
    }
    state
}

async fn run(inputs: Vec<(&str, i64, WorkflowMessage)>) -> MemoryWorkflowStore {
    let store = MemoryWorkflowStore::new();
    let handler = ExecutionHandler::new(store.clone());
    let execution = ExecutionId::new(EXECUTION);
    for (id, seconds, input) in inputs {
        handler
            .handle(
                &execution,
                input,
                metadata(id, seconds),
                Now::at(at(seconds)),
            )
            .await
            .unwrap_or_else(|error| panic!("{id}: {error}"));
    }
    store
}

fn start() -> WorkflowMessage {
    WorkflowMessage::Command(WorkflowCommand::StartExecution {
        execution_id: ExecutionId::new(EXECUTION),
        plan: Box::new(house_import()),
        owner: ExecutionOwner::Local,
        mode: ExecutionMode::Compiled,
        requested_by: "mk".to_owned(),
        input: BTreeMap::new(),
    })
}

fn completed(step_id: &str) -> WorkflowMessage {
    WorkflowMessage::Event(WorkflowEvent::StepCompleted {
        step_id: step_id.to_owned(),
        attempt: 1,
        outputs: Vec::new(),
        result: None,
    })
}

#[tokio::test]
async fn a_managed_execution_draws_itself_with_every_step_pending_before_one_runs() {
    let store = run(vec![("start", 0, start())]).await;
    let state = fold(&published(&store).await);

    let detail = state
        .execution(EXECUTION)
        .expect("the execution is in the workflow tab");
    assert_eq!(
        detail
            .nodes
            .iter()
            .map(|node| node.node_id.as_str())
            .collect::<Vec<_>>(),
        vec!["acquire", "normalize", "analyze", "persist"],
        "the graph is the plan, in plan order"
    );
    for node in &detail.nodes {
        assert_eq!(
            node.status,
            NodeStatus::Pending,
            "{} ran before anything dispatched it",
            node.node_id
        );
        assert!(node.declared, "{} is not in the declaration", node.node_id);
    }
    assert_eq!(detail.edges.len(), 3, "and it carries the plan's edges");
    // The declaration's version is the plan, which is what a cache key and a
    // rerun both name.
    assert_eq!(
        detail.summary.version.as_deref(),
        Some(house_import().plan_id.to_string().as_str())
    );
}

#[tokio::test]
async fn an_attempt_fills_in_the_node_it_ran_and_leaves_the_rest_pending() {
    let store = run(vec![
        ("start", 0, start()),
        (
            "started",
            10,
            WorkflowMessage::Event(WorkflowEvent::StepStarted {
                step_id: "acquire".to_owned(),
                attempt: 1,
            }),
        ),
        ("done", 200, completed("acquire")),
    ])
    .await;
    let state = fold(&published(&store).await);
    let detail = state.execution(EXECUTION).expect("the execution");

    assert_eq!(detail.nodes[0].status, NodeStatus::Succeeded);
    assert_eq!(detail.nodes[0].attempts, 1);
    assert_eq!(
        detail.nodes[0].publishers,
        vec!["engine".to_owned()],
        "exactly one party publishes an attempt"
    );
    assert!(!detail.nodes[0].has_two_publishers());
    // The next one is dispatched but nothing has reported it running, which is
    // exactly what `Pending` means and why the declaration exists.
    assert_eq!(detail.nodes[1].status, NodeStatus::Pending);
    assert_eq!(detail.summary.nodes_pending, 3);
}

#[tokio::test]
async fn a_node_two_parties_published_is_flagged_rather_than_reconciled() {
    // A managed step whose producer code also opened its own `node()` scope.
    // The attempt count and the duration are then describing two things at
    // once, and the flag is what makes that visible — ADR_0026.
    let store = run(vec![
        ("start", 0, start()),
        (
            "started",
            10,
            WorkflowMessage::Event(WorkflowEvent::StepStarted {
                step_id: "acquire".to_owned(),
                attempt: 1,
            }),
        ),
    ])
    .await;
    let mut events = published(&store).await;

    // The producer's own view of the same node, published under its own run.
    let mut theirs = EventEnvelope::new(
        aiwatcher_core::EventType::StepStarted,
        "run-planner-1",
        at(11),
        aiwatcher_core::Source::new("planner", aiwatcher_core::Sdk::Python),
    );
    theirs.workflow_id = Some("house-import".to_owned());
    theirs.workflow_run_id = Some(EXECUTION.to_owned());
    theirs.data = serde_json::json!({
        "node": "acquire",
        "call_id": "acquire/own",
        "published_by": "producer",
    });
    events.push(theirs.record(
        StreamPosition::from(99u64),
        GlobalPosition::from(99u64),
        at(11),
        None::<&ObservabilityContext>,
    ));

    let detail = fold(&events).execution(EXECUTION).expect("the execution");
    let acquire = detail
        .nodes
        .iter()
        .find(|node| node.node_id == "acquire")
        .expect("the node");
    assert!(
        acquire.has_two_publishers(),
        "publishers were {:?}",
        acquire.publishers
    );
    assert_eq!(acquire.publishers, vec!["engine", "producer"]);
}

#[tokio::test]
async fn a_whole_run_completes_and_the_execution_lists_as_finished() {
    let store = run(vec![
        ("start", 0, start()),
        ("a", 10, completed("acquire")),
        ("b", 20, completed("normalize")),
        ("c", 30, completed("analyze")),
        ("d", 40, completed("persist")),
    ])
    .await;
    let events = published(&store).await;
    let state = fold(&events);

    let detail = state.execution(EXECUTION).expect("the execution");
    assert_eq!(detail.summary.nodes_pending, 0);
    for node in &detail.nodes {
        assert_eq!(node.status, NodeStatus::Succeeded, "{}", node.node_id);
    }

    // And the run's own lifecycle is on the log too, forming no span.
    let lifecycle: Vec<&str> = events
        .iter()
        .map(|event| event.event_type.as_str())
        .filter(|name| name.starts_with("execution."))
        .collect();
    assert_eq!(
        lifecycle,
        vec![
            "execution.requested",
            "execution.started",
            "execution.completed"
        ]
    );
    for event in &events {
        if event.event_type.subject() == aiwatcher_core::Subject::Execution {
            assert!(!event.event_type.forms_span(), "{}", event.event_type);
        }
    }

    assert_eq!(
        state
            .executions(&ExecutionFilter::default(), at(100))
            .executions
            .len(),
        1
    );
}

/// The runs list and the workflow fold must agree about one managed run.
///
/// A managed execution never emits `run.completed` — the engine owns the run
/// and ends it with `execution.completed` (ADR_0026). The runs fold used to
/// read `Subject::Run` only, so the Workflows tab said `succeeded` while
/// Explore span beside it forever.
#[tokio::test]
async fn a_managed_execution_that_finished_is_not_still_running_in_the_runs_list() {
    let store = run(vec![
        ("start", 0, start()),
        ("a", 10, completed("acquire")),
        ("b", 20, completed("normalize")),
        ("c", 30, completed("analyze")),
        ("d", 40, completed("persist")),
    ])
    .await;
    let events = published(&store).await;

    let detail = fold(&events).execution(EXECUTION).expect("the execution");
    assert_eq!(detail.summary.nodes_pending, 0);

    let listed = runs(&events).await.run(EXECUTION).await.expect("the run");
    assert_eq!(listed.summary.status, RunStatus::Succeeded);
    assert!(
        listed.summary.ended_at.is_some(),
        "a finished run has an end, and a duration to draw"
    );
}

/// And the reason survives the crossing: `execution.failed` calls it `reason`,
/// which is a word the runs fold did not know.
#[tokio::test]
async fn a_managed_execution_that_failed_carries_the_engine_s_own_reason() {
    let store = run(vec![
        ("start", 0, start()),
        (
            "a",
            10,
            WorkflowMessage::Event(WorkflowEvent::StepFailed {
                step_id: "acquire".to_owned(),
                attempt: 1,
                error: StepError {
                    message: "the source url answered 404".to_owned(),
                    class: FailureClass::UserCode,
                },
            }),
        ),
    ])
    .await;

    let listed = runs(&published(&store).await)
        .await
        .run(EXECUTION)
        .await
        .expect("the run");
    assert_eq!(listed.summary.status, RunStatus::Failed);
    assert!(
        listed
            .summary
            .error
            .as_deref()
            .is_some_and(|reason| reason.contains("acquire")),
        "the run says which step ended it, got {:?}",
        listed.summary.error
    );
}

#[tokio::test]
async fn a_failed_run_names_the_step_and_leaves_the_rest_pending() {
    let store = run(vec![
        ("start", 0, start()),
        ("a", 10, completed("acquire")),
        (
            "b",
            20,
            WorkflowMessage::Event(WorkflowEvent::StepFailed {
                step_id: "normalize".to_owned(),
                attempt: 1,
                error: StepError::new(FailureClass::UserCode, "column 'area' is missing"),
            }),
        ),
    ])
    .await;
    let events = published(&store).await;
    let detail = fold(&events).execution(EXECUTION).expect("the execution");

    let normalize = &detail.nodes[1];
    assert_eq!(normalize.status, NodeStatus::Failed);
    assert_eq!(normalize.error.as_deref(), Some("column 'area' is missing"));
    // Two stages will never run. They stay `Pending`, which is what ADR_0012
    // says a stage nothing started is — the store holds the reason.
    assert_eq!(detail.nodes[2].status, NodeStatus::Pending);
    assert_eq!(detail.nodes[3].status, NodeStatus::Pending);

    assert!(
        events
            .iter()
            .any(|event| event.event_type.as_str() == "execution.failed")
    );
}

#[tokio::test]
async fn a_retry_is_two_attempts_of_one_node_rather_than_two_nodes() {
    let store = run(vec![
        ("start", 0, start()),
        (
            "fail",
            10,
            WorkflowMessage::Event(WorkflowEvent::StepFailed {
                step_id: "acquire".to_owned(),
                attempt: 1,
                error: StepError::new(FailureClass::Transient, "connection reset"),
            }),
        ),
        (
            "started-2",
            20,
            WorkflowMessage::Event(WorkflowEvent::StepStarted {
                step_id: "acquire".to_owned(),
                attempt: 2,
            }),
        ),
        (
            "done-2",
            30,
            WorkflowMessage::Event(WorkflowEvent::StepCompleted {
                step_id: "acquire".to_owned(),
                attempt: 2,
                outputs: Vec::new(),
                result: None,
            }),
        ),
    ])
    .await;
    let detail = fold(&published(&store).await)
        .execution(EXECUTION)
        .expect("the execution");

    let acquire = &detail.nodes[0];
    assert_eq!(acquire.status, NodeStatus::Succeeded);
    // `call_id` is `<step>/<attempt>`, so the fold counts the second try rather
    // than collapsing both into one node execution.
    assert_eq!(
        acquire.attempts, 1,
        "one *start* was published for attempt 2"
    );
    assert_eq!(detail.nodes.len(), 4, "and no fifth node appeared");
}

#[tokio::test]
async fn a_redelivered_command_publishes_nothing_a_second_time() {
    // The other half: redelivery and restart do not duplicate a workflow
    // decision.
    let store = MemoryWorkflowStore::new();
    let handler = ExecutionHandler::new(store.clone());
    let execution = ExecutionId::new(EXECUTION);

    let first = handler
        .handle(&execution, start(), metadata("start", 0), Now::at(at(0)))
        .await
        .expect("a start");
    assert!(!first.duplicate);
    let after_first = store.outbox().await.len();

    let again = handler
        .handle(&execution, start(), metadata("start", 0), Now::at(at(0)))
        .await
        .expect("a redelivery is not an error");
    assert!(again.duplicate);
    assert!(again.outbox.is_empty());
    assert_eq!(
        store.outbox().await.len(),
        after_first,
        "a redelivery published a second declaration"
    );

    // And the graph is still one execution with four nodes.
    let detail = fold(&published(&store).await)
        .execution(EXECUTION)
        .expect("the execution");
    assert_eq!(detail.nodes.len(), 4);
}

#[tokio::test]
async fn a_facts_ordering_holds_the_declaration_before_the_first_step() {
    // Not cosmetic: a `step.started` for a node the fold has never heard of
    // creates an *observed* node, and the declaration arriving afterwards would
    // be a graph that grew rather than one that was declared.
    let store = run(vec![
        ("start", 0, start()),
        (
            "started",
            10,
            WorkflowMessage::Event(WorkflowEvent::StepStarted {
                step_id: "acquire".to_owned(),
                attempt: 1,
            }),
        ),
    ])
    .await;
    let events = published(&store).await;
    assert_eq!(events[0].event_type.as_str(), "workflow.declared");
    let detail = fold(&events).execution(EXECUTION).expect("the execution");
    assert!(
        detail.nodes.iter().all(|node| node.declared),
        "a node arrived before the declaration did"
    );
}
