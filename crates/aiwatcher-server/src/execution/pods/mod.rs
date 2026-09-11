//! The pod launcher: one Job per `container_job` attempt, and nothing more
//! (ADR_0029).
//!
//! It reads the attempts somebody may take, asks the cluster for a pod for
//! each, and claims none of them. The pod is the worker: it claims its
//! attempt by key through the routes every worker uses, where it meets
//! `Reactor::take` and `Reactor::settle` like any other. So once a Job
//! exists, correctness needs nothing from this loop — a pod that never comes
//! back is a lease that lapses. Exactly one Job per attempt comes from the
//! Job's name, derived from the key: a second launcher is told the Job already
//! exists, which is Kubernetes' name uniqueness doing the work a lease would.
//!
//! The one thing it decides is whether a pod may be started at all. The
//! templates are configuration and may have changed since a definition was
//! saved, so the check registration made is made again, and an attempt whose
//! template or image has stopped being allowed fails as `UserCode` — every
//! retry would get the same answer. That is reported the way a reactor
//! reports: a fact about the attempt, under an id derived from it.
//!
//! The loop and the manifest are in every build and tested against a stand-in
//! [`Cluster`]; only the client that reaches a real one is behind the `kube`
//! feature, the shape `laser` has in `aiwatcher-bus`.

pub mod manifest;

#[cfg(feature = "kube")]
pub mod kubernetes;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use time::OffsetDateTime;

use aiwatcher_core::MessageId;
use aiwatcher_execution::pods::PodTemplates;
use aiwatcher_execution::{
    AttemptKey, AttemptRow, ExecutionHandler, ExecutionId, ExecutionPlan, FailureClass,
    HandleError, MessageMetadata, Now, RuntimeBinding, RuntimeKind, StepError, WorkflowEvent,
    WorkflowMessage, WorkflowStore, derive_uuid, replay,
};

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

/// What the launcher asks of a cluster.
#[async_trait]
pub trait Cluster: Send + Sync + std::fmt::Debug {
    /// Create one Job from its manifest.
    ///
    /// # Errors
    ///
    /// [`ClusterError::Refused`] when the cluster read the manifest and will
    /// not have it, and [`ClusterError::Unavailable`] when it could not be
    /// asked or declined for now.
    async fn create_job(&self, manifest: &Value) -> Result<Created, ClusterError>;
}

/// What a create came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Created {
    New,
    /// The name is derived from the attempt, so somebody asked first: another
    /// launcher, or this one before a restart.
    AlreadyExisted,
}

/// Why a create did not happen, split by whether asking again could help.
#[derive(Debug, thiserror::Error)]
pub enum ClusterError {
    /// The cluster read the manifest and will not have it. The same answer
    /// on every pass, until somebody edits the template.
    #[error("the cluster refused the Job: {0}")]
    Refused(String),
    /// A connection, a 5xx, a quota, a missing grant. The next pass asks
    /// again, and nothing about the attempt changes meanwhile.
    #[error("the cluster could not be asked: {0}")]
    Unavailable(String),
}

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
}

enum Launch {
    Created(Created),
    Refused,
    Waiting,
}

/// The loop's state between passes.
#[derive(Debug)]
pub struct Launcher<S> {
    handler: ExecutionHandler<S>,
    templates: Arc<PodTemplates>,
    cluster: Arc<dyn Cluster>,
    settings: Settings,
    /// Plans by execution. A run's plan is pinned, so its stream is read once.
    plans: HashMap<ExecutionId, Arc<ExecutionPlan>>,
    /// Attempts whose Job exists, so that a pass does not ask the cluster
    /// again for every attempt still waiting for its pod to claim it.
    /// Forgotten once an attempt stops being claimable, which keeps both maps
    /// bounded by the claim table.
    launched: HashSet<AttemptKey>,
}

impl<S: WorkflowStore> Launcher<S> {
    #[must_use]
    pub fn new(
        handler: ExecutionHandler<S>,
        templates: Arc<PodTemplates>,
        cluster: Arc<dyn Cluster>,
        settings: Settings,
    ) -> Self {
        Self {
            handler,
            templates,
            cluster,
            settings,
            plans: HashMap::new(),
            launched: HashSet::new(),
        }
    }

    /// One pass: a Job for every claimable pod's attempt that has none yet.
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
        self.launched
            .retain(|key| rows.iter().any(|row| &row.key == key));
        self.plans
            .retain(|execution, _| rows.iter().any(|row| &row.key.execution_id == execution));

        let mut pass = Pass::default();
        for row in &rows {
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
        Ok(pass)
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
            self.refuse(
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
                self.refuse(
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

    /// End an attempt no pod was started for, the way a reactor reports one:
    /// a fact about the attempt, under an id derived from it, so a pass that
    /// failed after the append and ran again lands on the inbox entry rather
    /// than beside it.
    async fn refuse(
        &self,
        row: &AttemptRow,
        class: FailureClass,
        message: String,
        now: OffsetDateTime,
    ) -> Result<(), HandleError> {
        let key = &row.key;
        let execution = &key.execution_id;
        let message_id = MessageId::new(derive_uuid(&format!(
            "aiwatcher/execution/report/{execution}/{}/{}/launch-refused",
            key.step_id, key.attempt
        )));
        let metadata = MessageMetadata::caused_by(execution, &row.command_id, message_id, now)
            .about_step(&key.step_id, key.attempt);
        tracing::warn!(
            attempt = %key,
            class = class.as_str(),
            %message,
            "a pod's attempt failed before its pod was started"
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
            "pods are launched from this process"
        );
        let mut launcher = Launcher::new(
            ExecutionHandler::new(store),
            templates,
            Arc::new(cluster),
            settings,
        );
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use aiwatcher_execution::definition::WorkflowSpec;
    use aiwatcher_execution::store::memory::MemoryWorkflowStore;
    use aiwatcher_execution::{ExecutionMode, ExecutionOwner, WorkflowCommand};
    use serde_json::json;

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
    }

    impl StandIn {
        fn answering(answer: Answer) -> Arc<Self> {
            Arc::new(Self {
                answer: Mutex::new(answer),
                asked: Mutex::new(Vec::new()),
            })
        }

        fn asked(&self) -> Vec<Value> {
            self.asked.lock().expect("lock").clone()
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

    fn launcher(
        store: &Arc<MemoryWorkflowStore>,
        templates: Arc<PodTemplates>,
        cluster: &Arc<StandIn>,
    ) -> Launcher<Arc<MemoryWorkflowStore>> {
        Launcher::new(
            ExecutionHandler::new(Arc::clone(store)),
            templates,
            Arc::clone(cluster) as Arc<dyn Cluster>,
            Settings {
                api_url: "http://aiwatcher-server.aiwatcher.svc:8080".to_owned(),
            },
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

    #[tokio::test]
    async fn an_attempt_gets_one_job_and_is_left_for_its_pod_to_claim() {
        let (store, key) = started().await;
        let cluster = StandIn::answering(Answer::New);
        let mut launcher = launcher(&store, templates(&["ghcr.io/planner/import"]), &cluster);

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
        let mut launcher = launcher(&store, templates(&["ghcr.io/planner/import"]), &cluster);

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
        let mut launcher = launcher(&store, templates(&["ghcr.io/planner/import-v2"]), &cluster);

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
        let mut launcher = launcher(&store, templates(&["ghcr.io/planner/import"]), &cluster);

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
        let mut launcher = launcher(&store, templates(&["ghcr.io/planner/import"]), &cluster);

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
}
