//! The one gate a project's work passes through. ADR_0033.
//!
//! A store that binds to one project, an execution that carries its owner, a
//! byte store and a catalog that pair, and an executor that names one run were
//! all already built. What was missing is the loop that puts them in the right
//! order — and the order *is* the design, because every way of getting it wrong
//! is silent.
//!
//! ```text
//!   bind the store ──► claim ──► ownership ──► grant? ──► cache ──► run
//!        │                                                            │
//!        └── one project, one catalog, one byte store        grant? ◄──┘
//!            (ProjectArtifacts::bind)                           │
//!                                                               └──► record
//! ```
//!
//! **The scope and the principal come from the record and from nowhere else.**
//! [`ExecutionOwnership`] is read from the bound store and handed to the
//! authority and to the executor: never the plan, a parameter, `requested_by`
//! or the name a claimant leases under.
//!
//! **The grant is asked twice, and a failure is never consent.** Once before
//! anything of the project is read, the cache lookup included, and once before
//! anything is written ([`aiwatcher_execution::authority`]).
//!
//! **Both halves of artifact storage are bound together**, or a project's
//! outputs are described in the index a cache lookup for anybody reads.
//!
//! It is **not** a start path, and registers no executor in a process-wide
//! [`ExecutorRegistry`](aiwatcher_execution::ExecutorRegistry): a project
//! executor names one run, so it is built per attempt. No production wiring
//! constructs one; `docs/iam-01-kickoff.md` is what a route still waits on.

use std::sync::Arc;

use aiwatcher_core::prompts::ObjectStore;
use aiwatcher_evaluation::{ExternalScorers, JudgeModel, Registry as Evaluations};
use aiwatcher_execution::reactor::{Claimed, Taken};
use aiwatcher_execution::{
    ActivityError, ActivityExecutor, Admitting, ClaimFilter, ExecutionAuthority, ExecutionHandler,
    ExecutionOwnership, ExecutionScope, FailureClass, Performed, Reactor, RuntimeBinding,
    RuntimeKind, WorkflowStore,
};
use aiwatcher_iam::{IamStore, Principal, ProjectRole, ProjectScope};
use async_trait::async_trait;
use time::OffsetDateTime;

use super::artifacts::ProjectArtifacts;
use super::scoring::ScoreExecutor;
use super::scoring::project::ProjectAuthority;

/// Whether this principal may still have a project's work done, right now.
///
/// One question in one place, asked by the reactor's authority before and after
/// the work and by [`ProjectAuthority`] inside it. A second copy would be a
/// second idea of which role runs a measurement, and the one that drifted would
/// be the one a revoked grant did not reach.
///
/// [`ProjectRole::Editor`] because running work is a write. An unreachable
/// backend is [`FailureClass::Transient`] and everything else is
/// [`FailureClass::Policy`]: not being *able* to ask is not being told yes.
///
/// # Errors
///
/// [`ActivityError`] carrying the class a refusal is acted on under.
pub(crate) async fn editor_grant(
    iam: &Arc<dyn IamStore>,
    scope: ProjectScope,
    principal: &Principal,
) -> Result<(), ActivityError> {
    let access = iam
        .access(scope, principal)
        .await
        .map_err(|error| match error {
            aiwatcher_iam::Error::Backend(_) => {
                ActivityError::transient("project execution authorization is unavailable")
            }
            _ => denied(),
        })?;
    if access.project.scope != scope || access.role < ProjectRole::Editor {
        return Err(denied());
    }
    Ok(())
}

fn denied() -> ActivityError {
    ActivityError::new(
        FailureClass::Policy,
        "project execution is not authorized for this principal and measurement",
    )
}

/// The reactor's half of the question: may this execution's owner be served.
///
/// Bound to one project, which is the project its reactor's store is bound to.
/// It reads no execution, no plan and no step — the record it is handed is the
/// whole input, and that is what makes "the scope comes from the store" a
/// property of the type rather than of a convention.
#[derive(Debug)]
pub struct ProjectGrant {
    iam: Arc<dyn IamStore>,
    scope: ProjectScope,
}

impl ProjectGrant {
    #[must_use]
    pub const fn new(iam: Arc<dyn IamStore>, scope: ProjectScope) -> Self {
        Self { iam, scope }
    }
}

#[async_trait]
impl ExecutionAuthority for ProjectGrant {
    async fn admits(
        &self,
        ownership: Option<&ExecutionOwnership>,
        _admitting: Admitting,
    ) -> Result<(), ActivityError> {
        // No record is a global run, and a project's reactor adopting one is
        // the repointing the record exists to prevent. The bound store refuses
        // it too; this is the same answer from the side that has a principal.
        let Some(owner) = ownership else {
            return Err(denied());
        };
        if owner.scope != self.scope {
            return Err(denied());
        }
        editor_grant(&self.iam, self.scope, &owner.principal).await
    }
}

/// One project's reactors: the bound store, the paired artifact halves, and the
/// executors built per attempt from the attempt's own owner.
///
/// A dispatcher per scope rather than one loop over many, for the reason
/// ADR_0033 names: binding is what makes the unscoped path safe, and a bind
/// that had to happen per attempt would be a cost per claim. Here it happens
/// once and the loop is a reactor like any other.
#[derive(Debug)]
pub struct ProjectDispatcher {
    scope: ProjectScope,
    reactor: Reactor<Arc<dyn WorkflowStore>>,
    artifacts: ProjectArtifacts,
    evaluations: Arc<Evaluations>,
    iam: Arc<dyn IamStore>,
    judge: Option<(Arc<dyn JudgeModel>, usize)>,
    scorers: Option<(Arc<dyn ExternalScorers>, usize)>,
    /// What the two steps around a generated answer need, where this process
    /// holds them: the fold to read traces off, and what a witnessed answer is
    /// held against. Absent is a working state and says so by claiming
    /// neither, which is every other executor's rule here.
    telemetry: Option<Telemetry>,
}

/// What a project's `cases` and `traces` steps are performed with.
///
/// Held as one value because the two steps are one half of one plan — a run
/// whose answers a worker generates hands it the cases and then reads the
/// traces of what came back — and a process that holds one without the other
/// would claim half a run.
#[derive(Clone)]
pub struct Telemetry {
    pub read_model: Arc<aiwatcher_projector::ReadModel>,
    pub bundles: Option<Arc<dyn aiwatcher_evaluation::ApprovalBundles>>,
    pub witnesses: aiwatcher_evaluation::Witnesses,
    pub prompts: Option<Arc<aiwatcher_prompts::Registry>>,
    pub asked: Option<Arc<aiwatcher_projector::AskedIndex>>,
    pub wait: std::time::Duration,
}

impl std::fmt::Debug for Telemetry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Telemetry")
            .field("bundles", &self.bundles.is_some())
            .field("prompts", &self.prompts.is_some())
            .field("asked", &self.asked.is_some())
            .finish_non_exhaustive()
    }
}

impl ProjectDispatcher {
    /// Bind everything one project needs, or say which half refused.
    ///
    /// `store` is the deployment's unscoped workflow store and `objects` its
    /// object store; both are narrowed here and neither is narrowed anywhere
    /// else on this path. `owner` is the lease name, and has to be unique per
    /// process as every reactor's is.
    ///
    /// # Errors
    ///
    /// [`ActivityError`] of class `Policy` when the store, the byte store or
    /// the catalog refuses the scope.
    pub fn bind(
        store: &Arc<dyn WorkflowStore>,
        objects: &Arc<dyn ObjectStore>,
        evaluations: &Arc<Evaluations>,
        iam: &Arc<dyn IamStore>,
        scope: ProjectScope,
        owner: String,
    ) -> Result<Self, ActivityError> {
        let bound = store
            .for_project(scope)
            .map_err(|error| ActivityError::new(FailureClass::Policy, error.to_string()))?;
        let artifacts = ProjectArtifacts::bind(objects, scope)?;
        // The store, the catalog and the authority are one project's or the
        // dispatcher does not exist: a reactor holding two of the three is the
        // failure this constructor is here to make unwritable.
        let reactor = Reactor::new(ExecutionHandler::new(bound), Default::default(), owner)
            .with_catalog(Arc::clone(artifacts.catalog()))
            .with_authority(Arc::new(ProjectGrant::new(Arc::clone(iam), scope)));
        Ok(Self {
            scope,
            reactor,
            artifacts,
            evaluations: Arc::clone(evaluations),
            iam: Arc::clone(iam),
            judge: None,
            scorers: None,
            telemetry: None,
        })
    }

    /// Also claim the two steps around a worker's generated answers.
    #[must_use]
    pub fn reading_telemetry(mut self, telemetry: Telemetry) -> Self {
        self.telemetry = Some(telemetry);
        self
    }

    /// Also claim the measurements whose card asks a judge.
    #[must_use]
    pub fn judged_by(mut self, judge: Arc<dyn JudgeModel>, concurrency: usize) -> Self {
        self.judge = Some((judge, concurrency));
        self
    }

    /// Also claim the measurements whose card asks a scorer service.
    #[must_use]
    pub fn scored_by(mut self, scorers: Arc<dyn ExternalScorers>, concurrency: usize) -> Self {
        self.scorers = Some((scorers, concurrency));
        self
    }

    #[must_use]
    pub const fn scope(&self) -> ProjectScope {
        self.scope
    }

    /// The byte store and the catalog this project's outputs go through.
    ///
    /// Held as the pair rather than as the catalog alone, although only the
    /// catalog has a reader today: a project measurement publishes through the
    /// evaluation registry and produces no artifact, and a project executor
    /// that reads a *global* byte store is refused by name. When one that
    /// writes bytes arrives, the store it is handed is this one.
    #[must_use]
    pub const fn artifacts(&self) -> &ProjectArtifacts {
        &self.artifacts
    }

    /// The runtimes a project's attempt may be claimed for here.
    ///
    /// Judged and framework measurements only where this process holds the
    /// client, which is every other reactor's rule: an attempt whose runtime
    /// nothing here performs waits for a process that does rather than failing
    /// in one that cannot.
    #[must_use]
    pub fn runtimes(&self) -> Vec<RuntimeKind> {
        let mut runtimes = vec![RuntimeKind::ScoreEvaluation];
        if self.judge.is_some() {
            runtimes.push(RuntimeKind::JudgeEvaluation);
        }
        if self.scorers.is_some() {
            runtimes.push(RuntimeKind::ExternalEvaluation);
        }
        if self.telemetry.is_some() {
            runtimes.push(RuntimeKind::EvaluationCases);
            runtimes.push(RuntimeKind::EvaluationTraces);
        }
        runtimes
    }

    /// Claim one of this project's attempts and carry it out, or report that
    /// there was none.
    ///
    /// The same three calls in the same order as [`Reactor::poll_once`], with
    /// the executor built in between rather than looked up: a project's
    /// executor names one execution and one declaration, so there is no
    /// registry it could have been in.
    ///
    /// # Errors
    ///
    /// Whatever the store could not do. A refusal is not one of them.
    pub async fn poll_once(
        &self,
        now: OffsetDateTime,
    ) -> Result<Performed, aiwatcher_execution::HandleError> {
        let pass = std::time::Instant::now();
        let runtimes = self.runtimes();
        let claimed = match self
            .reactor
            .take(&ClaimFilter::for_runtimes(&runtimes), &runtimes, now)
            .await?
        {
            Taken::Idle => return Ok(Performed::Idle),
            Taken::Settled(performed) => return Ok(performed),
            Taken::Work(claimed) => *claimed,
        };

        let outcome = match self.executor_for(&claimed).await {
            Ok(executor) => self.reactor.perform(&executor, &claimed).await,
            // After `step.started`, so it is an outcome rather than a release:
            // the attempt is this dispatcher's and something has to say what
            // became of it.
            Err(refused) => Err(refused.as_step_error()),
        };
        let finished_at =
            now + time::Duration::try_from(pass.elapsed()).unwrap_or(time::Duration::ZERO);
        self.reactor
            .settle_at(claimed, outcome, now, finished_at)
            .await
    }

    /// Drain this project's committed facts onto the event log.
    ///
    /// The publisher's own rule, one project in: the envelope leaves carrying
    /// this store's scope, stamped by `publish_pending` rather than read off
    /// the row (ADR_0026 and ADR_0033 together). Before this, a project's
    /// outbox had no reader at all, which is why ADR_0033's Consequences said
    /// a project run has no live view — and why this is the line that changes
    /// it.
    ///
    /// # Errors
    ///
    /// Whatever the store or the sink refused. The rows stay pending.
    pub async fn publish_once(
        &self,
        sink: &dyn aiwatcher_bus::MessageSink,
    ) -> Result<aiwatcher_execution::outbox::Published, aiwatcher_execution::StoreError> {
        aiwatcher_execution::publish_pending(
            self.reactor.handler().store().as_ref(),
            sink,
            OffsetDateTime::now_utc(),
        )
        .await
    }

    /// Deliver this project's due timers.
    ///
    /// # Errors
    ///
    /// Whatever the store refused. The rows stay due.
    pub async fn fire_due_timers(
        &self,
        now: OffsetDateTime,
        limit: usize,
    ) -> Result<Vec<aiwatcher_execution::hosted::Fired>, aiwatcher_execution::hosted::HostedError>
    {
        self.reactor.handler().fire_due_timers(now, limit).await
    }

    /// Forget this project's finished executions past the window.
    ///
    /// A **scoped** sweep, which is the gap ADR_0033's Consequences named: the
    /// unscoped sweep answers for the global side only, so a deployment that
    /// turned retention on reclaimed no project history at all. Keeping is
    /// still the safe direction — a window nobody configured prunes nothing.
    ///
    /// # Errors
    ///
    /// Whatever the store refused. The rows stay.
    pub async fn prune(
        &self,
        before: OffsetDateTime,
        limit: usize,
    ) -> Result<aiwatcher_execution::Pruned, aiwatcher_execution::StoreError> {
        self.reactor.handler().store().prune(before, limit).await
    }

    /// The executor for one claimed attempt, bound to that attempt's owner.
    ///
    /// The ownership is read again here rather than carried out of
    /// [`Reactor::take`]: the reactor hands back what a caller performs, not
    /// what it decided with, and a principal threaded through a struct the
    /// executor did not build is a principal somebody can set.
    async fn executor_for(
        &self,
        claimed: &Claimed,
    ) -> Result<Arc<dyn ActivityExecutor>, ActivityError> {
        let owner = self.owner_of(&claimed.row.key.execution_id).await?;
        // The two steps around a generated answer write this project's rows
        // and read this project's fold, and neither publishes evidence — so
        // what admits them is the bound registry and the byte store beside it,
        // plus this ask, which is the second one ADR_0033 requires before
        // anything of a project's is written.
        if let Some(telemetry) = self.telemetry.as_ref()
            && matches!(
                claimed.command.step.runtime,
                RuntimeBinding::EvaluationCases(_) | RuntimeBinding::EvaluationTraces(_)
            )
        {
            editor_grant(&self.iam, owner.scope, &owner.principal).await?;
            let evaluations = Arc::new(
                self.evaluations
                    .for_project_evidence(owner.scope)
                    .map_err(|error| ActivityError::new(FailureClass::Policy, error.to_string()))?,
            );
            return Ok(match claimed.command.step.runtime {
                RuntimeBinding::EvaluationCases(_) => Arc::new(super::scoring::CasesExecutor::new(
                    evaluations,
                    self.artifacts.artifacts().clone(),
                ))
                    as Arc<dyn ActivityExecutor>,
                _ => {
                    let mut executor = super::scoring::TracesExecutor::new(
                        evaluations,
                        self.artifacts.artifacts().clone(),
                        Arc::clone(&telemetry.read_model),
                        telemetry.wait,
                    )
                    // The project's own side of the fold, off the record and
                    // from nowhere else.
                    .reading(aiwatcher_projector::ReadScope::Project(
                        owner.scope.on_the_log(),
                    ))
                    .witnessed_by(telemetry.witnesses.clone());
                    if let Some(bundles) = &telemetry.bundles {
                        executor = executor.reading_bundles_from(Arc::clone(bundles));
                    }
                    if let Some(prompts) = &telemetry.prompts {
                        executor = executor.reading_prompts_from(Arc::clone(prompts));
                    }
                    if let Some(asked) = &telemetry.asked {
                        executor = executor.reading_asked_from(Arc::clone(asked));
                    }
                    Arc::new(executor) as Arc<dyn ActivityExecutor>
                }
            });
        }
        let (RuntimeBinding::ScoreEvaluation(spec)
        | RuntimeBinding::JudgeEvaluation(spec)
        | RuntimeBinding::ExternalEvaluation(spec)) = &claimed.command.step.runtime
        else {
            return Err(ActivityError::new(
                FailureClass::Policy,
                format!(
                    "{} has no project executor",
                    claimed.command.step.runtime.kind().as_str()
                ),
            ));
        };
        let authority = ProjectAuthority::new(
            Arc::clone(&self.iam),
            owner.scope,
            owner.principal.clone(),
            claimed.row.key.execution_id.clone(),
            spec.declaration.clone(),
        );
        let mut executor = ScoreExecutor::for_project(&self.evaluations, authority)
            .map_err(|error| ActivityError::new(FailureClass::Policy, error.to_string()))?;
        // Which client the step needs is the step's own runtime, not what this
        // process happens to hold: a card that asks a judge claimed as a plain
        // measurement would fold without ever putting its questions.
        match &claimed.command.step.runtime {
            RuntimeBinding::JudgeEvaluation(_) => {
                let (judge, concurrency) = self.judge.clone().ok_or_else(|| {
                    ActivityError::transient("this process holds no judge for a project run")
                })?;
                executor = executor.judged_by(judge, concurrency);
            }
            RuntimeBinding::ExternalEvaluation(_) => {
                let (scorers, concurrency) = self.scorers.clone().ok_or_else(|| {
                    ActivityError::transient(
                        "this process holds no scorer service for a project run",
                    )
                })?;
                executor = executor.scored_by(scorers, concurrency);
                if let Some((judge, concurrency)) = self.judge.clone() {
                    executor = executor.judged_by(judge, concurrency);
                }
            }
            _ => {}
        }
        Ok(Arc::new(executor) as Arc<dyn ActivityExecutor>)
    }

    /// The record on the bound store, refused if it is not this project's.
    ///
    /// A store that could not be read is `Transient` and one that *refused* the
    /// read is not: `says_the_same_next_time` is what tells a bad moment from a
    /// boundary, and a boundary retried every pass is the refusal loop
    /// `StartRefused` exists to prevent.
    async fn owner_of(
        &self,
        execution: &aiwatcher_execution::ExecutionId,
    ) -> Result<ExecutionOwnership, ActivityError> {
        let owner = self
            .reactor
            .handler()
            .store()
            .ownership(execution)
            .await
            .map_err(|error| {
                let class = if error.says_the_same_next_time() {
                    FailureClass::Policy
                } else {
                    FailureClass::Transient
                };
                ActivityError::new(class, error.to_string())
            })?
            .ok_or_else(denied)?;
        if ExecutionScope::Project(owner.scope) != self.reactor.handler().store().scope() {
            return Err(denied());
        }
        Ok(owner)
    }
}
