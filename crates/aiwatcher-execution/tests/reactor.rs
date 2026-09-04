#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Claiming, running and reporting — with a fake runtime, so every one of these
//! is a rule rather than a service being up.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use aiwatcher_core::{ArtifactKind, ArtifactRef, CausationId, CorrelationId, MessageId};
use aiwatcher_execution::activity::{
    ActivityCommand, ActivityContext, ActivityError, ActivityExecutor, ActivityResult,
    ExecutorRegistry, PriorAttempt,
};
use aiwatcher_execution::message::{MessageMetadata, SCHEMA_VERSION};
use aiwatcher_execution::plan::{
    CachePolicy, DefinitionKind, DefinitionRevision, FlowSourceRef, FlowStepSpec, InputBinding,
    PlanEdge, PlanStep, RetryPolicy, RuntimeBinding,
};
use aiwatcher_execution::store::memory::MemoryWorkflowStore;
use aiwatcher_execution::{
    ExecutionHandler, ExecutionId, ExecutionMode, ExecutionOwner, ExecutionPlan, FailureClass, Now,
    Performed, Reactor, RuntimeKind, StateType, WorkflowCommand, WorkflowMessage, WorkflowStore,
    replay,
};
use async_trait::async_trait;
use time::OffsetDateTime;

fn at(seconds: i64) -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(seconds)
}

/// A step a *reactor* runs. `PythonTask` deliberately is not one: it carries a
/// queue, and a queued attempt belongs to whoever holds that queue.
fn step(id: &str, inputs: Vec<InputBinding>) -> PlanStep {
    PlanStep {
        id: id.to_owned(),
        runtime: RuntimeBinding::FlowPhp(FlowStepSpec {
            script: "data_frame()->read(runs())->run();".to_owned(),
            source: FlowSourceRef {
                dataset: "runs".to_owned(),
                ..FlowSourceRef::default()
            },
            blocks: vec![id.to_owned()],
        }),
        inputs,
        outputs: Vec::new(),
        retry: RetryPolicy::default(),
        timeout_seconds: 60,
        cache: CachePolicy::Never,
    }
}

fn plan() -> ExecutionPlan {
    ExecutionPlan::seal(
        DefinitionKind::Workflow,
        "import".to_owned(),
        DefinitionRevision("ab".repeat(32)),
        vec![
            step("extract", Vec::new()),
            step(
                "load",
                vec![InputBinding::Step {
                    step: "extract".to_owned(),
                    output: "rows".to_owned(),
                }],
            ),
        ],
        vec![PlanEdge {
            from: "extract".to_owned(),
            to: "load".to_owned(),
        }],
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

/// A runtime that answers however the test says, and counts how often it ran.
#[derive(Debug)]
struct Fake {
    answer: Answer,
    ran: AtomicUsize,
    looked_up: AtomicUsize,
    prior: Option<Answer>,
    seen_key: std::sync::Mutex<Option<String>>,
    seen_inputs: std::sync::Mutex<Vec<ArtifactRef>>,
}

#[derive(Clone, Debug)]
enum Answer {
    Rows(String),
    Fails(FailureClass),
    Asks,
}

impl Fake {
    fn new(answer: Answer) -> Arc<Self> {
        Arc::new(Self {
            answer,
            ran: AtomicUsize::new(0),
            looked_up: AtomicUsize::new(0),
            prior: None,
            seen_key: std::sync::Mutex::new(None),
            seen_inputs: std::sync::Mutex::new(Vec::new()),
        })
    }

    fn with_prior(answer: Answer, prior: Answer) -> Arc<Self> {
        Arc::new(Self {
            answer,
            ran: AtomicUsize::new(0),
            looked_up: AtomicUsize::new(0),
            prior: Some(prior),
            seen_key: std::sync::Mutex::new(None),
            seen_inputs: std::sync::Mutex::new(Vec::new()),
        })
    }

    fn result(answer: &Answer) -> Result<ActivityResult, ActivityError> {
        match answer {
            Answer::Rows(digest) => Ok(ActivityResult {
                outputs: vec![
                    ArtifactRef::new("rows", format!("s3://bucket/{digest}"), digest.clone())
                        .of_kind(ArtifactKind::Rows),
                ],
                ..ActivityResult::default()
            }),
            Answer::Fails(class) => Err(ActivityError::new(*class, "the fake said no")),
            Answer::Asks => Ok(ActivityResult {
                awaiting: Some(aiwatcher_execution::state::InputRequest {
                    prompt: "carry on?".to_owned(),
                    role: "admin".to_owned(),
                    choices: vec!["yes".to_owned()],
                    deadline: None,
                }),
                ..ActivityResult::default()
            }),
        }
    }
}

#[async_trait]
impl ActivityExecutor for Fake {
    fn runtime(&self) -> RuntimeKind {
        RuntimeKind::FlowPhp
    }

    async fn execute(
        &self,
        command: &ActivityCommand,
        _context: &ActivityContext,
    ) -> Result<ActivityResult, ActivityError> {
        self.ran.fetch_add(1, Ordering::SeqCst);
        *self.seen_key.lock().expect("the key") = Some(command.idempotency_key());
        self.seen_inputs
            .lock()
            .expect("the inputs")
            .clone_from(&command.inputs);
        Self::result(&self.answer)
    }

    async fn lookup(&self, _command: &ActivityCommand) -> Result<PriorAttempt, ActivityError> {
        self.looked_up.fetch_add(1, Ordering::SeqCst);
        match &self.prior {
            Some(answer) => Ok(PriorAttempt::Done(Box::new(
                Self::result(answer).unwrap_or_default(),
            ))),
            None => Ok(PriorAttempt::Absent),
        }
    }
}

async fn started(store: &MemoryWorkflowStore) {
    ExecutionHandler::new(store.clone())
        .handle(&execution(), start(), metadata("m-1"), Now::at(at(0)))
        .await
        .expect("a start");
}

fn reactor(store: MemoryWorkflowStore, fake: Arc<Fake>) -> Reactor<MemoryWorkflowStore> {
    Reactor::new(
        ExecutionHandler::new(store),
        ExecutorRegistry::new().with(fake),
        "reactor-1".to_owned(),
    )
}

#[tokio::test]
async fn a_reactor_claims_runs_and_reports_and_the_decider_moves_the_run_on() {
    let store = MemoryWorkflowStore::new();
    started(&store).await;
    let fake = Fake::new(Answer::Rows("ab".repeat(32)));
    let reactor = reactor(store.clone(), Arc::clone(&fake));

    assert_eq!(
        reactor.poll_once(at(10)).await.expect("a poll"),
        Performed::Reported {
            step_id: "extract".to_owned(),
            attempt: 1,
            succeeded: true,
        }
    );
    assert_eq!(fake.ran.load(Ordering::SeqCst), 1);
    // The key the runtime was called with is the one a lookup would ask by.
    assert_eq!(
        fake.seen_key.lock().expect("the key").as_deref(),
        Some("exec-1/extract/1")
    );

    // The reactor decided nothing: the decider read the completion and
    // dispatched what follows.
    let state = replay(store.load(&execution()).await.expect("a load").events());
    let run = state.active().expect("an execution");
    assert_eq!(
        run.step("extract").expect("the step").state.state_type,
        StateType::Completed
    );
    assert_eq!(
        run.step("load").expect("the step").state.state_type,
        StateType::Pending,
        "the next step was dispatched by the decider, not by the reactor"
    );
}

#[tokio::test]
async fn the_next_step_is_handed_what_the_one_before_it_produced() {
    let store = MemoryWorkflowStore::new();
    started(&store).await;
    let fake = Fake::new(Answer::Rows("ab".repeat(32)));
    let reactor = reactor(store.clone(), Arc::clone(&fake));

    reactor.poll_once(at(10)).await.expect("the first step");
    reactor.poll_once(at(20)).await.expect("the second step");

    let inputs = fake.seen_inputs.lock().expect("the inputs").clone();
    assert_eq!(inputs.len(), 1, "the second step read one artifact");
    assert_eq!(inputs[0].digest, "ab".repeat(32));
    assert_eq!(inputs[0].kind, ArtifactKind::Rows);
}

#[tokio::test]
async fn a_reactor_with_nothing_to_do_says_so_rather_than_blocking() {
    let store = MemoryWorkflowStore::new();
    let fake = Fake::new(Answer::Rows("ab".repeat(32)));
    assert_eq!(
        reactor(store, fake).poll_once(at(0)).await.expect("a poll"),
        Performed::Idle
    );
}

#[tokio::test]
async fn a_failure_is_reported_and_the_decider_owns_the_retry() {
    // A reactor that scheduled its own retry would be the second orchestrator
    // ADR_0025 refuses.
    let store = MemoryWorkflowStore::new();
    started(&store).await;
    let fake = Fake::new(Answer::Fails(FailureClass::Transient));
    let reactor = reactor(store.clone(), Arc::clone(&fake));

    assert_eq!(
        reactor.poll_once(at(10)).await.expect("a poll"),
        Performed::Reported {
            step_id: "extract".to_owned(),
            attempt: 1,
            succeeded: false,
        }
    );
    let state = replay(store.load(&execution()).await.expect("a load").events());
    assert_eq!(
        state
            .active()
            .expect("an execution")
            .step("extract")
            .expect("the step")
            .current_attempt,
        2,
        "the decider scheduled attempt 2"
    );
}

#[tokio::test]
async fn a_deterministic_failure_ends_the_run_rather_than_looping() {
    let store = MemoryWorkflowStore::new();
    started(&store).await;
    let fake = Fake::new(Answer::Fails(FailureClass::UserCode));
    let reactor = reactor(store.clone(), Arc::clone(&fake));

    reactor.poll_once(at(10)).await.expect("a poll");
    assert_eq!(
        reactor.poll_once(at(20)).await.expect("a second poll"),
        Performed::Idle,
        "nothing is claimable: the run failed"
    );
    assert_eq!(fake.ran.load(Ordering::SeqCst), 1);
    assert_eq!(
        replay(store.load(&execution()).await.expect("a load").events())
            .active()
            .expect("an execution")
            .state
            .state_type,
        StateType::Failed
    );
}

#[tokio::test]
async fn a_takeover_asks_the_runtime_before_it_runs_the_work_again() {
    // A timeout proves nothing about whether the runtime finished. Running it
    // again without asking is the duplicate side effect the idempotency key
    // exists to prevent.
    let store = MemoryWorkflowStore::new();
    started(&store).await;
    let fake = Fake::with_prior(Answer::Rows("cd".repeat(32)), Answer::Rows("ab".repeat(32)));
    let first = reactor(store.clone(), Arc::clone(&fake));
    first
        .handler()
        .store()
        .claim_attempt(
            &ExecutorRegistry::new()
                .with(Arc::clone(&fake) as Arc<dyn ActivityExecutor>)
                .claim_filter(),
            "reactor-that-died",
            at(0),
        )
        .await
        .expect("a claim")
        .expect("the attempt");

    // Long enough that the dead reactor's lease has expired.
    let past = at(aiwatcher_jobs::LEASE_SECONDS + 1);
    let performed = first.poll_once(past).await.expect("a takeover");
    assert!(matches!(
        performed,
        Performed::Reported {
            succeeded: true,
            ..
        }
    ));
    assert_eq!(fake.looked_up.load(Ordering::SeqCst), 1);
    assert_eq!(
        fake.ran.load(Ordering::SeqCst),
        0,
        "the runtime had already finished, so it was not asked to run again"
    );

    // And what the previous attempt produced is what the workflow recorded.
    let state = replay(store.load(&execution()).await.expect("a load").events());
    assert_eq!(
        state
            .active()
            .expect("an execution")
            .step("extract")
            .expect("the step")
            .outputs[0]
            .digest,
        "ab".repeat(32)
    );
}

#[tokio::test]
async fn a_fresh_attempt_does_not_ask_and_simply_runs() {
    let store = MemoryWorkflowStore::new();
    started(&store).await;
    let fake = Fake::new(Answer::Rows("ab".repeat(32)));
    reactor(store, Arc::clone(&fake))
        .poll_once(at(10))
        .await
        .expect("a poll");
    assert_eq!(
        fake.looked_up.load(Ordering::SeqCst),
        0,
        "nobody held this attempt before, so there is nothing to ask about"
    );
}

/// A runtime that, while it is working, lets somebody else take the lease.
#[derive(Debug)]
struct StealsItsOwnLease {
    store: MemoryWorkflowStore,
}

#[async_trait]
impl ActivityExecutor for StealsItsOwnLease {
    fn runtime(&self) -> RuntimeKind {
        RuntimeKind::FlowPhp
    }

    async fn execute(
        &self,
        command: &ActivityCommand,
        _context: &ActivityContext,
    ) -> Result<ActivityResult, ActivityError> {
        // Long enough that the caller's lease is gone, and somebody else picks
        // it up — a pod that was drained and whose work was taken over.
        let past = at(aiwatcher_jobs::LEASE_SECONDS + 1);
        self.store
            .claim_attempt(
                &aiwatcher_execution::ClaimFilter::for_runtimes(&[RuntimeKind::FlowPhp]),
                "the-replacement",
                past,
            )
            .await
            .expect("a takeover")
            .expect("the attempt");
        let _ = command;
        Ok(ActivityResult::default())
    }
}

#[tokio::test]
async fn a_reactor_that_lost_its_lease_stops_rather_than_writing_beside_its_replacement() {
    // ADR_0022's rule for an export shard, in the place it matters most: a
    // stream with two answers for one attempt is a run nothing can explain.
    // The work is lost on purpose — the alternative is worse.
    let store = MemoryWorkflowStore::new();
    started(&store).await;
    let reactor = Reactor::new(
        ExecutionHandler::new(store.clone()),
        ExecutorRegistry::new().with(Arc::new(StealsItsOwnLease {
            store: store.clone(),
        })),
        "reactor-that-was-drained".to_owned(),
    );

    assert_eq!(
        reactor.poll_once(at(0)).await.expect("a poll"),
        Performed::LeaseLost {
            step_id: "extract".to_owned(),
            attempt: 1,
        }
    );

    // The step is still running as far as the workflow is concerned: no
    // completion was written, so the replacement's answer is the only one that
    // will be.
    let state = replay(store.load(&execution()).await.expect("a load").events());
    assert_eq!(
        state
            .active()
            .expect("an execution")
            .step("extract")
            .expect("the step")
            .state
            .state_type,
        StateType::Running
    );
}

#[tokio::test]
async fn a_reactor_never_claims_a_runtime_it_does_not_hold() {
    // The filter is built from what is registered, so an attempt for a runtime
    // this process cannot run is never claimed at all — which is better than
    // claiming it and releasing it, because a claim it cannot serve is a lease
    // nobody else may take for five minutes.
    #[derive(Debug)]
    struct Flow;

    #[async_trait]
    impl ActivityExecutor for Flow {
        fn runtime(&self) -> RuntimeKind {
            RuntimeKind::Marimo
        }

        async fn execute(
            &self,
            _command: &ActivityCommand,
            _context: &ActivityContext,
        ) -> Result<ActivityResult, ActivityError> {
            unreachable!("this executor is never reached")
        }
    }

    let store = MemoryWorkflowStore::new();
    started(&store).await;
    let reactor = Reactor::new(
        ExecutionHandler::new(store),
        ExecutorRegistry::new().with(Arc::new(Flow)),
        "marimo-reactor".to_owned(),
    );
    assert_eq!(
        reactor.poll_once(at(10)).await.expect("a poll"),
        Performed::Idle
    );
}

#[tokio::test]
async fn a_step_that_stopped_to_ask_reports_the_question_and_releases_its_claim() {
    let store = MemoryWorkflowStore::new();
    started(&store).await;
    let fake = Fake::new(Answer::Asks);
    reactor(store.clone(), fake)
        .poll_once(at(10))
        .await
        .expect("a poll");

    let state = replay(store.load(&execution()).await.expect("a load").events());
    let step = state
        .active()
        .expect("an execution")
        .step("extract")
        .expect("the step")
        .clone();
    assert_eq!(step.state.state_type, StateType::AwaitingInput);
    assert_eq!(
        step.awaiting.expect("a question").role,
        "admin",
        "the role that may answer travels with the question"
    );
}

#[tokio::test]
async fn a_reactor_holding_no_executors_claims_nothing() {
    let store = MemoryWorkflowStore::new();
    started(&store).await;
    let reactor = Reactor::new(
        ExecutionHandler::new(store.clone()),
        ExecutorRegistry::new(),
        "empty".to_owned(),
    );
    assert_eq!(
        reactor.poll_once(at(10)).await.expect("a poll"),
        Performed::Idle
    );
    // And the attempt is still there for a process that can run it.
    assert!(
        store
            .attempt(&aiwatcher_execution::AttemptKey::new(
                execution(),
                "extract",
                1
            ))
            .await
            .expect("a read")
            .expect("the row")
            .is_claimable(at(10))
    );
}
