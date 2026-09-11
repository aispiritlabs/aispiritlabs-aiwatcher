//! What the launcher asks of a cluster, and what an answer means.
//!
//! Four questions, and each is here because the launcher has one decision that
//! needs it: create the Job an attempt was dispatched for, read back what this
//! release's Jobs are doing, read a pod's output before it is gone, and delete
//! a Job whose reason to exist has ended.
//!
//! What a real cluster does with them is
//! [`kubernetes`](super::kubernetes), behind the `kube` feature; everything
//! that decides *what to do about an answer* is in [`super`] and is tested
//! against a stand-in.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::Value;
use time::OffsetDateTime;

use aiwatcher_execution::AttemptKey;

use super::log::Kept;

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

    /// Every Job this release manages in the namespace, and what its pod is
    /// doing.
    ///
    /// The cluster is the record of what was launched, rather than anything
    /// this process remembers: a launcher that restarted still has to end the
    /// attempt of a pod that died while it was away.
    ///
    /// # Errors
    ///
    /// [`ClusterError`] when the listing could not be read.
    async fn jobs(&self) -> Result<Vec<Observed>, ClusterError>;

    /// The last of what one Job's pod printed, bounded to `at_most` bytes.
    ///
    /// `None` when there is no pod to read — it was never created, or somebody
    /// deleted it — which is an absence rather than a failure.
    ///
    /// # Errors
    ///
    /// [`ClusterError`] when the log could not be read.
    async fn log(&self, job: &str, at_most: usize) -> Result<Option<Kept>, ClusterError>;

    /// Delete one Job and the pods it owns.
    ///
    /// A Job that is already gone is [`Ok`]: the launcher asks because its
    /// reason to exist has ended, and somebody else having got there first is
    /// the same outcome.
    ///
    /// # Errors
    ///
    /// [`ClusterError`] when the delete could not be made.
    async fn delete(&self, job: &str) -> Result<(), ClusterError>;
}

/// What a create came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Created {
    New,
    /// The name is derived from the attempt, so somebody asked first: another
    /// launcher, or this one before a restart.
    AlreadyExisted,
}

/// Why a call did not happen, split by whether asking again could help.
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

/// One of this release's Jobs, as the cluster describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observed {
    /// The Job's name, which is what a log read and a delete name it by.
    pub name: String,
    /// The attempt it was created for, off its annotations.
    pub key: AttemptKey,
    /// The template it was started from, off its label — which is how the
    /// watch reads a start allowance without the plan, and `None` for a Job
    /// carrying no such label.
    pub template: Option<String>,
    /// When the Job was created — the clock the start allowance runs on.
    pub created_at: OffsetDateTime,
    pub pod: Phase,
}

/// How far one Job's pod got.
///
/// The Job says *whether* it ended and the pod says *why*, which is the
/// precedence a cluster makes easy to get backwards: a pod may be deleted
/// after its Job finished, and then the only thing left that knows is the Job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Nothing has ended: the pod is being scheduled, pulling, starting or
    /// running — or has not been created at all.
    ///
    /// `reason` is the cluster's own word for why it is not running yet, when
    /// it has one: `ImagePullBackOff`, `Unschedulable`, `CrashLoopBackOff`. A
    /// pod that is simply running has none, and neither has a Job whose pod is
    /// still being scheduled.
    Live { reason: Option<String> },
    /// Every container ended, and this is the pod's own word for how:
    /// `OOMKilled`, `DeadlineExceeded`, `Error`, an exit code, `Completed`.
    Ended { reason: String },
}

impl Observed {
    /// A Job the cluster described, when its annotations say which attempt it
    /// is for.
    ///
    /// `None` for anything else that the label selector reached, which is
    /// skipped rather than guessed at — this launcher creates Jobs by a
    /// derived name and ends attempts by these annotations, and a Job it did
    /// not write is neither.
    #[must_use]
    pub fn of(
        name: String,
        labels: &BTreeMap<String, String>,
        annotations: &BTreeMap<String, String>,
        created_at: OffsetDateTime,
        pod: Phase,
    ) -> Option<Self> {
        Some(Self {
            name,
            key: super::manifest::attempt_of(annotations)?,
            template: labels.get(super::manifest::TEMPLATE_LABEL).cloned(),
            created_at,
            pod,
        })
    }

    /// Whether the start allowance has passed without the pod getting going.
    ///
    /// The Job's own `activeDeadlineSeconds` is the allowance *plus* the step's
    /// timeout, because a pod that took its whole allowance to start still
    /// gets the step's full time — so a pod that never started at all would
    /// otherwise hold its attempt for the length of work it never did.
    #[must_use]
    pub fn overdue(&self, allowance_seconds: u64, now: OffsetDateTime) -> bool {
        let allowance =
            time::Duration::seconds(i64::try_from(allowance_seconds).unwrap_or(i64::MAX));
        now >= self.created_at.saturating_add(allowance)
    }
}
