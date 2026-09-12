//! The launcher's cluster, through kube-rs — the one file in `pods` that needs
//! the `kube` feature. What to ask for, and what an answer means for the
//! attempt, is decided in [`super`].

use std::collections::BTreeMap;

use async_trait::async_trait;
use futures::AsyncReadExt;
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, DeleteParams, ListParams, LogParams, PostParams};
use serde_json::Value;
use time::OffsetDateTime;

use super::log::{Kept, Tail};
use super::manifest;
use super::{Cluster, ClusterError, Created, Observed, Phase};

/// What the pod of one Job is labelled with, set by the Job controller.
///
/// The prefixed one is what a current cluster writes; the bare one is what it
/// also still writes for compatibility, and the fallback here for the same
/// reason.
const JOB_NAME_LABELS: [&str; 2] = ["batch.kubernetes.io/job-name", "job-name"];

/// Jobs and pods in one namespace.
#[derive(Clone)]
pub struct KubeCluster {
    jobs: Api<Job>,
    pods: Api<Pod>,
    namespace: String,
}

impl std::fmt::Debug for KubeCluster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KubeCluster")
            .field("namespace", &self.namespace)
            .finish_non_exhaustive()
    }
}

impl KubeCluster {
    /// A client from the pod's own service account inside a cluster, or from
    /// the kubeconfig outside one.
    ///
    /// `namespace` is `AIWATCHER_POD_NAMESPACE`; absent, the client's own —
    /// in a cluster, the namespace of the service account, which is the
    /// release's.
    ///
    /// # Errors
    ///
    /// [`ClusterError::Unavailable`] when no configuration could be read.
    pub async fn connect(namespace: Option<&str>) -> Result<Self, ClusterError> {
        provider();
        let client = kube::Client::try_default()
            .await
            .map_err(|error| ClusterError::Unavailable(error.to_string()))?;
        let namespace =
            namespace.map_or_else(|| client.default_namespace().to_owned(), str::to_owned);
        Ok(Self {
            jobs: Api::namespaced(client.clone(), &namespace),
            pods: Api::namespaced(client, &namespace),
            namespace,
        })
    }

    #[must_use]
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Everything this release launched, and nothing else.
    ///
    /// Derived from the labels the manifest writes rather than spelled again:
    /// a selector that had drifted from them would quietly watch nothing.
    fn selector() -> String {
        manifest::LABELS
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// Say which crypto provider this process uses, before the first connection.
///
/// rustls asks the process, not the library, and it refuses to guess when more
/// than one provider is compiled in — which is the state this workspace is in
/// whenever `laser` is on, because `iggy` brings `ring` and reqwest brings
/// `aws-lc-rs`. Left unsaid, every call to the cluster panicked inside rustls
/// rather than failing as a port error, and the launcher's task died with it.
///
/// An `Err` means somebody installed one first, which answers the same
/// question; what matters is that one is installed by the time a client is
/// built.
fn provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

#[async_trait]
impl Cluster for KubeCluster {
    async fn create_job(&self, manifest: &Value) -> Result<Created, ClusterError> {
        // Read back into the typed Job first, so a template whose fragment is
        // not a pod spec is refused here, with serde's words, rather than
        // sent and refused in the API server's.
        let job: Job = serde_json::from_value(manifest.clone()).map_err(|error| {
            ClusterError::Refused(format!("the manifest is not a Job: {error}"))
        })?;
        match self.jobs.create(&PostParams::default(), &job).await {
            Ok(_) => Ok(Created::New),
            // The name is derived from the attempt, so this is another
            // launcher — or this one, before a restart — having asked first.
            Err(kube::Error::Api(status)) if status.code == 409 => Ok(Created::AlreadyExisted),
            // The cluster read the manifest and will not have it. Every pass
            // would get the same answer until somebody edits the template.
            Err(kube::Error::Api(status)) if matches!(status.code, 400 | 422) => {
                Err(ClusterError::Refused(status.message.clone()))
            }
            // A missing grant, a quota, a 5xx, a connection: any of them may
            // answer differently on the next pass.
            Err(error) => Err(ClusterError::Unavailable(error.to_string())),
        }
    }

    async fn jobs(&self) -> Result<Vec<Observed>, ClusterError> {
        let selector = Self::selector();
        let listing = ListParams::default().labels(&selector);
        let jobs = self
            .jobs
            .list(&listing)
            .await
            .map_err(|error| ClusterError::Unavailable(error.to_string()))?;
        // One listing rather than a read per Job: a fan-out is many pods of one
        // run, and the pod is only here to say *why*.
        let pods = self
            .pods
            .list(&listing)
            .await
            .map_err(|error| ClusterError::Unavailable(error.to_string()))?;
        let mut by_job: BTreeMap<String, &Pod> = BTreeMap::new();
        for pod in &pods.items {
            if let Some(job) = job_of(pod) {
                by_job.insert(job, pod);
            }
        }

        Ok(jobs
            .items
            .iter()
            .filter_map(|job| {
                let name = job.metadata.name.clone()?;
                let pod = by_job.get(&name).copied();
                Observed::of(
                    name.clone(),
                    job.metadata.labels.as_ref().unwrap_or(&BTreeMap::new()),
                    job.metadata
                        .annotations
                        .as_ref()
                        .unwrap_or(&BTreeMap::new()),
                    created_at(job),
                    phase_of(job, pod),
                )
            })
            .collect())
    }

    async fn log(&self, job: &str, at_most: usize) -> Result<Option<Kept>, ClusterError> {
        let Some(pod) = self
            .pods
            .list(&ListParams::default().labels(&format!("{}={job}", JOB_NAME_LABELS[0])))
            .await
            .map_err(|error| ClusterError::Unavailable(error.to_string()))?
            .items
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        let Some(name) = pod.metadata.name else {
            return Ok(None);
        };
        // Streamed rather than read whole: `limitBytes` bounds the *first* N
        // bytes of a log and the part worth keeping is the last, so the bound
        // is kept here while the stream is read.
        let mut stream = Box::pin(
            self.pods
                .log_stream(&name, &LogParams::default())
                .await
                .map_err(|error| ClusterError::Unavailable(error.to_string()))?,
        );
        let mut tail = Tail::new(at_most);
        let mut chunk = vec![0_u8; 64 * 1024];
        loop {
            let read = stream
                .read(&mut chunk)
                .await
                .map_err(|error| ClusterError::Unavailable(error.to_string()))?;
            if read == 0 {
                break;
            }
            tail.push(&chunk[..read]);
        }
        Ok(Some(tail.finish()))
    }

    async fn delete(&self, job: &str) -> Result<(), ClusterError> {
        // Background, explicitly: without a propagation policy the pod would
        // be orphaned by the delete and keep running, which is the one thing
        // a cancel is for.
        match self.jobs.delete(job, &DeleteParams::background()).await {
            Ok(_) => Ok(()),
            // Somebody got there first, which is the same outcome.
            Err(kube::Error::Api(status)) if status.code == 404 => Ok(()),
            Err(error) => Err(ClusterError::Unavailable(error.to_string())),
        }
    }
}

/// The Job a pod belongs to, off the label its controller set.
fn job_of(pod: &Pod) -> Option<String> {
    let labels = pod.metadata.labels.as_ref()?;
    JOB_NAME_LABELS
        .iter()
        .find_map(|label| labels.get(*label))
        .cloned()
}

/// When the Job was created, or now for one the cluster did not say.
///
/// Now rather than the epoch: this is the clock a start allowance runs on, and
/// the epoch would read as overdue the moment it was seen.
fn created_at(job: &Job) -> OffsetDateTime {
    job.metadata
        .creation_timestamp
        .as_ref()
        .and_then(|at| OffsetDateTime::from_unix_timestamp(at.0.as_second()).ok())
        .unwrap_or_else(OffsetDateTime::now_utc)
}

/// How far one Job's pod got.
///
/// The Job is asked first: a pod may be deleted after its Job finished, and
/// then the Job is the only thing left that knows it did.
fn phase_of(job: &Job, pod: Option<&Pod>) -> Phase {
    let counted = job
        .status
        .as_ref()
        .is_some_and(|status| status.succeeded.unwrap_or(0) > 0 || status.failed.unwrap_or(0) > 0);
    if counted {
        return Phase::Ended {
            reason: ended_reason(job, pod),
        };
    }
    // The pod may have ended before the Job's status caught up with it.
    if let Some(reason) = pod.and_then(terminated_reason) {
        return Phase::Ended { reason };
    }
    Phase::Live {
        reason: pod.and_then(unstarted_reason),
    }
}

/// The pod's own word for how it ended, falling back to the Job's.
fn ended_reason(job: &Job, pod: Option<&Pod>) -> String {
    if let Some(reason) = pod.and_then(terminated_reason) {
        return reason;
    }
    if let Some(condition) = job.status.as_ref().and_then(|status| {
        status
            .conditions
            .as_ref()?
            .iter()
            .find(|condition| condition.status == "True")
            .cloned()
    }) {
        return condition.reason.unwrap_or(condition.type_);
    }
    let succeeded = job
        .status
        .as_ref()
        .is_some_and(|status| status.succeeded.unwrap_or(0) > 0);
    if succeeded { "Completed" } else { "Failed" }.to_owned()
}

/// Why a pod that ended, ended: the pod's own reason where the kubelet set one
/// — `Evicted`, `DeadlineExceeded` — and otherwise the container's.
fn terminated_reason(pod: &Pod) -> Option<String> {
    let status = pod.status.as_ref()?;
    if let Some(reason) = status.reason.as_ref().filter(|reason| !reason.is_empty()) {
        return Some(reason.clone());
    }
    let terminated = status
        .container_statuses
        .iter()
        .flatten()
        .find_map(|container| container.state.as_ref()?.terminated.clone())?;
    let reason = terminated
        .reason
        .filter(|reason| !reason.is_empty())
        .unwrap_or_else(|| "Terminated".to_owned());
    // The exit code is what tells one failing program from another, and
    // `Error` on its own tells nobody anything.
    Some(if terminated.exit_code == 0 {
        reason
    } else {
        format!("{reason} (exit {})", terminated.exit_code)
    })
}

/// Why a pod is not running yet, when the cluster says.
fn unstarted_reason(pod: &Pod) -> Option<String> {
    let status = pod.status.as_ref()?;
    let waiting = status
        .init_container_statuses
        .iter()
        .flatten()
        .chain(status.container_statuses.iter().flatten())
        .find_map(|container| container.state.as_ref()?.waiting.clone());
    if let Some(reason) = waiting.and_then(|waiting| waiting.reason) {
        return Some(reason);
    }
    // An unschedulable pod has no container status at all: nothing has been
    // asked to start it.
    status
        .conditions
        .iter()
        .flatten()
        .find(|condition| condition.status == "False")
        .and_then(|condition| condition.reason.clone())
}
