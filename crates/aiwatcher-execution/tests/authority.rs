#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! What a reactor asks before it reads a project's data, and again before it
//! writes any. ADR_0033.
//!
//! `tests/scope.rs` proves that a project's run is invisible to the unscoped
//! paths. This proves the other half: that a reactor *bound* to that project
//! still asks, in the right order, and never reads a refusal as a yes.
//!
//! The authority here is a stub, because the question is the reactor's ordering
//! rather than IAM's answer. The IAM half is
//! `aiwatcher-server/tests/evaluation/project_dispatcher.rs`, against a real
//! policy and a real store.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use time::OffsetDateTime;

use aiwatcher_core::{ArtifactKind, ArtifactRef};
use aiwatcher_datasets::QueryEngine;
use aiwatcher_execution::activity::{
    ActivityCommand, ActivityContext, ActivityError, ActivityExecutor, ActivityResult,
    ExecutorRegistry,
};
use aiwatcher_execution::message::PayloadDefault;
use aiwatcher_execution::plan::{
    CachePolicy, DefinitionKind, DefinitionRevision, ExecutionPlan, FlowSourceRef, FlowStepSpec,
    PlanStep, ResolvedWindow, RetryPolicy, RuntimeBinding, RuntimeKind,
};
use aiwatcher_execution::store::memory::MemoryWorkflowStore;
use aiwatcher_execution::{
    Admitting, ArtifactCatalog, CacheEntry, CatalogedArtifact, Decider, ExecutionAuthority,
    ExecutionHandler, ExecutionId, ExecutionOwnership, Executions, FailureClass,
    MemoryArtifactCatalog, Performed, ProjectStart, Reactor, RunIdentity, StartRun, StateType,
    WorkflowStore, replay,
};
use aiwatcher_iam::{OrganizationId, Principal, ProjectId, ProjectScope};
use async_trait::async_trait;

fn at(seconds: i64) -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(seconds)
}

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
                source: FlowSourceRef {
                    dataset: "runs".to_owned(),
                    // Pinned, so this step *has* a key and the lookup is a
                    // question about ordering rather than about caching.
                    window: Some(ResolvedWindow { from: 0, to: 3600 }),
                    ..FlowSourceRef::default()
                },
                blocks: Vec::new(),
            }),
            inputs: Vec::new(),
            outputs: Vec::new(),
            retry: RetryPolicy::default(),
            timeout_seconds: 60,
            // The whole point of the third rule: this step may answer from the
            // index, so a lookup happens whether or not any work follows.
            cache: CachePolicy::ByContent,
        }],
        Vec::new(),
    )
}

/// What the stub authority answers, and what it was asked.
#[derive(Debug, Default)]
struct Stub {
    /// One refusal per moment, or `None` to admit it.
    refuse: BTreeMap<&'static str, FailureClass>,
    asked: std::sync::Mutex<Vec<(&'static str, Option<ExecutionOwnership>)>>,
}

impl Stub {
    fn admitting() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn refusing(moment: Admitting, class: FailureClass) -> Arc<Self> {
        let mut refuse = BTreeMap::new();
        refuse.insert(moment.as_str(), class);
        Arc::new(Self {
            refuse,
            asked: std::sync::Mutex::new(Vec::new()),
        })
    }

    fn moments(&self) -> Vec<&'static str> {
        self.asked
            .lock()
            .expect("the asks")
            .iter()
            .map(|(moment, _)| *moment)
            .collect()
    }

    fn owners(&self) -> Vec<Option<ExecutionOwnership>> {
        self.asked
            .lock()
            .expect("the asks")
            .iter()
            .map(|(_, owner)| owner.clone())
            .collect()
    }
}

#[async_trait]
impl ExecutionAuthority for Stub {
    async fn admits(
        &self,
        ownership: Option<&ExecutionOwnership>,
        admitting: Admitting,
    ) -> Result<(), ActivityError> {
        self.asked
            .lock()
            .expect("the asks")
            .push((admitting.as_str(), ownership.cloned()));
        match self.refuse.get(admitting.as_str()) {
            Some(class) => Err(ActivityError::new(*class, "the stub said no")),
            None => Ok(()),
        }
    }
}

/// A catalog that says how often it was asked, over the one this crate ships.
///
/// It answers for real — a stub that returned nothing would hide a hit — and
/// what the tests read is the two counts: whether a lookup was *reached*, and
/// whether a result was described in the index.
#[derive(Debug, Default)]
struct Counting {
    inner: MemoryArtifactCatalog,
    looked_up: AtomicUsize,
    recorded: AtomicUsize,
}

#[async_trait]
impl ArtifactCatalog for Counting {
    async fn record(
        &self,
        artifact: CatalogedArtifact,
    ) -> Result<CatalogedArtifact, aiwatcher_execution::StoreError> {
        self.recorded.fetch_add(1, Ordering::SeqCst);
        self.inner.record(artifact).await
    }

    async fn by_digest(
        &self,
        digest: &str,
        kind: ArtifactKind,
    ) -> Result<Option<CatalogedArtifact>, aiwatcher_execution::StoreError> {
        self.inner.by_digest(digest, kind).await
    }

    async fn produced_by(
        &self,
        execution: &ExecutionId,
    ) -> Result<Vec<CatalogedArtifact>, aiwatcher_execution::StoreError> {
        self.inner.produced_by(execution).await
    }

    async fn cached(
        &self,
        cache_key: &str,
        now: OffsetDateTime,
    ) -> Result<Option<CacheEntry>, aiwatcher_execution::StoreError> {
        self.looked_up.fetch_add(1, Ordering::SeqCst);
        self.inner.cached(cache_key, now).await
    }

    async fn remember(&self, entry: CacheEntry) -> Result<(), aiwatcher_execution::StoreError> {
        self.inner.remember(entry).await
    }

    async fn invalidate(
        &self,
        cache_key: &str,
        at: OffsetDateTime,
    ) -> Result<(), aiwatcher_execution::StoreError> {
        self.inner.invalidate(cache_key, at).await
    }
}

/// A runtime that answers with one artifact and counts how often it ran.
#[derive(Debug, Default)]
struct Fake {
    ran: AtomicUsize,
}

#[async_trait]
impl ActivityExecutor for Fake {
    fn runtime(&self) -> RuntimeKind {
        RuntimeKind::FlowPhp
    }

    async fn execute(
        &self,
        _command: &ActivityCommand,
        _context: &ActivityContext,
    ) -> Result<ActivityResult, ActivityError> {
        self.ran.fetch_add(1, Ordering::SeqCst);
        Ok(ActivityResult {
            outputs: vec![
                ArtifactRef::new(
                    "rows",
                    format!("object://rows/{}", "cd".repeat(32)),
                    "cd".repeat(32),
                )
                .of_kind(ArtifactKind::Rows),
            ],
            cacheable: true,
            ..ActivityResult::default()
        })
    }
}

/// Everything one project's pass needs, with the run already started.
struct Bound {
    execution: ExecutionId,
    store: Arc<dyn WorkflowStore>,
    ownership: ExecutionOwnership,
    authority: Arc<Stub>,
    catalog: Arc<Counting>,
    executor: Arc<Fake>,
    reactor: Reactor<Arc<dyn WorkflowStore>>,
}

impl Bound {
    async fn start(authority: Arc<Stub>) -> Self {
        let scope = scope();
        let shared = MemoryWorkflowStore::new();
        let store = shared.for_project(scope).expect("a project store");
        let start = ProjectStart::new(scope, principal("alice"));
        let handler = ExecutionHandler::new(Arc::clone(&store));
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
            plan(),
            StartRun {
                identity: RunIdentity::Key("nightly".to_owned()),
                parameters: BTreeMap::new(),
                requested_by: "somebody".to_owned(),
                decided_by: Decider::Local,
                payloads: None,
                project: Some(start.clone()),
            },
        )
        .await
        .expect("a project start");

        let catalog = Arc::new(Counting::default());
        let executor = Arc::new(Fake::default());
        let reactor = Reactor::new(
            ExecutionHandler::new(Arc::clone(&store)),
            ExecutorRegistry::new().with(Arc::clone(&executor) as Arc<dyn ActivityExecutor>),
            "project-reactor".to_owned(),
        )
        .with_catalog(Arc::clone(&catalog) as Arc<dyn ArtifactCatalog>)
        .with_authority(Arc::clone(&authority) as Arc<dyn ExecutionAuthority>);

        Self {
            execution: started.execution_id,
            store,
            ownership: ExecutionOwnership::of(&start, &plan()),
            authority,
            catalog,
            executor,
            reactor,
        }
    }

    /// What the step's state is now, and the error it carries if it has one.
    async fn step(&self) -> (StateType, Option<FailureClass>) {
        let slice = self.store.load(&self.execution).await.expect("a load");
        let run = replay(slice.events()).active().cloned().expect("a run");
        let step = run.step("read").expect("the step").clone();
        let class = step
            .attempts
            .iter()
            .find_map(|record| record.error.as_ref())
            .map(|error| error.class);
        (step.state.state_type, class)
    }
}

#[tokio::test]
async fn the_grant_is_asked_before_the_index_is_looked_up_and_before_the_step_starts() {
    let bound = Bound::start(Stub::refusing(Admitting::Work, FailureClass::Policy)).await;

    let performed = bound.reactor.poll_once(at(10)).await.expect("a pass");
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
        bound.catalog.looked_up.load(Ordering::SeqCst),
        0,
        "a hit is an answer about a project's data whether or not any work follows"
    );
    assert_eq!(bound.executor.ran.load(Ordering::SeqCst), 0);
    assert_eq!(bound.authority.moments(), vec!["work"]);

    // And nothing claims an attempt began: the refusal came before
    // `step.started`, so the waterfall has no bar for work that never ran.
    let slice = bound.store.load(&bound.execution).await.expect("a load");
    assert!(
        !slice.events().any(|event| matches!(
            event,
            aiwatcher_execution::WorkflowEvent::StepStarted { .. }
        )),
        "a refused attempt reported a start"
    );
}

#[tokio::test]
async fn the_index_is_reached_once_the_grant_admits_the_work() {
    // The same pass with the same catalog, so the count above is a refusal
    // rather than a lookup this plan never makes.
    let bound = Bound::start(Stub::admitting()).await;
    bound.reactor.poll_once(at(10)).await.expect("a pass");
    assert_eq!(bound.catalog.looked_up.load(Ordering::SeqCst), 1);
    assert_eq!(bound.executor.ran.load(Ordering::SeqCst), 1);
    assert_eq!(bound.authority.moments(), vec!["work", "publication"]);
}

#[tokio::test]
async fn an_authority_that_could_not_answer_is_not_a_yes_and_costs_the_run_nothing() {
    let bound = Bound::start(Stub::refusing(Admitting::Work, FailureClass::Transient)).await;

    let performed = bound.reactor.poll_once(at(10)).await.expect("a pass");
    assert!(
        matches!(performed, Performed::Released { .. }),
        "an unavailable authority ran the work or ended the run: {performed:?}"
    );
    assert_eq!(bound.executor.ran.load(Ordering::SeqCst), 0);
    assert_eq!(bound.catalog.looked_up.load(Ordering::SeqCst), 0);

    // Nothing was written: no start, no failure, no retry budget spent. The
    // attempt is claimable again once the lease lapses.
    let (state, class) = bound.step().await;
    assert_eq!(state, StateType::Pending, "the step was moved on");
    assert_eq!(class, None);
}

#[tokio::test]
async fn a_grant_that_is_gone_ends_the_run_rather_than_leaving_it_claimed_every_pass() {
    let bound = Bound::start(Stub::refusing(Admitting::Work, FailureClass::Policy)).await;
    bound.reactor.poll_once(at(10)).await.expect("a pass");

    let (state, class) = bound.step().await;
    assert_eq!(state, StateType::Failed);
    assert_eq!(class, Some(FailureClass::Policy));

    // And the refusal is final: `Policy` is not retryable, so the run has
    // settled rather than come back for another claim.
    let slice = bound.store.load(&bound.execution).await.expect("a load");
    let run = replay(slice.events()).active().cloned().expect("a run");
    assert!(run.state.state_type.is_terminal(), "{:?}", run.state);
    assert!(
        matches!(
            bound.reactor.poll_once(at(20)).await.expect("a pass"),
            Performed::Idle
        ),
        "a settled run was claimed again"
    );
}

#[tokio::test]
async fn a_grant_revoked_while_the_work_ran_is_asked_again_before_anything_is_recorded() {
    let bound = Bound::start(Stub::refusing(Admitting::Publication, FailureClass::Policy)).await;

    let performed = bound.reactor.poll_once(at(10)).await.expect("a pass");
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
    assert_eq!(bound.executor.ran.load(Ordering::SeqCst), 1, "the work ran");
    assert_eq!(
        bound.catalog.recorded.load(Ordering::SeqCst),
        0,
        "a project's outputs were described in the index after its grant was gone"
    );
    assert_eq!(bound.authority.moments(), vec!["work", "publication"]);
    assert_eq!(
        bound.step().await,
        (StateType::Failed, Some(FailureClass::Policy))
    );
}

#[tokio::test]
async fn an_authority_that_could_not_answer_before_publication_still_says_what_happened() {
    // The other half of the rule above: before the work, being unable to ask
    // leaves the attempt alone; after it, the lease is this pass's and saying
    // nothing would lose the outcome silently.
    let bound = Bound::start(Stub::refusing(
        Admitting::Publication,
        FailureClass::Transient,
    ))
    .await;

    let performed = bound.reactor.poll_once(at(10)).await.expect("a pass");
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
    assert_eq!(bound.catalog.recorded.load(Ordering::SeqCst), 0);
    let (state, class) = bound.step().await;
    assert_eq!(class, Some(FailureClass::Transient));
    assert_ne!(state, StateType::Failed, "a retryable class ended the run");
}

#[tokio::test]
async fn the_record_the_authority_is_asked_about_is_the_one_the_start_wrote() {
    let bound = Bound::start(Stub::admitting()).await;
    bound.reactor.poll_once(at(10)).await.expect("a pass");

    // Every ask carries the durable record and nothing else: an authority has
    // no plan, no parameter and no claimant's name to read a scope or a
    // principal out of.
    let owners = bound.authority.owners();
    assert_eq!(owners.len(), 2);
    for owner in owners {
        assert_eq!(owner.as_ref(), Some(&bound.ownership));
    }
    assert_eq!(
        bound
            .store
            .ownership(&bound.execution)
            .await
            .expect("a read"),
        Some(bound.ownership.clone()),
    );
}

#[tokio::test]
async fn a_reactor_with_an_authority_does_not_rebuild_a_claim_across_requests() {
    // The worker seam's third method is a claim rebuilt in another request,
    // with no pass to ask an authority in. A reactor that has one holds a
    // project's store, and answers as it would about an attempt it does not
    // hold.
    let bound = Bound::start(Stub::admitting()).await;
    let key = aiwatcher_execution::AttemptKey {
        execution_id: bound.execution.clone(),
        step_id: "read".to_owned(),
        attempt: 1,
    };
    assert!(
        bound
            .reactor
            .resume(&key, at(10))
            .await
            .expect("a resume")
            .is_none()
    );
    assert!(bound.authority.moments().is_empty());
}
