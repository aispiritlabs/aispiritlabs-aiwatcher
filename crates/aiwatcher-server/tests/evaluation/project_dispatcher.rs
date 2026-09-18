//! The gate itself: a project's attempt claimed, authorized, run and published
//! through `ProjectDispatcher`, against a real IAM policy and a real store.
//!
//! `aiwatcher-execution/tests/authority.rs` proves the reactor's ordering with a
//! stub. This proves what the ordering is made of: the scope and the principal
//! come out of `store.ownership`, the grant is the one IAM holds now, both
//! halves of artifact storage are one project's, and nothing of this is
//! registered where the deployment's own reactors would find it.
use super::{fixture::Fixture, project_declarations::seed, project_evidence::source, *};
use aiwatcher_datasets::QueryEngine;
use aiwatcher_execution::message::PayloadDefault;
use aiwatcher_execution::plan::{
    CachePolicy, DefinitionKind, DefinitionRevision, ExecutionPlan, PlanStep, RetryPolicy,
    ScoreEvaluationSpec,
};
use aiwatcher_execution::store::memory::MemoryWorkflowStore;
use aiwatcher_execution::{
    Decider, ExecutionHandler, ExecutionId, Executions, ExecutorRegistry, FailureClass, Performed,
    ProjectStart, Reactor, RunIdentity, RuntimeBinding, RuntimeKind, StartRun, StateType,
    WorkflowStore, replay,
};
use aiwatcher_iam::{
    Change, Command, GrantId, GrantWindow, Grantee, IamStore, OrganizationId, Principal,
    ProjectRole, ProjectScope,
};
use aiwatcher_prompts::adapters::fs::FileObjectStore;
use aiwatcher_server::execution::project::ProjectDispatcher;

/// An IAM store that answers whatever it is told to, over the real policy.
///
/// Only one question is asked of it that a memory policy cannot answer: what a
/// *backend that is down* does to a pass. `Error::Backend` is the one an
/// adapter raises when its database refused, and the rule under test is that it
/// is never read as a yes.
#[derive(Debug)]
struct Flaky {
    inner: Arc<aiwatcher_iam::memory::MemoryIamStore>,
    down: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl IamStore for Flaky {
    async fn create_organization(
        &self,
        owner: &Principal,
        name: &str,
    ) -> aiwatcher_iam::Result<aiwatcher_iam::Organization> {
        self.inner.create_organization(owner, name).await
    }
    async fn audit(
        &self,
        organization: OrganizationId,
        actor: &Principal,
        after: i64,
        limit: usize,
    ) -> aiwatcher_iam::Result<Vec<aiwatcher_iam::AuditEntry>> {
        self.inner.audit(organization, actor, after, limit).await
    }
    async fn organizations(
        &self,
        actor: &Principal,
    ) -> aiwatcher_iam::Result<Vec<aiwatcher_iam::Organization>> {
        self.inner.organizations(actor).await
    }
    async fn apply(
        &self,
        organization: OrganizationId,
        actor: &Principal,
        command: Command,
    ) -> aiwatcher_iam::Result<Change> {
        self.inner.apply(organization, actor, command).await
    }
    async fn projects(
        &self,
        organization: OrganizationId,
        actor: &Principal,
    ) -> aiwatcher_iam::Result<Vec<aiwatcher_iam::ProjectAccess>> {
        self.inner.projects(organization, actor).await
    }
    async fn roster(
        &self,
        organization: OrganizationId,
        actor: &Principal,
    ) -> aiwatcher_iam::Result<aiwatcher_iam::Roster> {
        self.inner.roster(organization, actor).await
    }
    async fn project_grants(
        &self,
        scope: ProjectScope,
        actor: &Principal,
    ) -> aiwatcher_iam::Result<Vec<aiwatcher_iam::Grant>> {
        self.inner.project_grants(scope, actor).await
    }
    async fn access(
        &self,
        scope: ProjectScope,
        actor: &Principal,
    ) -> aiwatcher_iam::Result<aiwatcher_iam::ProjectAccess> {
        if self.down.load(Ordering::SeqCst) {
            return Err(aiwatcher_iam::Error::Backend("the pool is gone".into()));
        }
        self.inner.access(scope, actor).await
    }
}

/// One project, one declared measurement, one started execution, one dispatcher.
struct Dispatched {
    _fixture: Fixture,
    iam: Arc<Flaky>,
    actor: Principal,
    scope: ProjectScope,
    grant: GrantId,
    root: Registry,
    local: Registry,
    shared: MemoryWorkflowStore,
    objects: Arc<dyn ObjectStore>,
    bound: Arc<dyn WorkflowStore>,
    execution: ExecutionId,
    dispatcher: ProjectDispatcher,
    result: String,
}

impl Dispatched {
    async fn new(name: &str) -> Self {
        let fixture = Fixture::new(name).await;
        let store: Arc<dyn ObjectStore> = Arc::new(
            FileObjectStore::open(&fixture.root.join("store"))
                .await
                .unwrap(),
        );
        let iam = Arc::new(Flaky {
            inner: Arc::new(aiwatcher_iam::memory::MemoryIamStore::default()),
            down: std::sync::atomic::AtomicBool::new(false),
        });
        let actor = Principal::new("issuer", "owner").unwrap();
        let organization = iam
            .create_organization(&actor, "organization")
            .await
            .unwrap();
        let Change::ProjectCreated(project) = iam
            .apply(
                organization.id,
                &actor,
                Command::CreateProject {
                    name: "project".into(),
                },
            )
            .await
            .unwrap()
        else {
            panic!("project")
        };
        let scope = project.scope;
        // The creator's own grant goes, so what admits the run is the explicit
        // one below and never the fact that this principal made the project.
        for grant in iam.access(scope, &actor).await.unwrap().grants {
            iam.apply(
                scope.organization,
                &actor,
                Command::RevokeGrant {
                    project: scope.project,
                    grant: grant.grant.id,
                },
            )
            .await
            .unwrap();
        }
        let Change::GrantCreated(grant) = iam
            .apply(
                organization.id,
                &actor,
                Command::Grant {
                    project: scope.project,
                    grantee: Grantee::User(actor.clone()),
                    role: ProjectRole::Editor,
                    window: GrantWindow::permanent(0),
                },
            )
            .await
            .unwrap()
        else {
            panic!("grant")
        };

        let owner = Arc::new(source(store.clone(), &fixture));
        let root = Registry::new(store.clone(), owner.clone(), Default::default()).unwrap();
        let local = root.for_project_evidence(scope).unwrap();
        aiwatcher_datasets::Registry::new(store.clone(), "datasets")
            .for_project(scope)
            .unwrap()
            .publish(fixture.rows.clone())
            .await
            .unwrap();
        let run = seed(&local, &fixture).await;
        let declaration = local
            .declare_scoring_run(&run, "declaration-author", 10)
            .await
            .unwrap();
        let view = local
            .scoring_run_view(&declaration.id)
            .await
            .unwrap()
            .unwrap();
        let bundles = owner.for_project(scope).unwrap();
        bundles
            .stage(
                &view.approval_id,
                "manifest.json",
                serde_json::to_vec(&view.manifest).unwrap(),
            )
            .await
            .unwrap();
        for name in ["responses.py", "generation.json", "workflow.json"] {
            bundles
                .stage(
                    &view.approval_id,
                    name,
                    tokio::fs::read(fixture.root.join(name)).await.unwrap(),
                )
                .await
                .unwrap();
        }
        local
            .approve(&view.manifest, "project-admin", 11)
            .await
            .unwrap();

        // The production start path: the ownership is written in the
        // transaction that creates the execution, from a `ProjectStart` this
        // test builds the way trusted wiring would.
        let shared = MemoryWorkflowStore::new();
        let bound = shared.for_project(scope).unwrap();
        let handler = ExecutionHandler::new(Arc::clone(&bound));
        let started = Executions {
            handler: Some(&handler),
            pipelines: None,
            workflows: None,
            payloads: PayloadDefault::default(),
            archive: false,
            engine: QueryEngine::default(),
            query_timeout_seconds: None,
            notify: None,
        }
        .start(
            plan(&declaration.id, &run.evaluation_id),
            StartRun {
                identity: RunIdentity::Key(run.evaluation_id.clone()),
                parameters: BTreeMap::new(),
                requested_by: "whoever pressed start".to_owned(),
                decided_by: Decider::Local,
                payloads: None,
                project: Some(ProjectStart::new(scope, actor.clone())),
            },
        )
        .await
        .unwrap();

        let iam_port: Arc<dyn IamStore> = iam.clone();
        let workflow: Arc<dyn WorkflowStore> = Arc::new(shared.clone());
        let dispatcher = ProjectDispatcher::bind(
            &workflow,
            &store,
            &Arc::new(root.clone()),
            &iam_port,
            scope,
            "project-dispatcher".to_owned(),
        )
        .unwrap();

        Self {
            _fixture: fixture,
            iam,
            actor,
            scope,
            grant: grant.id,
            root,
            local,
            shared,
            objects: store,
            bound,
            execution: started.execution_id,
            dispatcher,
            result: run.evaluation_id,
        }
    }

    async fn revoke(&self) {
        self.iam
            .apply(
                self.scope.organization,
                &self.actor,
                Command::RevokeGrant {
                    project: self.scope.project,
                    grant: self.grant,
                },
            )
            .await
            .unwrap();
    }

    /// What the run's one step is now, and the class it failed with if it did.
    async fn step(&self) -> (StateType, Option<FailureClass>) {
        let slice = self.bound.load(&self.execution).await.unwrap();
        let run = replay(slice.events()).active().cloned().unwrap();
        let step = run.step("score").unwrap().clone();
        let class = step
            .attempts
            .iter()
            .find_map(|record| record.error.as_ref())
            .map(|error| error.class);
        (step.state.state_type, class)
    }

    async fn published(&self, registry: &Registry) -> bool {
        registry
            .get(
                &self.result,
                "reader",
                time::OffsetDateTime::now_utc().unix_timestamp(),
            )
            .await
            .unwrap()
            .is_some()
    }
}

fn plan(declaration: &str, evaluation: &str) -> ExecutionPlan {
    ExecutionPlan::seal(
        DefinitionKind::Evaluation,
        evaluation.to_owned(),
        DefinitionRevision(declaration.to_owned()),
        vec![PlanStep {
            id: "score".to_owned(),
            runtime: RuntimeBinding::ScoreEvaluation(ScoreEvaluationSpec {
                declaration: declaration.to_owned(),
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

fn now() -> time::OffsetDateTime {
    time::OffsetDateTime::now_utc()
}

#[tokio::test]
async fn a_project_s_attempt_is_claimed_authorized_run_and_published_only_in_its_project() {
    let f = Dispatched::new("dispatcher-happy-path").await;

    let performed = f.dispatcher.poll_once(now()).await.unwrap();
    assert!(
        matches!(
            performed,
            Performed::Reported {
                succeeded: true,
                ..
            }
        ),
        "{performed:?}"
    );
    assert_eq!(f.step().await.0, StateType::Completed);
    assert!(f.published(&f.local).await, "the project holds no result");
    assert!(
        !f.published(&f.root).await,
        "a project's measurement was published where the deployment reads"
    );
    // A second pass finds nothing: the run settled, and the dispatcher claims
    // by the same rules every other reactor does.
    assert!(matches!(
        f.dispatcher.poll_once(now()).await.unwrap(),
        Performed::Idle
    ));
}

#[tokio::test]
async fn the_deployment_s_own_reactor_claims_nothing_of_it() {
    let f = Dispatched::new("dispatcher-unscoped-reactor").await;
    // The unscoped store is what every reactor, the launcher, the timer tick,
    // the outbox publisher and the retention sweep in this binary hold.
    let global = Reactor::new(
        ExecutionHandler::new(f.shared.clone()),
        ExecutorRegistry::new().with(Arc::new(Refuses)),
        "global-reactor".to_owned(),
    );
    assert!(matches!(
        global.poll_once(now()).await.unwrap(),
        Performed::Idle
    ));
    // And it was there to be claimed all along.
    assert!(!matches!(
        f.dispatcher.poll_once(now()).await.unwrap(),
        Performed::Idle
    ));
}

#[tokio::test]
async fn a_grant_revoked_before_the_claim_ends_the_run_and_publishes_nothing() {
    let f = Dispatched::new("dispatcher-revoked").await;
    f.revoke().await;

    let performed = f.dispatcher.poll_once(now()).await.unwrap();
    assert!(
        matches!(
            performed,
            Performed::Reported {
                succeeded: false,
                ..
            }
        ),
        "{performed:?}"
    );
    assert_eq!(
        f.step().await,
        (StateType::Failed, Some(FailureClass::Policy))
    );
    assert!(!f.published(&f.local).await);
    assert!(!f.published(&f.root).await);
    // No start was reported either: the refusal came before the plan was read.
    let slice = f.bound.load(&f.execution).await.unwrap();
    assert!(!slice.events().any(|event| matches!(
        event,
        aiwatcher_execution::WorkflowEvent::StepStarted { .. }
    )));
}

#[tokio::test]
async fn an_iam_backend_that_is_down_is_not_consent_and_the_attempt_survives_it() {
    let f = Dispatched::new("dispatcher-iam-down").await;
    f.iam.down.store(true, Ordering::SeqCst);

    let performed = f.dispatcher.poll_once(now()).await.unwrap();
    assert!(
        matches!(performed, Performed::Released { .. }),
        "an unreachable IAM backend ran a project's work: {performed:?}"
    );
    assert!(!f.published(&f.local).await);
    assert_eq!(f.step().await, (StateType::Pending, None));

    // The lease is what holds the row until it lapses, so a pass while IAM is
    // still down finds nothing — and once it answers again the work runs.
    f.iam.down.store(false, Ordering::SeqCst);
    let later = now() + time::Duration::seconds(aiwatcher_jobs::LEASE_SECONDS + 1);
    let performed = f.dispatcher.poll_once(later).await.unwrap();
    assert!(
        matches!(
            performed,
            Performed::Reported {
                succeeded: true,
                ..
            }
        ),
        "{performed:?}"
    );
    assert!(f.published(&f.local).await);
}

#[tokio::test]
async fn the_dispatcher_holds_one_project_s_bytes_and_one_project_s_index() {
    let f = Dispatched::new("dispatcher-artifacts").await;
    let pair = f.dispatcher.artifacts();
    assert_eq!(pair.scope(), f.scope);
    assert_eq!(f.dispatcher.scope(), f.scope);
    let expected = format!(
        "artifacts/scopes/{}/{}/registry",
        f.scope.organization.0, f.scope.project.0
    );
    assert_eq!(pair.artifacts().prefix(), expected);
    // Both halves, from one scope, through one door: `bind` compares the two
    // prefixes itself, and a scoped byte store beside the deployment-wide
    // catalog is a project's outputs in the index a cache lookup for anybody
    // reads.
    assert!(
        aiwatcher_execution::ObjectArtifactCatalog::new(Arc::clone(&f.objects))
            .for_project(f.scope)
            .is_ok_and(|catalog| catalog.prefix() == expected)
    );
}

#[tokio::test]
async fn a_measurement_whose_card_asks_a_judge_is_claimed_by_nobody_that_holds_none() {
    // What the dispatcher may claim is what it holds a client for, as it is for
    // every other reactor: an attempt of a runtime nothing here performs waits
    // for a process that does. A judged or framework measurement is now
    // admitted by the authority, so what decides is the client alone.
    let f = Dispatched::new("dispatcher-runtimes").await;
    assert_eq!(f.dispatcher.runtimes(), vec![RuntimeKind::ScoreEvaluation]);

    let judged = ProjectDispatcher::bind(
        &(Arc::new(f.shared.clone()) as Arc<dyn WorkflowStore>),
        &f.objects,
        &Arc::new(f.root.clone()),
        &(f.iam.clone() as Arc<dyn IamStore>),
        f.scope,
        "project-dispatcher-judged".to_owned(),
    )
    .unwrap()
    .judged_by(Arc::new(Unasked), 1);
    assert_eq!(
        judged.runtimes(),
        vec![RuntimeKind::ScoreEvaluation, RuntimeKind::JudgeEvaluation]
    );
    // And it runs one: the executor is built per attempt from the step's own
    // binding, so the judged half is wired without a second registry.
    assert!(matches!(
        judged.poll_once(now()).await.unwrap(),
        Performed::Reported {
            succeeded: true,
            ..
        }
    ));
}

/// A judge on this host that is never asked: the declaration under test names
/// no rubric, so what it proves is that the client was wired at all.
#[derive(Debug)]
struct Unasked;

#[async_trait]
impl aiwatcher_evaluation::JudgeModel for Unasked {
    fn provider(&self) -> &str {
        "llamacpp"
    }

    async fn ask(
        &self,
        _call: &aiwatcher_evaluation::JudgeCall,
    ) -> std::result::Result<aiwatcher_evaluation::JudgeReply, aiwatcher_evaluation::JudgeFailure>
    {
        panic!("a declaration that names no rubric put a question to a judge")
    }
}

/// An executor that refuses everything, so a claimed attempt goes no further.
#[derive(Debug)]
struct Refuses;

#[async_trait]
impl aiwatcher_execution::ActivityExecutor for Refuses {
    fn runtime(&self) -> RuntimeKind {
        RuntimeKind::ScoreEvaluation
    }

    async fn execute(
        &self,
        _command: &aiwatcher_execution::ActivityCommand,
        _context: &aiwatcher_execution::ActivityContext,
    ) -> std::result::Result<aiwatcher_execution::ActivityResult, aiwatcher_execution::ActivityError>
    {
        panic!("the unscoped reactor performed a project's attempt")
    }
}
