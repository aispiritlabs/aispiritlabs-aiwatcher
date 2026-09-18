#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! What a project execution is, from the start that creates it to the paths
//! that may not touch it.
//!
//! `tests/store_contract.rs` proves the *store* keeps the boundary, adapter by
//! adapter. This proves the things above it: that a start writes the ownership
//! rather than a caller writing it separately, that the handler and the reactor
//! a deployment already runs reach nothing of a project's run, and that the id
//! two projects derive from one idempotency key is two ids.
//!
//! Nothing here opens a route. There is no project `/start`, and these
//! construct the scoped store directly — which is exactly what a future
//! dispatcher will do once it has somewhere trusted to read the scope from.

use std::collections::BTreeMap;
use std::sync::Arc;

use time::OffsetDateTime;

use aiwatcher_core::MessageId;
use aiwatcher_datasets::QueryEngine;
use aiwatcher_execution::message::PayloadDefault;
use aiwatcher_execution::plan::{
    CachePolicy, DefinitionKind, DefinitionRevision, ExecutionPlan, FlowSourceRef, FlowStepSpec,
    PlanStep, RetryPolicy, RuntimeBinding, RuntimeKind,
};
use aiwatcher_execution::store::memory::MemoryWorkflowStore;
use aiwatcher_execution::{
    Decider, ExecutionHandler, ExecutionId, ExecutionOwnership, Executions, ExecutorRegistry,
    MessageMetadata, Now, Performed, ProjectStart, Reactor, RunIdentity, StartRun, StoreError,
    WorkflowCommand, WorkflowMessage, WorkflowStore,
};
use aiwatcher_iam::{OrganizationId, Principal, ProjectId, ProjectScope};

fn scope() -> ProjectScope {
    ProjectScope {
        organization: OrganizationId::new(),
        project: ProjectId::new(),
    }
}

fn principal(subject: &str) -> Principal {
    Principal::new("https://id.example", subject).expect("a principal")
}

fn plan() -> ExecutionPlan {
    ExecutionPlan::seal(
        DefinitionKind::CurationPipeline,
        "nightly".to_owned(),
        DefinitionRevision("ab".repeat(32)),
        vec![PlanStep {
            id: "read".to_owned(),
            runtime: RuntimeBinding::FlowPhp(FlowStepSpec {
                script: "data_frame()->read(runs)".to_owned(),
                source: FlowSourceRef::default(),
                blocks: Vec::new(),
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

/// What a run asks for, on whichever side of the boundary it is on.
fn run(project: Option<ProjectStart>) -> StartRun {
    StartRun {
        identity: RunIdentity::Key("nightly".to_owned()),
        parameters: BTreeMap::new(),
        requested_by: "somebody".to_owned(),
        decided_by: Decider::Local,
        payloads: None,
        project,
    }
}

/// The use case, wired with nothing it does not need: this starts a plan the
/// caller already has, so neither registry is read.
fn executions<S: WorkflowStore>(handler: &ExecutionHandler<S>) -> Executions<'_, S> {
    Executions {
        handler: Some(handler),
        pipelines: None,
        workflows: None,
        payloads: PayloadDefault::default(),
        archive: false,
        engine: QueryEngine::default(),
        query_timeout_seconds: None,
        notify: None,
    }
}

/// Start one project execution through the production path, and hand back what
/// it took to do it.
async fn started(
    shared: &MemoryWorkflowStore,
    scope: ProjectScope,
    subject: &str,
) -> (ExecutionId, Arc<dyn WorkflowStore>, ExecutionOwnership) {
    let bound = shared.for_project(scope).expect("a project store");
    let start = ProjectStart::new(scope, principal(subject));
    let handler = ExecutionHandler::new(Arc::clone(&bound));
    let started = executions(&handler)
        .start(plan(), run(Some(start.clone())))
        .await
        .expect("a project start");
    (
        started.execution_id,
        bound,
        ExecutionOwnership::of(&start, &plan()),
    )
}

#[tokio::test]
async fn a_project_start_writes_its_owner_in_the_transaction_that_creates_the_run() {
    let shared = MemoryWorkflowStore::new();
    let scope = scope();
    let (execution, bound, ownership) = started(&shared, scope, "alice").await;

    assert_eq!(
        bound.ownership(&execution).await.expect("a read"),
        Some(ownership.clone()),
        "the owner is the record the start established"
    );
    // And it names what was started, so a dispatcher can decide whether it may
    // run this at all before it reads a message of the stream.
    assert_eq!(ownership.definition.plan_id, plan().plan_id);
    assert_eq!(ownership.definition.name, "nightly");
    assert_eq!(ownership.principal, principal("alice"));
    assert_eq!(ownership.scope, scope);
}

#[tokio::test]
async fn the_handler_a_deployment_already_runs_reaches_nothing_of_a_project_s_run() {
    let shared = MemoryWorkflowStore::new();
    let (execution, _bound, _) = started(&shared, scope(), "alice").await;

    // The unscoped handler is what every route, the schedule tick and the
    // reactor in this binary hold. A command from it is refused, not applied.
    let global = ExecutionHandler::new(shared.clone());
    let refused = global
        .handle(
            &execution,
            WorkflowMessage::Command(WorkflowCommand::CancelExecution {
                reason: "because".to_owned(),
            }),
            MessageMetadata::caused_by(
                &execution,
                &MessageId::new("cancel"),
                MessageId::new("cancel"),
                OffsetDateTime::UNIX_EPOCH,
            ),
            Now::at(OffsetDateTime::UNIX_EPOCH),
        )
        .await
        .expect_err("an unscoped command on a project's execution");
    assert!(
        matches!(
            refused,
            aiwatcher_execution::HandleError::Store(StoreError::OutOfScope(_))
        ),
        "{refused}"
    );
    assert!(
        refused.says_the_same_next_time(),
        "a boundary is not a bad moment a scheduler should come back for"
    );
}

#[tokio::test]
async fn the_reactor_a_deployment_already_runs_claims_nothing_of_a_project_s_run() {
    let shared = MemoryWorkflowStore::new();
    let (_execution, bound, _) = started(&shared, scope(), "alice").await;

    // The start dispatched its first step, so there is a claimable row — for
    // the project's own reactor and for nothing else.
    let global = Reactor::new(
        ExecutionHandler::new(shared.clone()),
        ExecutorRegistry::new().with(Arc::new(Refuses)),
        "global-reactor".to_owned(),
    );
    assert!(
        matches!(
            global
                .poll_once(OffsetDateTime::UNIX_EPOCH)
                .await
                .expect("a pass"),
            Performed::Idle
        ),
        "the unscoped reactor took a project's attempt"
    );

    let theirs = Reactor::new(
        ExecutionHandler::new(Arc::clone(&bound)),
        ExecutorRegistry::new().with(Arc::new(Refuses)),
        "project-reactor".to_owned(),
    );
    assert!(
        !matches!(
            theirs
                .poll_once(OffsetDateTime::UNIX_EPOCH)
                .await
                .expect("a pass"),
            Performed::Idle
        ),
        "the project's own reactor found nothing to do"
    );
}

#[tokio::test]
async fn one_idempotency_key_in_two_projects_is_two_runs_with_two_owners() {
    let shared = MemoryWorkflowStore::new();
    let (one, other) = (scope(), scope());
    let (mine, my_store, my_owner) = started(&shared, one, "alice").await;
    let (theirs, their_store, their_owner) = started(&shared, other, "bob").await;

    assert_ne!(mine, theirs, "one key in two projects is two executions");
    assert_eq!(my_owner.scope, one);
    assert_eq!(their_owner.scope, other);

    // And neither reaches the other's, by id or by key.
    let refused = my_store
        .load(&theirs)
        .await
        .expect_err("another project's execution");
    assert!(matches!(refused, StoreError::OutOfScope(_)), "{refused}");
    let refused = their_store
        .load(&mine)
        .await
        .expect_err("another project's execution");
    assert!(matches!(refused, StoreError::OutOfScope(_)), "{refused}");
}

#[tokio::test]
async fn a_second_start_naming_another_principal_is_refused_rather_than_deduplicated() {
    let shared = MemoryWorkflowStore::new();
    let scope = scope();
    let (execution, bound, held) = started(&shared, scope, "alice").await;

    // Same key, same project, same plan — so the same execution id, and the
    // same derived message id, which is a redelivery by every other measure —
    // and a different principal. Answering it `Duplicate` would tell the second
    // caller the run was theirs.
    let handler = ExecutionHandler::new(Arc::clone(&bound));
    let refused = executions(&handler)
        .start(
            plan(),
            run(Some(ProjectStart::new(scope, principal("mallory")))),
        )
        .await
        .expect_err("a second owner");
    assert!(
        matches!(
            refused,
            aiwatcher_execution::StartRefused::Command(aiwatcher_execution::HandleError::Store(
                StoreError::OwnershipConflict { .. }
            ))
        ),
        "{refused}"
    );
    assert_eq!(
        bound.ownership(&execution).await.expect("a read"),
        Some(held),
        "the record did not move"
    );
}

/// An executor that refuses everything, so a claimed attempt goes no further.
///
/// What is under test is which attempts a reactor *reaches*, and running a Flow
/// query would put a service in a test about a boundary.
#[derive(Debug)]
struct Refuses;

#[async_trait::async_trait]
impl aiwatcher_execution::ActivityExecutor for Refuses {
    fn runtime(&self) -> RuntimeKind {
        RuntimeKind::FlowPhp
    }

    async fn execute(
        &self,
        _command: &aiwatcher_execution::ActivityCommand,
        _context: &aiwatcher_execution::ActivityContext,
    ) -> Result<aiwatcher_execution::ActivityResult, aiwatcher_execution::ActivityError> {
        Err(aiwatcher_execution::ActivityError::new(
            aiwatcher_execution::FailureClass::UserCode,
            "this executor exists to be reached, not to run",
        ))
    }
}
