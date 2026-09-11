//! The launcher's cluster, through kube-rs — the one file in `pods` that needs
//! the `kube` feature. What to ask for, and what an answer means for the
//! attempt, is decided in [`super`].

use async_trait::async_trait;
use k8s_openapi::api::batch::v1::Job;
use kube::api::{Api, PostParams};
use serde_json::Value;

use super::{Cluster, ClusterError, Created};

/// Jobs in one namespace.
#[derive(Clone)]
pub struct KubeCluster {
    jobs: Api<Job>,
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
        let client = kube::Client::try_default()
            .await
            .map_err(|error| ClusterError::Unavailable(error.to_string()))?;
        let namespace =
            namespace.map_or_else(|| client.default_namespace().to_owned(), str::to_owned);
        Ok(Self {
            jobs: Api::namespaced(client, &namespace),
            namespace,
        })
    }

    #[must_use]
    pub fn namespace(&self) -> &str {
        &self.namespace
    }
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
}
