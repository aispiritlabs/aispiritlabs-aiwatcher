//! Executor boundary only: no project HTTP start or global reactor registration.
use super::{fixture::Fixture, project_declarations::seed, project_evidence::source, *};
use aiwatcher_execution::{
    ActivityCommand, ActivityContext, ActivityExecutor, ExecutionId, FailureClass,
};
use aiwatcher_iam::{
    Change, Command, GrantId, GrantWindow, Grantee, IamStore, Principal, ProjectRole, ProjectScope,
    memory::MemoryIamStore,
};
use aiwatcher_server::execution::scoring::{ScoreExecutor, project::ProjectAuthority};
use std::sync::atomic::{AtomicBool, AtomicI64};
use tokio::sync::Notify;

#[derive(Debug)]
struct Clock(AtomicI64);
impl aiwatcher_iam::Clock for Clock {
    fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Debug)]
struct PausedRead {
    inner: Arc<dyn ObjectStore>,
    armed: AtomicBool,
    writing: AtomicBool,
    entered: Notify,
    resume: Notify,
}
#[async_trait]
impl ObjectStore for PausedRead {
    async fn get(&self, key: &str) -> PortResult<Option<Vec<u8>>> {
        if self.armed.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        self.inner.get(key).await
    }
    async fn put(&self, key: &str, bytes: Vec<u8>) -> PortResult<()> {
        self.inner.put(key, bytes).await
    }
    async fn create(&self, key: &str, bytes: Vec<u8>) -> PortResult<bool> {
        if self.writing.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        self.inner.create(key, bytes).await
    }
    async fn list(&self, prefix: &str) -> PortResult<Vec<ObjectEntry>> {
        self.inner.list(prefix).await
    }
    async fn delete(&self, key: &str) -> PortResult<()> {
        self.inner.delete(key).await
    }
}

struct ProjectRun {
    _fixture: Fixture,
    iam: Arc<MemoryIamStore>,
    clock: Arc<Clock>,
    actor: Principal,
    scope: ProjectScope,
    grant: GrantId,
    store: Arc<PausedRead>,
    root: Registry,
    local: Registry,
    command: ActivityCommand,
    context: ActivityContext,
    declaration: String,
    result: String,
}
impl ProjectRun {
    async fn new(name: &str) -> Self {
        let fixture = Fixture::new(name).await;
        let clock = Arc::new(Clock(AtomicI64::new(1000)));
        let iam = Arc::new(MemoryIamStore::with_clock(clock.clone()));
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
        let creator_grants = iam.access(scope, &actor).await.unwrap().grants;
        for grant in creator_grants {
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
                    window: GrantWindow {
                        valid_from: 1000,
                        edit_until: Some(2000),
                        read_until: Some(3000),
                    },
                },
            )
            .await
            .unwrap()
        else {
            panic!("grant")
        };
        let store = Arc::new(PausedRead {
            inner: fixture.store.clone(),
            armed: AtomicBool::new(false),
            writing: AtomicBool::new(false),
            entered: Notify::new(),
            resume: Notify::new(),
        });
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
        let (command, context) = attempt(&declaration.id, &run.evaluation_id);
        Self {
            _fixture: fixture,
            iam,
            clock,
            actor,
            scope,
            grant: grant.id,
            store,
            root,
            local,
            command,
            context,
            declaration: declaration.id,
            result: run.evaluation_id,
        }
    }
    fn executor(&self, actor: Principal) -> ScoreExecutor {
        ScoreExecutor::for_project(
            &self.root,
            ProjectAuthority::new(
                self.iam.clone(),
                self.scope,
                actor,
                self.command.key.execution_id.clone(),
                self.declaration.clone(),
            ),
        )
        .unwrap()
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
    async fn no_result(&self) {
        assert!(
            self.local
                .get(&self.result, "reader", 1000)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            self.root
                .get(&self.result, "reader", 1000)
                .await
                .unwrap()
                .is_none()
        );
    }
}

#[tokio::test]
async fn a_project_executor_binds_the_principal_execution_and_declaration_not_the_worker_name() {
    let f = ProjectRun::new("project-executor-binding").await;
    for actor in [
        Principal::new("other-issuer", "owner").unwrap(),
        Principal::new("issuer", "declaration-author").unwrap(),
        Principal::new("issuer", "project-admin").unwrap(),
    ] {
        let error = f
            .executor(actor)
            .execute(&f.command, &f.context)
            .await
            .unwrap_err();
        assert_eq!(error.class, FailureClass::Policy);
    }
    let executor = f.executor(f.actor.clone());
    let mut wrong = f.command.clone();
    wrong.key.execution_id = ExecutionId::new("another-execution");
    assert_eq!(
        executor
            .execute(&wrong, &f.context)
            .await
            .unwrap_err()
            .class,
        FailureClass::Policy
    );
    wrong = f.command.clone();
    wrong.step.id = "other-step".into();
    assert_eq!(
        executor
            .execute(&wrong, &f.context)
            .await
            .unwrap_err()
            .class,
        FailureClass::Policy
    );
    // A card that asks a judge or a scorer service compiles to another binding
    // over the same declaration, so all three are admitted — and in each of
    // them the declaration is still the pinned one, or nothing is.
    let spec = |declaration: &str| aiwatcher_execution::plan::ScoreEvaluationSpec {
        declaration: declaration.to_owned(),
    };
    let binding = |declaration: &str| {
        [
            aiwatcher_execution::RuntimeBinding::ScoreEvaluation(spec(declaration)),
            aiwatcher_execution::RuntimeBinding::JudgeEvaluation(spec(declaration)),
            aiwatcher_execution::RuntimeBinding::ExternalEvaluation(spec(declaration)),
        ]
    };
    for runtime in binding(&"0".repeat(64)) {
        wrong = f.command.clone();
        wrong.step.runtime = runtime;
        assert_eq!(
            executor
                .execute(&wrong, &f.context)
                .await
                .unwrap_err()
                .class,
            FailureClass::Policy
        );
    }
    let other = ProjectScope {
        project: aiwatcher_iam::ProjectId::new(),
        ..f.scope
    };
    assert!(
        ScoreExecutor::for_project(
            &f.local,
            ProjectAuthority::new(
                f.iam.clone(),
                other,
                f.actor.clone(),
                f.command.key.execution_id.clone(),
                f.declaration.clone()
            )
        )
        .is_err()
    );
    f.no_result().await;
    // Successful publication lives only under this project's registry. Changing
    // the worker's display name does not alter the trusted principal.
    let mut context = f.context.clone();
    context.owner = "unrelated-worker-name".into();
    let result = executor.execute(&f.command, &context).await.unwrap();
    assert!(!result.cacheable);
    assert!(
        f.root
            .get(&f.result, "reader", 1000)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        f.local
            .get(
                &f.result,
                "reader",
                time::OffsetDateTime::now_utc().unix_timestamp()
            )
            .await
            .unwrap()
            .is_some()
    );
    // A retry is not permission to bypass a grant that has since been revoked.
    f.revoke().await;
    assert_eq!(
        executor
            .execute(&f.command, &context)
            .await
            .unwrap_err()
            .class,
        FailureClass::Policy
    );
}

#[tokio::test]
async fn revocation_after_publication_was_admitted_does_not_interrupt_its_commit() {
    let f = ProjectRun::new("project-executor-committing").await;
    let executor = f.executor(f.actor.clone());
    let command = f.command.clone();
    let context = f.context.clone();
    f.store.writing.store(true, Ordering::SeqCst);
    let task = tokio::spawn(async move { executor.execute(&command, &context).await });
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        f.store.entered.notified(),
    )
    .await
    .unwrap();
    f.revoke().await;
    f.store.resume.notify_one();
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!result.cacheable);
    assert!(
        f.local
            .get(
                &f.result,
                "reader",
                time::OffsetDateTime::now_utc().unix_timestamp()
            )
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        f.executor(f.actor.clone())
            .execute(&f.command, &f.context)
            .await
            .unwrap_err()
            .class,
        FailureClass::Policy
    );
}

#[tokio::test]
async fn an_execution_grant_cannot_replace_a_withdrawn_pair_approval() {
    let f = ProjectRun::new("project-executor-withdrawn").await;
    let view = f
        .local
        .scoring_run_view(&f.declaration)
        .await
        .unwrap()
        .unwrap();
    f.local
        .withdraw(&view.approval_id, "project-admin", 12)
        .await
        .unwrap()
        .unwrap();
    assert!(
        f.executor(f.actor.clone())
            .execute(&f.command, &f.context)
            .await
            .is_err()
    );
    f.no_result().await;
}

#[tokio::test]
async fn revocation_or_expiry_while_a_project_measurement_reads_prevents_publication() {
    for expire in [false, true] {
        let f = ProjectRun::new(if expire {
            "project-executor-expiry"
        } else {
            "project-executor-revocation"
        })
        .await;
        let executor = f.executor(f.actor.clone());
        let command = f.command.clone();
        let context = f.context.clone();
        f.store.armed.store(true, Ordering::SeqCst);
        let task = tokio::spawn(async move { executor.execute(&command, &context).await });
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            f.store.entered.notified(),
        )
        .await
        .unwrap();
        if expire {
            f.clock.0.store(2000, Ordering::SeqCst);
        } else {
            f.revoke().await;
        }
        f.store.resume.notify_one();
        let error = tokio::time::timeout(std::time::Duration::from_secs(10), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(error.class, FailureClass::Policy);
        f.no_result().await;
        // Organization ownership alone, or read-only access after edit_until,
        // grants no right to execute another attempt.
        assert_eq!(
            f.executor(f.actor.clone())
                .execute(&f.command, &f.context)
                .await
                .unwrap_err()
                .class,
            FailureClass::Policy
        );
    }
}

#[tokio::test]
async fn a_judged_or_framework_binding_over_the_pinned_declaration_is_admitted() {
    // A card that asks a judge compiles to `judge_evaluation` and one that asks
    // a scorer service to `external_evaluation`. They are the same measurement
    // in the role that holds the client, so the authority admits all three —
    // and whether *this* process holds that client is the dispatcher's
    // question rather than the authority's.
    let f = ProjectRun::new("project-executor-bindings").await;
    for runtime in [
        aiwatcher_execution::RuntimeBinding::JudgeEvaluation(
            aiwatcher_execution::plan::ScoreEvaluationSpec {
                declaration: f.declaration.clone(),
            },
        ),
        aiwatcher_execution::RuntimeBinding::ExternalEvaluation(
            aiwatcher_execution::plan::ScoreEvaluationSpec {
                declaration: f.declaration.clone(),
            },
        ),
    ] {
        let mut pinned = f.command.clone();
        pinned.step.runtime = runtime;
        f.executor(f.actor.clone())
            .execute(&pinned, &f.context)
            .await
            .expect("the pinned declaration in a measuring binding");
    }
    // And a revoked grant still refuses every one of them.
    f.revoke().await;
    let mut pinned = f.command.clone();
    pinned.step.runtime = aiwatcher_execution::RuntimeBinding::JudgeEvaluation(
        aiwatcher_execution::plan::ScoreEvaluationSpec {
            declaration: f.declaration.clone(),
        },
    );
    assert_eq!(
        f.executor(f.actor.clone())
            .execute(&pinned, &f.context)
            .await
            .unwrap_err()
            .class,
        FailureClass::Policy
    );
}

/// A judge on this host that is never asked: the declaration under test names
/// no rubric, so what it proves is which capability the guard refuses.
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

#[tokio::test]
async fn a_project_recording_may_hold_a_deployment_s_judge_and_never_its_artifact_store() {
    let f = ProjectRun::new("project-executor-capabilities").await;
    // A judge is a socket and a credential this role holds, asked a question
    // composed from the project's own card.
    f.executor(f.actor.clone())
        .judged_by(Arc::new(Unasked), 1)
        .execute(&f.command, &f.context)
        .await
        .expect("a project recording beside the deployment's judge");

    // The artifact reader is not: it resolves an `object://` in the global
    // namespace, so a project run reading through it would read bytes that are
    // not its project's.
    let global = aiwatcher_server::execution::artifacts::Artifacts::new(f.store.clone());
    assert_eq!(
        f.executor(f.actor.clone())
            .reading_from(global)
            .execute(&f.command, &f.context)
            .await
            .unwrap_err()
            .class,
        FailureClass::Policy
    );
}
