//! The pod launcher: one Job per `container_job` attempt, and nothing more
//! (ADR_0029, whose amendments carry the reasoning this summarises).
//!
//! It reads the attempts somebody may take, asks the cluster for a pod for
//! each, and claims none of them. The pod is the worker: it claims its attempt
//! by key through the routes every worker uses. So once a Job exists,
//! correctness needs nothing from this loop — a pod that never comes back is a
//! lease that lapses. Exactly one Job per attempt comes from the Job's name,
//! derived from the key: a second launcher is told it already exists. The one
//! thing it decides is whether a pod may be started at all, because templates
//! are configuration and may have changed since a definition was saved.
//!
//! The other half of the pass is the **watch**, which adds nothing to
//! correctness and three things to speed and explanation: a pod that ended
//! while its attempt was unfinished ends it now as `Infrastructure`, carrying
//! the cluster's own word for it; a Job no pod claimed within its template's
//! start allowance is ended and deleted; and a run that is no longer running
//! has its pods stopped as `Policy`, which is what asking a pod to stop means.
//! Then the Job's log is kept and the Job is deleted — the log is never in the
//! stream and never an output ([`log`]), so the attempt ends first. All of it
//! is in every build and tested against a stand-in [`Cluster`]; only the client
//! that reaches a real one is behind the `kube` feature.

pub mod cluster;
pub mod log;
pub mod manifest;
pub mod process;

#[cfg(feature = "kube")]
pub mod kubernetes;

pub use cluster::{Cluster, ClusterError, Created, Observed, Phase};

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use time::OffsetDateTime;

use aiwatcher_core::MessageId;
use aiwatcher_execution::pods::{DEFAULT_START_ALLOWANCE_SECONDS, PodTemplates};
use aiwatcher_execution::{
    AttemptKey, AttemptRow, ExecutionHandler, ExecutionId, ExecutionPlan, FailureClass,
    HandleError, MessageMetadata, Now, RuntimeBinding, RuntimeKind, StateType, StepError,
    WorkflowEvent, WorkflowMessage, WorkflowStore, derive_uuid, replay,
};

use log::Keeper;

/// How many attempts one pass reads.
///
/// Large on purpose. An attempt whose pod was asked for stays claimable until
/// the pod claims it, which is an image pull and a scheduling decision away,
/// and it is still read on every pass until then. A bound near the number of
/// pods one fan-out starts would let those fill every read and starve the
/// attempts behind them. The claim table holds live attempts only.
pub const READ_PER_PASS: usize = 1_000;

/// How often to look. A pod's cold start is seconds to minutes, so two
/// seconds of lateness is not the number anybody is waiting on.
pub const TICK: Duration = Duration::from_secs(2);

/// Where launched pods report.
#[derive(Clone, Debug)]
pub struct Settings {
    /// What each pod is told in `AIWATCHER_URL` (`AIWATCHER_POD_API_URL`).
    pub api_url: String,
}

/// What one pass did, counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pass {
    /// Jobs this pass created.
    pub launched: usize,
    /// Jobs somebody had already created for the attempt.
    pub already: usize,
    /// Attempts ended here, before any pod: a template or an image no longer
    /// allowed, or a manifest the cluster refused.
    pub refused: usize,
    /// Attempts left for the next pass, the cluster not having answered.
    pub waiting: usize,
    /// Attempts the watch ended: a pod that died holding one, one no pod
    /// claimed in time, or one whose run is no longer running.
    pub ended: usize,
    /// Logs stored and recorded against their attempt.
    pub logged: usize,
    /// Jobs deleted, having been read.
    pub deleted: usize,
}

enum Launch {
    Created(Created),
    Refused,
    Waiting,
}

/// Whether a run is still taking work, asked at most once per execution per
/// pass.
///
/// A fan-out is many pods of one run, so a Job at a time would be a keyed read
/// per pod per two seconds for one answer.
type Stopping = HashMap<ExecutionId, bool>;

/// The loop's state between passes.
#[derive(Debug)]
pub struct Launcher<S> {
    handler: ExecutionHandler<S>,
    templates: Arc<PodTemplates>,
    cluster: Arc<dyn Cluster>,
    settings: Settings,
    /// Where a finished pod's log goes, when this deployment has an object
    /// store to put it in.
    keeper: Option<Keeper>,
    /// Plans by execution. A run's plan is pinned, so its stream is read once.
    plans: HashMap<ExecutionId, Arc<ExecutionPlan>>,
    /// Attempts whose Job exists, so that a pass does not ask the cluster
    /// again for every attempt still waiting for its pod to claim it.
    /// Forgotten once an attempt stops being claimable, which keeps both maps
    /// bounded by the claim table.
    ///
    /// The cluster's own listing is the record and this is what a pass adds to
    /// it: a listing that could not be read leaves the launch half working
    /// from what this process asked for.
    launched: HashSet<AttemptKey>,
}

impl<S: WorkflowStore> Launcher<S> {
    #[must_use]
    pub fn new(
        handler: ExecutionHandler<S>,
        templates: Arc<PodTemplates>,
        cluster: Arc<dyn Cluster>,
        settings: Settings,
        keeper: Option<Keeper>,
    ) -> Self {
        Self {
            handler,
            templates,
            cluster,
            settings,
            keeper,
            plans: HashMap::new(),
            launched: HashSet::new(),
        }
    }

    /// One pass: what the Jobs that exist have come to, and then a Job for
    /// every claimable pod's attempt that has none yet.
    ///
    /// The watch runs first, because an ended Job is an attempt to end, and
    /// ending it before the launch half reads its row is what keeps one pass
    /// from asking for a second pod for an attempt it has just finished.
    ///
    /// # Errors
    ///
    /// Whatever the store could not do. A cluster that could not be asked is
    /// not an error here: the attempt is counted as waiting and asked for again.
    pub async fn pass(&mut self, now: OffsetDateTime) -> Result<Pass, HandleError> {
        let rows = self
            .handler
            .store()
            .claimable_attempts(RuntimeKind::ContainerJob, now, READ_PER_PASS)
            .await?;

        // The cluster is the record of what was launched — a launcher that
        // restarted still has to end the attempt of a pod that died while it
        // was away. A listing that could not be read leaves the watch for the
        // next pass and does not stop the launch: a create is idempotent by
        // name, and an attempt with no pod is the one thing waiting on this
        // loop.
        let observed = match self.cluster.jobs().await {
            Ok(jobs) => Some(jobs),
            Err(error) => {
                tracing::warn!(%error, "the launched Jobs could not be read; the next pass asks again");
                None
            }
        };

        let mut pass = Pass::default();
        let mut stopping = Stopping::new();
        let mut ended: HashSet<AttemptKey> = HashSet::new();
        if let Some(observed) = observed {
            for job in &observed {
                self.launched.insert(job.key.clone());
            }
            for job in &observed {
                if self
                    .watch(job, &rows, &mut stopping, now, &mut pass)
                    .await?
                {
                    ended.insert(job.key.clone());
                }
            }
        }

        for row in &rows {
            if ended.contains(&row.key) {
                continue;
            }
            // A run that is no longer running gets no pod, whether one was
            // ever started for this attempt or not: without this, an attempt
            // whose Job the watch has just deleted would be launched again by
            // the half below.
            if self
                .is_stopping(&row.key.execution_id, &mut stopping)
                .await?
            {
                self.stop(row, now).await?;
                pass.ended += 1;
                continue;
            }
            if self.launched.contains(&row.key) {
                continue;
            }
            match self.launch(row, now).await? {
                Launch::Created(created) => {
                    match created {
                        Created::New => pass.launched += 1,
                        Created::AlreadyExisted => pass.already += 1,
                    }
                    self.launched.insert(row.key.clone());
                }
                Launch::Refused => pass.refused += 1,
                Launch::Waiting => pass.waiting += 1,
            }
        }

        // Forget what this pass was not about, which is what keeps both of
        // these bounded by the claim table.
        self.launched
            .retain(|key| rows.iter().any(|row| &row.key == key));
        self.plans
            .retain(|execution, _| rows.iter().any(|row| &row.key.execution_id == execution));
        Ok(pass)
    }

    /// What one Job that exists has come to, and whether this ended its
    /// attempt.
    ///
    /// # Errors
    ///
    /// Whatever the store could not do. A cluster that could not be asked
    /// leaves the Job for the next pass: it is the cluster's record either
    /// way, and nothing about the attempt has changed meanwhile.
    async fn watch(
        &mut self,
        job: &Observed,
        rows: &[AttemptRow],
        stopping: &mut Stopping,
        now: OffsetDateTime,
        pass: &mut Pass,
    ) -> Result<bool, HandleError> {
        let key = &job.key;
        let claimable = rows.iter().find(|row| &row.key == key);
        let ended = match &job.pod {
            // Whatever the run is doing, this pod has ended. If its attempt is
            // still unfinished then nothing else will ever report it — the
            // lease would decide the same thing a lease later, and say
            // "crashed" where the cluster knows why.
            Phase::Ended { reason } => {
                let unfinished = self.unfinished(key, claimable).await?;
                if let Some(row) = unfinished {
                    self.report(
                        &row,
                        FailureClass::Infrastructure,
                        format!("the pod ended before its attempt was reported: {reason}"),
                        now,
                    )
                    .await?;
                    pass.ended += 1;
                    true
                } else {
                    false
                }
            }
            Phase::Live { reason } => {
                if self.is_stopping(&key.execution_id, stopping).await? {
                    let unfinished = self.unfinished(key, claimable).await?;
                    if let Some(row) = unfinished {
                        self.stop(&row, now).await?;
                        pass.ended += 1;
                    }
                    true
                } else {
                    let allowance = self.allowance_of(job.template.as_deref());
                    // Claimable means no pod has taken it: one that a pod
                    // holds is not, and one that ended has no row at all.
                    match claimable {
                        Some(row) if job.overdue(allowance, now) => {
                            let why = reason.as_ref().map_or_else(
                                || "the cluster says nothing about why".to_owned(),
                                |reason| format!("the cluster says {reason}"),
                            );
                            self.report(
                                row,
                                FailureClass::Infrastructure,
                                format!(
                                    "no pod claimed this attempt within the {allowance}s its \
                                     template allows for starting one: {why}"
                                ),
                                now,
                            )
                            .await?;
                            pass.ended += 1;
                            true
                        }
                        // Still starting, or a pod is holding it and this loop
                        // has nothing to add.
                        _ => return Ok(false),
                    }
                }
            }
        };
        self.close(job, now, pass).await;
        Ok(ended)
    }

    /// Keep the pod's log and delete the Job, in that order.
    ///
    /// A log that could not be kept leaves the Job where it is, so the next
    /// pass reads it again — and the bytes are named by their own hash, so
    /// reading one twice stores it once. `ttlSecondsAfterFinished` is what
    /// bounds that when nothing here ever succeeds.
    ///
    /// Nothing here fails a pass: the attempt has already ended, and a
    /// settlement never waits for its log.
    async fn close(&self, job: &Observed, now: OffsetDateTime, pass: &mut Pass) {
        if let Some(keeper) = &self.keeper {
            match self.cluster.log(&job.name, log::TAIL_BYTES).await {
                Ok(Some(kept)) => match keeper.keep(&job.key, &kept, now).await {
                    Ok(artifact) => {
                        tracing::info!(
                            attempt = %job.key,
                            digest = %artifact.digest,
                            bytes = kept.bytes.len(),
                            skipped = kept.skipped,
                            "a pod's log was kept"
                        );
                        pass.logged += 1;
                    }
                    Err(error) => {
                        tracing::warn!(
                            attempt = %job.key, %error,
                            "a pod's log was read and not kept; its Job stays for the next pass"
                        );
                        return;
                    }
                },
                // No pod to read: it was never created, or somebody deleted
                // it. An absence rather than a failure, and the Job still goes.
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(
                        attempt = %job.key, %error,
                        "a pod's log could not be read; its Job stays for the next pass"
                    );
                    return;
                }
            }
        }
        match self.cluster.delete(&job.name).await {
            Ok(()) => pass.deleted += 1,
            Err(error) => tracing::warn!(
                job = %job.name, %error,
                "a Job that has been read could not be deleted; its TTL is the backstop"
            ),
        }
    }

    /// The attempt's row, when it is one this loop may still end.
    ///
    /// A row that is gone is an attempt somebody reported, and one that is
    /// `awaiting_input` is a question rather than an ending — its pod exiting
    /// is the pod going back to the pool while somebody reads it, and the
    /// answer dispatches attempt *n+1* under its own key.
    async fn unfinished(
        &self,
        key: &AttemptKey,
        claimable: Option<&AttemptRow>,
    ) -> Result<Option<AttemptRow>, HandleError> {
        let row = match claimable {
            Some(row) => Some(row.clone()),
            None => self.handler.store().attempt(key).await?,
        };
        Ok(row.filter(|row| !row.state.is_terminal() && row.state != StateType::AwaitingInput))
    }

    /// Whether this run has stopped taking work.
    ///
    /// A cancel it is still carrying out, or an ending it already reached —
    /// a step that failed for the last time skips what is downstream of it and
    /// ends the run while a sibling's pod is still working. Read from the
    /// projection, which is the one keyed read written by the decision itself.
    async fn is_stopping(
        &self,
        execution: &ExecutionId,
        stopping: &mut Stopping,
    ) -> Result<bool, HandleError> {
        if let Some(answer) = stopping.get(execution) {
            return Ok(*answer);
        }
        let answer = self
            .handler
            .store()
            .projection(execution)
            .await?
            .is_some_and(|run| run.state.is_cancelling() || run.state.state_type.is_terminal());
        stopping.insert(execution.clone(), answer);
        Ok(answer)
    }

    /// End an attempt whose run is no longer running.
    ///
    /// `Policy` is the class whose own words are "cancelled", and the one thing
    /// about it that this loop depends on is that nothing retries it: a
    /// retried attempt would be a second pod for a run that is stopping. What
    /// the *run* then ends as is the decider's — a failure while cancelling is
    /// an `ExecutionCancelled`.
    async fn stop(&self, row: &AttemptRow, now: OffsetDateTime) -> Result<(), HandleError> {
        self.report(
            row,
            FailureClass::Policy,
            "the pod was stopped: its execution is no longer running".to_owned(),
            now,
        )
        .await
    }

    /// The start allowance of the template a Job was started from.
    ///
    /// From the templates this process holds rather than from the plan, because
    /// the Job carries the template's name in a label and the plan would be a
    /// stream read per Job. A template that is no longer configured gets the
    /// default: this is the clock that decides a pod never got going, and an
    /// absent one would decide "never".
    fn allowance_of(&self, template: Option<&str>) -> u64 {
        template
            .and_then(|name| self.templates.get(name))
            .map_or(DEFAULT_START_ALLOWANCE_SECONDS, |template| {
                template.start_allowance_seconds
            })
    }

    async fn launch(
        &mut self,
        row: &AttemptRow,
        now: OffsetDateTime,
    ) -> Result<Launch, HandleError> {
        let key = &row.key;
        let Some(plan) = self.plan_of(&key.execution_id).await? else {
            tracing::warn!(attempt = %key, "a pod's attempt whose execution holds no plan");
            return Ok(Launch::Waiting);
        };
        let Some(step) = plan.step(&key.step_id) else {
            tracing::warn!(attempt = %key, "a pod's attempt whose plan has no such step");
            return Ok(Launch::Waiting);
        };
        let RuntimeBinding::ContainerJob(spec) = &step.runtime else {
            tracing::warn!(
                attempt = %key,
                runtime = step.runtime.kind().as_str(),
                "a pod's attempt whose plan binds the step to something else"
            );
            return Ok(Launch::Waiting);
        };

        // The list is configuration, and it may have changed since the
        // definition was saved: the check registration made, made again.
        let refusals = self.templates.refusals(&key.step_id, &spec.pod);
        if !refusals.is_empty() {
            self.report(
                row,
                FailureClass::UserCode,
                format!("no pod was started: {}", refusals.join("; ")),
                now,
            )
            .await?;
            return Ok(Launch::Refused);
        }
        // Present: `refusals` names a template that is not configured.
        let Some(template) = self.templates.get(&spec.pod.template) else {
            return Ok(Launch::Waiting);
        };

        let job = manifest::job(&manifest::JobRequest {
            key,
            template_name: &spec.pod.template,
            template,
            pod: &spec.pod,
            timeout_seconds: step.timeout_seconds,
            api_url: &self.settings.api_url,
        });
        match self.cluster.create_job(&job).await {
            Ok(created) => {
                if created == Created::New {
                    tracing::info!(
                        attempt = %key,
                        job = %aiwatcher_execution::pods::job_name(key),
                        template = %spec.pod.template,
                        image = %spec.pod.image,
                        "a pod was asked for"
                    );
                }
                Ok(Launch::Created(created))
            }
            Err(ClusterError::Refused(why)) => {
                self.report(
                    row,
                    FailureClass::Validation,
                    format!(
                        "the cluster refused the Job for pod template '{}': {why}",
                        spec.pod.template
                    ),
                    now,
                )
                .await?;
                Ok(Launch::Refused)
            }
            Err(ClusterError::Unavailable(why)) => {
                tracing::warn!(
                    attempt = %key,
                    %why,
                    "the cluster could not be asked for a pod; the next pass asks again"
                );
                Ok(Launch::Waiting)
            }
        }
    }

    /// End an attempt, the way a reactor reports one: a fact about the
    /// attempt, under an id derived from it, so a pass that failed after the
    /// append and ran again lands on the inbox entry rather than beside it.
    ///
    /// One id per attempt rather than one per reason, and deliberately: this
    /// loop has one thing to say about any one attempt — no pod was started,
    /// the pod ended, no pod claimed it, the run stopped — and whichever it is
    /// retires the row, so a second cannot follow. Two ids would mean two
    /// passes racing on one fact landing beside each other instead of on it.
    async fn report(
        &self,
        row: &AttemptRow,
        class: FailureClass,
        message: String,
        now: OffsetDateTime,
    ) -> Result<(), HandleError> {
        let key = &row.key;
        let execution = &key.execution_id;
        let message_id = MessageId::new(derive_uuid(&format!(
            "aiwatcher/execution/report/{execution}/{}/{}/pod-launcher",
            key.step_id, key.attempt
        )));
        let metadata = MessageMetadata::caused_by(execution, &row.command_id, message_id, now)
            .about_step(&key.step_id, key.attempt);
        tracing::warn!(
            attempt = %key,
            class = class.as_str(),
            %message,
            "the pod launcher ended a pod's attempt"
        );
        let failed = WorkflowEvent::StepFailed {
            step_id: key.step_id.clone(),
            attempt: key.attempt,
            error: StepError::new(class, message),
        };
        match self
            .handler
            .handle(
                execution,
                WorkflowMessage::Event(failed),
                metadata,
                Now::at(now),
            )
            .await
        {
            // A refusal from the decider is an attempt already recorded, or
            // one that ended some other way first. Neither is this loop's.
            Ok(_) | Err(HandleError::Decision(_)) => Ok(()),
            Err(error) => Err(error),
        }
    }

    async fn plan_of(
        &mut self,
        execution: &ExecutionId,
    ) -> Result<Option<Arc<ExecutionPlan>>, HandleError> {
        if let Some(plan) = self.plans.get(execution) {
            return Ok(Some(Arc::clone(plan)));
        }
        let slice = self.handler.store().load(execution).await?;
        let Some(plan) = replay(slice.events())
            .active()
            .map(|run| Arc::new(run.plan.clone()))
        else {
            return Ok(None);
        };
        self.plans.insert(execution.clone(), Arc::clone(&plan));
        Ok(Some(plan))
    }
}

/// Start the launcher against a real cluster.
///
/// The client is built inside the task and built again after a failure,
/// rather than failing the start: outside a cluster with no kubeconfig this is
/// an error every half-minute naming what is missing, and the rest of the
/// process runs.
#[cfg(feature = "kube")]
#[must_use]
pub fn spawn(
    store: Arc<dyn WorkflowStore>,
    templates: Arc<PodTemplates>,
    settings: Settings,
    namespace: Option<String>,
    keeper: Option<Keeper>,
    shutdown: tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let cluster = loop {
            match kubernetes::KubeCluster::connect(namespace.as_deref()).await {
                Ok(cluster) => break cluster,
                Err(error) => {
                    tracing::error!(%error, "no pod can be launched until a Kubernetes client is built");
                    tokio::select! {
                        () = shutdown.cancelled() => return,
                        () = tokio::time::sleep(super::BACKOFF) => {}
                    }
                }
            }
        };
        tracing::info!(
            namespace = cluster.namespace(),
            templates = templates.len(),
            api = %settings.api_url,
            logs = keeper.is_some(),
            "pods are launched from this process"
        );
        run(
            Launcher::new(
                ExecutionHandler::new(store),
                templates,
                Arc::new(cluster),
                settings,
                keeper,
            ),
            shutdown,
        )
        .await;
    })
}

/// Start the launcher against this host, each attempt a process of its own.
///
/// Everything after the backend is the same loop, because a plan means the
/// same thing either way: what changes is what a Job is (see [`process`]).
#[must_use]
pub fn spawn_processes(
    store: Arc<dyn WorkflowStore>,
    templates: Arc<PodTemplates>,
    settings: Settings,
    limit: usize,
    keeper: Option<Keeper>,
    shutdown: tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<()> {
    let cluster = process::ProcessCluster::new(limit);
    tracing::warn!(
        limit = cluster.limit(),
        templates = templates.len(),
        api = %settings.api_url,
        logs = keeper.is_some(),
        "step pods are local processes on this host: no image is run, no resource limit is          applied, and they stop when this process does"
    );
    tokio::spawn(async move {
        run(
            Launcher::new(
                ExecutionHandler::new(store),
                templates,
                Arc::new(cluster),
                settings,
                keeper,
            ),
            shutdown,
        )
        .await;
    })
}

/// Pass, wait, and stop when asked.
async fn run<S: WorkflowStore>(
    mut launcher: Launcher<S>,
    shutdown: tokio_util::sync::CancellationToken,
) {
    loop {
        let wait = match launcher.pass(OffsetDateTime::now_utc()).await {
            Ok(_) => TICK,
            Err(error) => {
                tracing::warn!(%error, "the pod launcher could not read or report");
                super::BACKOFF
            }
        };
        tokio::select! {
            () = shutdown.cancelled() => {
                tracing::info!("the pod launcher is stopping");
                return;
            }
            () = tokio::time::sleep(wait) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use aiwatcher_core::ArtifactKind;
    use aiwatcher_core::prompts::ObjectStore;
    use aiwatcher_execution::definition::WorkflowSpec;
    use aiwatcher_execution::store::memory::MemoryWorkflowStore;
    use aiwatcher_execution::{
        ArtifactCatalog, ClaimFilter, ExecutionMode, ExecutionOwner, MemoryArtifactCatalog,
        StateType, WorkflowCommand,
    };
    use aiwatcher_prompts::adapters::memory::MemoryObjectStore;
    use async_trait::async_trait;
    use serde_json::{Value, json};

    use crate::execution::artifacts::{Artifacts, SCHEME};

    use super::cluster::{Cluster, ClusterError, Created, Observed, Phase};
    use super::log::{Keeper, Kept};

    #[derive(Clone, Copy, Debug)]
    enum Answer {
        New,
        AlreadyExisted,
        Refused,
        Unavailable,
    }

    /// A cluster that answers however the test says, and keeps what it was
    /// asked for.
    #[derive(Debug)]
    struct StandIn {
        answer: Mutex<Answer>,
        asked: Mutex<Vec<Value>>,
        /// What [`Cluster::jobs`] says exists, and whether it can be asked.
        holding: Mutex<Vec<Observed>>,
        listable: Mutex<bool>,
        /// What a pod printed, by Job name. A Job with no entry has no pod to
        /// read, which is an absence rather than a failure.
        printed: Mutex<BTreeMap<String, Kept>>,
        readable: Mutex<bool>,
        deleted: Mutex<Vec<String>>,
    }

    impl StandIn {
        fn answering(answer: Answer) -> Arc<Self> {
            Arc::new(Self {
                answer: Mutex::new(answer),
                asked: Mutex::new(Vec::new()),
                holding: Mutex::new(Vec::new()),
                listable: Mutex::new(true),
                printed: Mutex::new(BTreeMap::new()),
                readable: Mutex::new(true),
                deleted: Mutex::new(Vec::new()),
            })
        }

        fn asked(&self) -> Vec<Value> {
            self.asked.lock().expect("lock").clone()
        }

        fn deleted(&self) -> Vec<String> {
            self.deleted.lock().expect("lock").clone()
        }

        /// One Job of this release, as the cluster would describe it.
        fn holds(&self, key: &AttemptKey, created_at: OffsetDateTime, pod: Phase) {
            let name = aiwatcher_execution::pods::job_name(key);
            self.printed.lock().expect("lock").insert(
                name.clone(),
                Kept {
                    bytes: b"Traceback (most recent call last):\n".to_vec(),
                    skipped: 0,
                },
            );
            self.holding.lock().expect("lock").push(Observed {
                name,
                key: key.clone(),
                template: Some("planner-import".to_owned()),
                created_at,
                pod,
            });
        }
    }

    #[async_trait]
    impl Cluster for StandIn {
        async fn create_job(&self, manifest: &Value) -> Result<Created, ClusterError> {
            self.asked.lock().expect("lock").push(manifest.clone());
            let answer = *self.answer.lock().expect("lock");
            match answer {
                Answer::New => Ok(Created::New),
                Answer::AlreadyExisted => Ok(Created::AlreadyExisted),
                Answer::Refused => Err(ClusterError::Refused(
                    "spec.template.spec.volumes[0].name: Invalid value".to_owned(),
                )),
                Answer::Unavailable => {
                    Err(ClusterError::Unavailable("connection refused".to_owned()))
                }
            }
        }

        async fn jobs(&self) -> Result<Vec<Observed>, ClusterError> {
            if !*self.listable.lock().expect("lock") {
                return Err(ClusterError::Unavailable("connection refused".to_owned()));
            }
            Ok(self.holding.lock().expect("lock").clone())
        }

        async fn log(&self, job: &str, _at_most: usize) -> Result<Option<Kept>, ClusterError> {
            if !*self.readable.lock().expect("lock") {
                return Err(ClusterError::Unavailable("the pod is gone".to_owned()));
            }
            Ok(self.printed.lock().expect("lock").get(job).cloned())
        }

        async fn delete(&self, job: &str) -> Result<(), ClusterError> {
            self.deleted.lock().expect("lock").push(job.to_owned());
            self.holding
                .lock()
                .expect("lock")
                .retain(|held| held.name != job);
            Ok(())
        }
    }

    fn at(seconds: i64) -> OffsetDateTime {
        OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(seconds)
    }

    fn templates(images: &[&str]) -> Arc<PodTemplates> {
        Arc::new(
            PodTemplates::parse(
                json!({"planner-import": {
                    "images": images,
                    "resources": {"max": {"cpu": "2", "memory": "4Gi"}},
                    "command": ["python", "-m", "aiwatcher_sdk.worker", "run-attempt"]
                }})
                .to_string()
                .as_bytes(),
            )
            .expect("templates"),
        )
    }

    /// A run of one step asking for a pod, started, so its attempt is
    /// dispatched.
    async fn started() -> (Arc<MemoryWorkflowStore>, AttemptKey) {
        let spec: WorkflowSpec = serde_json::from_value(json!({
            "name": "pod-import", "version": "1", "steps": [
                {"id": "acquire", "task_ref": "acquire@1", "queue": "planner-import",
                 "timeout_seconds": 600,
                 "pod": {"template": "planner-import", "image": "ghcr.io/planner/import:1.4"}}
            ]
        }))
        .expect("a workflow");
        let plan = spec.compile().expect("a plan");
        let store = Arc::new(MemoryWorkflowStore::default());
        let execution = ExecutionId::new("import-1");
        ExecutionHandler::new(Arc::clone(&store))
            .handle(
                &execution,
                WorkflowMessage::Command(WorkflowCommand::StartExecution {
                    execution_id: execution.clone(),
                    plan: Box::new(plan),
                    owner: ExecutionOwner::Local,
                    mode: ExecutionMode::Compiled,
                    payloads: Default::default(),
                    requested_by: "mk".to_owned(),
                    input: BTreeMap::new(),
                }),
                MessageMetadata::caused_by(
                    &execution,
                    &MessageId::new("start"),
                    MessageId::new("start"),
                    at(0),
                ),
                Now::at(at(0)),
            )
            .await
            .expect("started");
        (store, AttemptKey::new(execution, "acquire", 1))
    }

    /// The claim a pod makes: its own attempt, by key, under its template's
    /// queue and the code the step pins.
    async fn claimed_by_its_pod(
        store: &Arc<MemoryWorkflowStore>,
        key: &AttemptKey,
        pod: &str,
        now: OffsetDateTime,
    ) {
        let filter = ClaimFilter {
            attempt: Some(key.clone()),
            runtimes: Vec::new(),
            queues: vec!["planner-import".to_owned()],
            tasks: vec!["acquire@1".to_owned()],
        };
        assert!(
            store
                .claim_attempt(&filter, pod, now)
                .await
                .expect("a claim")
                .is_some(),
            "the pod takes the attempt its Job was created for"
        );
        // And says so, which is what makes the step `running` rather than a
        // dispatched attempt nobody has taken — `Reactor::take`'s own order.
        ExecutionHandler::new(Arc::clone(store))
            .handle(
                &key.execution_id,
                WorkflowMessage::Event(WorkflowEvent::StepStarted {
                    step_id: key.step_id.clone(),
                    attempt: key.attempt,
                }),
                MessageMetadata::caused_by(
                    &key.execution_id,
                    &MessageId::new("started"),
                    MessageId::new("started"),
                    now,
                )
                .about_step(&key.step_id, key.attempt),
                Now::at(now),
            )
            .await
            .expect("the start was recorded");
    }

    async fn cancelled(store: &Arc<MemoryWorkflowStore>, key: &AttemptKey, now: OffsetDateTime) {
        ExecutionHandler::new(Arc::clone(store))
            .handle(
                &key.execution_id,
                WorkflowMessage::Command(WorkflowCommand::CancelExecution {
                    reason: "somebody pressed cancel".to_owned(),
                }),
                MessageMetadata::caused_by(
                    &key.execution_id,
                    &MessageId::new("cancel"),
                    MessageId::new("cancel"),
                    now,
                ),
                Now::at(now),
            )
            .await
            .expect("the cancel was accepted");
    }

    /// The object store and catalog a pod's log goes into, and the keeper over
    /// both.
    fn keeping() -> (Keeper, MemoryObjectStore, Arc<MemoryArtifactCatalog>) {
        let objects = MemoryObjectStore::new();
        let artifacts = Artifacts::new(Arc::new(objects.clone()));
        let catalog = Arc::new(MemoryArtifactCatalog::default());
        let keeper = Keeper::of(
            Some(&artifacts),
            Some(&(Arc::clone(&catalog) as Arc<dyn ArtifactCatalog>)),
        )
        .expect("a deployment with an object store keeps logs");
        (keeper, objects, catalog)
    }

    fn launcher(
        store: &Arc<MemoryWorkflowStore>,
        templates: Arc<PodTemplates>,
        cluster: &Arc<StandIn>,
        keeper: Option<Keeper>,
    ) -> Launcher<Arc<MemoryWorkflowStore>> {
        Launcher::new(
            ExecutionHandler::new(Arc::clone(store)),
            templates,
            Arc::clone(cluster) as Arc<dyn Cluster>,
            Settings {
                api_url: "http://aiwatcher-server.aiwatcher.svc:8080".to_owned(),
            },
            keeper,
        )
    }

    async fn failure_of(store: &MemoryWorkflowStore, key: &AttemptKey) -> StepError {
        store
            .load(&key.execution_id)
            .await
            .expect("the stream")
            .events()
            .find_map(|event| match event {
                WorkflowEvent::StepFailed {
                    step_id,
                    attempt,
                    error,
                } if *step_id == key.step_id && *attempt == key.attempt => Some(error.clone()),
                _ => None,
            })
            .expect("a failure was recorded")
    }

    async fn run_state(store: &MemoryWorkflowStore, key: &AttemptKey) -> StateType {
        store
            .projection(&key.execution_id)
            .await
            .expect("a read")
            .expect("the run")
            .state
            .state_type
    }

    #[tokio::test]
    async fn an_attempt_gets_one_job_and_is_left_for_its_pod_to_claim() {
        let (store, key) = started().await;
        let cluster = StandIn::answering(Answer::New);
        let mut launcher = launcher(
            &store,
            templates(&["ghcr.io/planner/import"]),
            &cluster,
            None,
        );

        let pass = launcher.pass(at(1)).await.expect("a pass");
        assert_eq!(
            pass,
            Pass {
                launched: 1,
                ..Pass::default()
            }
        );
        let asked = cluster.asked();
        assert_eq!(asked.len(), 1);
        assert_eq!(
            asked[0]["metadata"]["name"],
            aiwatcher_execution::pods::job_name(&key)
        );
        assert_eq!(asked[0]["spec"]["backoffLimit"], 0);
        assert_eq!(
            asked[0]["spec"]["activeDeadlineSeconds"],
            300 + 600,
            "the template's start allowance plus the step's timeout"
        );

        // It claims nothing: the row is the pod's to take, by key.
        let row = store
            .attempt(&key)
            .await
            .expect("a read")
            .expect("the row is still there");
        assert!(row.lease_owner.is_none());
        assert!(row.is_claimable(at(1)));

        // And it does not ask again for an attempt it already has a Job for.
        assert_eq!(launcher.pass(at(3)).await.expect("a pass"), Pass::default());
        assert_eq!(cluster.asked().len(), 1);
    }

    #[tokio::test]
    async fn a_second_launcher_is_told_the_job_exists_and_nothing_else_happens() {
        let (store, key) = started().await;
        let cluster = StandIn::answering(Answer::AlreadyExisted);
        let mut launcher = launcher(
            &store,
            templates(&["ghcr.io/planner/import"]),
            &cluster,
            None,
        );

        assert_eq!(
            launcher.pass(at(1)).await.expect("a pass"),
            Pass {
                already: 1,
                ..Pass::default()
            }
        );
        assert!(
            store.attempt(&key).await.expect("a read").is_some(),
            "the attempt is still waiting for the pod the first launcher started"
        );
    }

    #[tokio::test]
    async fn an_attempt_whose_image_left_its_templates_list_fails_as_user_code_without_a_pod() {
        let (store, key) = started().await;
        let cluster = StandIn::answering(Answer::New);
        // The list changed after the definition was saved.
        let mut launcher = launcher(
            &store,
            templates(&["ghcr.io/planner/import-v2"]),
            &cluster,
            None,
        );

        assert_eq!(
            launcher.pass(at(1)).await.expect("a pass"),
            Pass {
                refused: 1,
                ..Pass::default()
            }
        );
        assert!(
            cluster.asked().is_empty(),
            "no pod for an image the template no longer runs"
        );
        assert!(
            store.attempt(&key).await.expect("a read").is_none(),
            "the failure settled the row"
        );
        let failure = failure_of(&store, &key).await;
        assert_eq!(
            failure.class,
            FailureClass::UserCode,
            "every retry would be refused the same way"
        );
        assert!(
            failure.message.contains("ghcr.io/planner/import:1.4")
                && failure.message.contains("planner-import"),
            "naming the image and the template: {}",
            failure.message
        );
    }

    #[tokio::test]
    async fn a_job_the_cluster_refuses_fails_its_attempt_naming_the_template() {
        let (store, key) = started().await;
        let cluster = StandIn::answering(Answer::Refused);
        let mut launcher = launcher(
            &store,
            templates(&["ghcr.io/planner/import"]),
            &cluster,
            None,
        );

        assert_eq!(
            launcher.pass(at(1)).await.expect("a pass"),
            Pass {
                refused: 1,
                ..Pass::default()
            }
        );
        let failure = failure_of(&store, &key).await;
        assert_eq!(failure.class, FailureClass::Validation);
        assert!(
            failure.message.contains("planner-import") && failure.message.contains("Invalid value"),
            "{}",
            failure.message
        );
    }

    #[tokio::test]
    async fn a_cluster_that_cannot_be_asked_leaves_the_attempt_waiting_and_is_asked_again() {
        let (store, key) = started().await;
        let cluster = StandIn::answering(Answer::Unavailable);
        let mut launcher = launcher(
            &store,
            templates(&["ghcr.io/planner/import"]),
            &cluster,
            None,
        );

        assert_eq!(
            launcher.pass(at(1)).await.expect("a pass"),
            Pass {
                waiting: 1,
                ..Pass::default()
            }
        );
        assert!(
            store.attempt(&key).await.expect("a read").is_some(),
            "nothing ran, so nothing failed"
        );

        *cluster.answer.lock().expect("lock") = Answer::New;
        assert_eq!(
            launcher.pass(at(3)).await.expect("a pass"),
            Pass {
                launched: 1,
                ..Pass::default()
            }
        );
        assert_eq!(cluster.asked().len(), 2);
    }

    #[tokio::test]
    async fn a_launcher_that_cannot_list_still_starts_a_pod_for_a_new_attempt() {
        // A create is idempotent by name, so launching without knowing what
        // exists is safe — and an attempt with no pod is the one thing
        // waiting on this loop.
        let (store, _key) = started().await;
        let cluster = StandIn::answering(Answer::New);
        *cluster.listable.lock().expect("lock") = false;
        let mut launcher = launcher(
            &store,
            templates(&["ghcr.io/planner/import"]),
            &cluster,
            None,
        );

        assert_eq!(
            launcher.pass(at(1)).await.expect("a pass"),
            Pass {
                launched: 1,
                ..Pass::default()
            }
        );
    }

    #[tokio::test]
    async fn a_pod_that_died_holding_its_attempt_ends_it_with_the_clusters_own_reason() {
        let (store, key) = started().await;
        let cluster = StandIn::answering(Answer::New);
        let (keeper, objects, catalog) = keeping();
        let mut launcher = launcher(
            &store,
            templates(&["ghcr.io/planner/import"]),
            &cluster,
            Some(keeper),
        );
        launcher.pass(at(1)).await.expect("a pass");
        claimed_by_its_pod(&store, &key, "aiwatcher-abc-x7", at(2)).await;

        // It exceeded its memory limit while it held the attempt.
        cluster.holds(
            &key,
            at(1),
            Phase::Ended {
                reason: "OOMKilled".to_owned(),
            },
        );
        let pass = launcher.pass(at(30)).await.expect("a pass");
        assert_eq!(
            pass,
            Pass {
                ended: 1,
                logged: 1,
                deleted: 1,
                ..Pass::default()
            }
        );

        let failure = failure_of(&store, &key).await;
        assert_eq!(
            failure.class,
            FailureClass::Infrastructure,
            "the work was lost rather than answered, so the tighter budget decides"
        );
        assert!(
            failure.message.contains("OOMKilled"),
            "the cluster's own word for it, rather than a lapsed lease's: {}",
            failure.message
        );
        assert!(
            store.attempt(&key).await.expect("a read").is_none(),
            "the ending retired the row"
        );

        // The log is kept against the attempt, and never on the step.
        let kept = catalog
            .produced_by(&key.execution_id)
            .await
            .expect("the catalog");
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].artifact.kind, ArtifactKind::Log);
        assert_eq!(
            kept[0].produced_by.as_ref().map(|by| by.step_id.as_str()),
            Some("acquire")
        );
        let bytes = objects
            .get(
                kept[0]
                    .artifact
                    .uri
                    .strip_prefix(SCHEME)
                    .expect("an object in this deployment's store"),
            )
            .await
            .expect("a read")
            .expect("the bytes");
        assert_eq!(bytes, b"Traceback (most recent call last):\n");
        assert_eq!(
            cluster.deleted(),
            vec![aiwatcher_execution::pods::job_name(&key)]
        );
    }

    #[tokio::test]
    async fn a_pod_that_reported_before_it_ended_is_read_and_deleted_and_nothing_is_decided() {
        let (store, key) = started().await;
        let cluster = StandIn::answering(Answer::New);
        let (keeper, _objects, catalog) = keeping();
        let mut launcher = launcher(
            &store,
            templates(&["ghcr.io/planner/import"]),
            &cluster,
            Some(keeper),
        );
        launcher.pass(at(1)).await.expect("a pass");
        // The pod claimed, did the work and reported: its row is gone, which
        // is what a settlement is.
        claimed_by_its_pod(&store, &key, "aiwatcher-abc-x7", at(2)).await;
        ExecutionHandler::new(Arc::clone(&store))
            .handle(
                &key.execution_id,
                WorkflowMessage::Event(WorkflowEvent::StepCompleted {
                    step_id: key.step_id.clone(),
                    attempt: key.attempt,
                    outputs: Vec::new(),
                    result: None,
                }),
                MessageMetadata::caused_by(
                    &key.execution_id,
                    &MessageId::new("report"),
                    MessageId::new("report"),
                    at(20),
                )
                .about_step(&key.step_id, key.attempt),
                Now::at(at(20)),
            )
            .await
            .expect("the report was accepted");

        cluster.holds(
            &key,
            at(1),
            Phase::Ended {
                reason: "Completed".to_owned(),
            },
        );
        let pass = launcher.pass(at(30)).await.expect("a pass");
        assert_eq!(
            pass,
            Pass {
                logged: 1,
                deleted: 1,
                ..Pass::default()
            },
            "a pod that printed after it reported is still read in full"
        );
        assert_eq!(
            run_state(&store, &key).await,
            StateType::Completed,
            "the watch decided nothing about an attempt somebody reported"
        );
        assert_eq!(
            catalog
                .produced_by(&key.execution_id)
                .await
                .expect("the catalog")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn a_job_no_pod_claimed_within_the_start_allowance_is_ended_and_deleted() {
        let (store, key) = started().await;
        let cluster = StandIn::answering(Answer::New);
        let mut launcher = launcher(
            &store,
            templates(&["ghcr.io/planner/import"]),
            &cluster,
            None,
        );
        launcher.pass(at(1)).await.expect("a pass");

        // The image does not exist, so the pod never starts and never claims.
        cluster.holds(
            &key,
            at(1),
            Phase::Live {
                reason: Some("ImagePullBackOff".to_owned()),
            },
        );
        assert_eq!(
            launcher.pass(at(120)).await.expect("a pass"),
            Pass::default(),
            "inside the allowance there is nothing to say: a pull takes minutes"
        );

        let pass = launcher.pass(at(302)).await.expect("a pass");
        assert_eq!(
            pass,
            Pass {
                ended: 1,
                deleted: 1,
                ..Pass::default()
            }
        );
        let failure = failure_of(&store, &key).await;
        assert_eq!(
            failure.class,
            FailureClass::Infrastructure,
            "nothing ran, so the budget decides"
        );
        assert!(
            failure.message.contains("ImagePullBackOff") && failure.message.contains("300"),
            "naming what the cluster said and the allowance it passed: {}",
            failure.message
        );
    }

    #[tokio::test]
    async fn a_cancel_deletes_a_running_pods_job_and_the_run_ends_cancelled() {
        let (store, key) = started().await;
        let cluster = StandIn::answering(Answer::New);
        let mut launcher = launcher(
            &store,
            templates(&["ghcr.io/planner/import"]),
            &cluster,
            None,
        );
        launcher.pass(at(1)).await.expect("a pass");
        claimed_by_its_pod(&store, &key, "aiwatcher-abc-x7", at(2)).await;
        cluster.holds(&key, at(1), Phase::Live { reason: None });

        cancelled(&store, &key, at(10)).await;
        assert_eq!(
            run_state(&store, &key).await,
            StateType::Running,
            "a cancel is cooperative: what is in flight is asked to stop"
        );

        let pass = launcher.pass(at(11)).await.expect("a pass");
        assert_eq!(
            pass,
            Pass {
                ended: 1,
                deleted: 1,
                ..Pass::default()
            }
        );
        assert_eq!(
            cluster.deleted(),
            vec![aiwatcher_execution::pods::job_name(&key)],
            "for a pod, asking it to stop is deleting its Job"
        );
        let failure = failure_of(&store, &key).await;
        assert_eq!(
            failure.class,
            FailureClass::Policy,
            "nothing retries a Policy failure, so no second pod is started"
        );
        assert_eq!(
            run_state(&store, &key).await,
            StateType::Cancelled,
            "the attempt's ending is what the cancel was waiting for"
        );
    }

    #[tokio::test]
    async fn a_claimable_attempt_of_a_stopping_run_is_ended_rather_than_given_a_second_pod() {
        // The row a cancel leaves behind is the one a pod had *claimed*: an
        // attempt only dispatched is skipped and its row settled with it. This
        // one's pod died with the launcher down, so by the time a pass runs
        // the lease has lapsed, the Job is gone and the row is claimable
        // again — and without this guard the half below the watch would start
        // a second pod for a run that is stopping.
        let (store, key) = started().await;
        let cluster = StandIn::answering(Answer::New);
        let mut launcher = launcher(
            &store,
            templates(&["ghcr.io/planner/import"]),
            &cluster,
            None,
        );
        launcher.pass(at(1)).await.expect("a pass");
        claimed_by_its_pod(&store, &key, "aiwatcher-abc-x7", at(2)).await;
        cancelled(&store, &key, at(10)).await;
        cluster
            .delete(&aiwatcher_execution::pods::job_name(&key))
            .await
            .expect("deleted");

        let lapsed = at(aiwatcher_jobs::LEASE_SECONDS + 20);
        let pass = launcher.pass(lapsed).await.expect("a pass");
        assert_eq!(
            pass,
            Pass {
                ended: 1,
                ..Pass::default()
            }
        );
        assert_eq!(
            cluster.asked().len(),
            1,
            "the one Job of the first pass, and no second pod for a run that is stopping"
        );
        assert_eq!(run_state(&store, &key).await, StateType::Cancelled);
    }

    #[tokio::test]
    async fn a_pod_that_ended_having_asked_a_question_is_left_for_the_answer() {
        // A park keeps its row because a question is not an ending, and the
        // answer dispatches attempt n+1 under its own key. A watch that ended
        // this would end a step somebody is being asked about.
        let (store, key) = started().await;
        let cluster = StandIn::answering(Answer::New);
        let mut launcher = launcher(
            &store,
            templates(&["ghcr.io/planner/import"]),
            &cluster,
            None,
        );
        launcher.pass(at(1)).await.expect("a pass");
        claimed_by_its_pod(&store, &key, "aiwatcher-abc-x7", at(2)).await;
        ExecutionHandler::new(Arc::clone(&store))
            .handle(
                &key.execution_id,
                WorkflowMessage::Event(WorkflowEvent::InputRequested {
                    step_id: key.step_id.clone(),
                    attempt: key.attempt,
                    request: aiwatcher_execution::state::InputRequest {
                        prompt: "keep going?".to_owned(),
                        role: "editor".to_owned(),
                        choices: Vec::new(),
                        deadline: None,
                        on_timeout: None,
                    },
                }),
                MessageMetadata::caused_by(
                    &key.execution_id,
                    &MessageId::new("ask"),
                    MessageId::new("ask"),
                    at(20),
                )
                .about_step(&key.step_id, key.attempt),
                Now::at(at(20)),
            )
            .await
            .expect("the question was recorded");

        cluster.holds(
            &key,
            at(1),
            Phase::Ended {
                reason: "Completed".to_owned(),
            },
        );
        let pass = launcher.pass(at(30)).await.expect("a pass");
        assert_eq!(
            pass,
            Pass {
                deleted: 1,
                ..Pass::default()
            },
            "the Job is read and deleted, and the attempt is nobody else's to end"
        );
        let row = store
            .attempt(&key)
            .await
            .expect("a read")
            .expect("a parked attempt keeps its row");
        assert_eq!(row.state, StateType::AwaitingInput);
    }

    #[tokio::test]
    async fn a_log_that_could_not_be_read_leaves_the_job_for_the_next_pass() {
        let (store, key) = started().await;
        let cluster = StandIn::answering(Answer::New);
        let (keeper, _objects, catalog) = keeping();
        let mut launcher = launcher(
            &store,
            templates(&["ghcr.io/planner/import"]),
            &cluster,
            Some(keeper),
        );
        launcher.pass(at(1)).await.expect("a pass");
        claimed_by_its_pod(&store, &key, "aiwatcher-abc-x7", at(2)).await;
        cluster.holds(
            &key,
            at(1),
            Phase::Ended {
                reason: "Error (exit 1)".to_owned(),
            },
        );
        *cluster.readable.lock().expect("lock") = false;

        let pass = launcher.pass(at(30)).await.expect("a pass");
        assert_eq!(
            pass,
            Pass {
                ended: 1,
                ..Pass::default()
            },
            "the attempt ends either way: a settlement never waits for its log"
        );
        assert!(
            cluster.deleted().is_empty(),
            "the Job stays, so the next pass reads it again"
        );

        *cluster.readable.lock().expect("lock") = true;
        let pass = launcher.pass(at(32)).await.expect("a pass");
        assert_eq!(
            pass,
            Pass {
                logged: 1,
                deleted: 1,
                // The ending was retryable, so the budget scheduled attempt
                // two — a new key, a new Job, and this pass starts it.
                launched: 1,
                ..Pass::default()
            },
            "and the second read is what keeps it"
        );
        assert_eq!(
            catalog
                .produced_by(&key.execution_id)
                .await
                .expect("the catalog")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn a_deployment_with_no_object_store_keeps_no_log_and_still_deletes_the_job() {
        let (store, key) = started().await;
        let cluster = StandIn::answering(Answer::New);
        let mut launcher = launcher(
            &store,
            templates(&["ghcr.io/planner/import"]),
            &cluster,
            None,
        );
        launcher.pass(at(1)).await.expect("a pass");
        claimed_by_its_pod(&store, &key, "aiwatcher-abc-x7", at(2)).await;
        cluster.holds(
            &key,
            at(1),
            Phase::Ended {
                reason: "DeadlineExceeded".to_owned(),
            },
        );

        let pass = launcher.pass(at(30)).await.expect("a pass");
        assert_eq!(
            pass,
            Pass {
                ended: 1,
                deleted: 1,
                ..Pass::default()
            },
            "nowhere to put a log is an absence, not a failure"
        );
    }
}
