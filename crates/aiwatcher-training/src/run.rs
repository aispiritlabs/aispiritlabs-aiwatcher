//! What a training run is, and what it accumulates while it runs.
//!
//! Nothing here is an event and nothing here becomes a span. A training run is
//! a *record* that grows in place: it opens, collects a curve, and closes. See
//! ADR_0018 for why that stopped being the event log's problem.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::{Error, Result, validate_slug};

/// Where a run is.
///
/// `Running` is the *absence* of an end, not a claim that anything is
/// happening — exactly the rule the projector follows for agent runs. A
/// trainer killed by an OOM and a trainer thinking for twenty minutes look
/// identical from here, so nothing in this crate decides which one it is.
/// [`TrainingRun::last_heard_from`] is what a reader draws the line from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrainingStatus {
    #[default]
    Running,
    Succeeded,
    Failed,
    /// Stopped on purpose. Distinct from `Failed` because an early stop that
    /// hit its patience is a result, and a crash is not.
    Cancelled,
}

impl TrainingStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    #[must_use]
    pub const fn is_finished(self) -> bool {
        !matches!(self, Self::Running)
    }
}

/// One epoch: the grain a human reads a training run at.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct EpochRecord {
    pub epoch: u32,
    pub duration_ms: f64,
    /// Optimiser steps inside this epoch. Counted by the client, never sent
    /// one by one — a step is arithmetic, not a request.
    pub steps: u64,
    pub metrics: BTreeMap<String, f64>,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

/// A point on a finer series — a learning rate, a gradient norm.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct SampleRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<u64>,
    pub metrics: BTreeMap<String, f64>,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

/// Where the weights went, and what selected them. Never the weights.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct CheckpointRecord {
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<u64>,
    /// What selected it. A checkpoint with no metric is a periodic save; one
    /// with a metric is a decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(default)]
    pub best: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

/// A profiler session, as the part somebody reads in a review.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ProfileRecord {
    /// Top operators, totals, memory peak — whatever the profiler bridge
    /// produced. Free-form because the fields on a profiler event have moved
    /// between framework releases more than once.
    #[schema(value_type = Object)]
    pub summary: Value,
    /// Where the full trace is, for whoever wants to open it in a profiler UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

/// The number a run is judged on.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct BestMetric {
    pub metric: String,
    pub value: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<u32>,
}

/// One training run, with everything it accumulated.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct TrainingRun {
    pub run_id: String,
    /// The model being trained — a name, not a version. The version is what
    /// this run *produces*; see [`crate::model::ModelVersion`].
    pub model: String,
    /// `project@export-sha256` from the annotation registry. A bare project
    /// name is recorded and marks the run irreproducible rather than being
    /// refused: a smoke test on an unversioned dataset is a legitimate thing
    /// to do, and refusing it would only teach people to lie about it.
    pub dataset: String,
    /// Whether this run can be pointed at afterwards and repeated. Decided
    /// once, when the run opens, and stored rather than re-derived: a rule
    /// computed in three places is a rule three places can disagree about, and
    /// the panel is one of them.
    #[serde(default)]
    pub reproducible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub framework: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub device: String,
    /// The code that ran. A git revision, usually.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub code: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schema(value_type = Object)]
    pub params: BTreeMap<String, Value>,
    /// Set when the orchestrator launched this run, so it joins the workflow
    /// execution it belongs to. See ADR_0012 and ADR_0016.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run_id: Option<String>,

    pub status: TrainingStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub ended_at: Option<OffsetDateTime>,
    /// When this run last wrote anything. The only honest answer to "is it
    /// still alive", and the field a reader draws a stall from.
    #[serde(with = "time::serde::rfc3339")]
    pub last_heard_from: OffsetDateTime,

    #[serde(default)]
    pub epochs: Vec<EpochRecord>,
    #[serde(default)]
    pub samples: Vec<SampleRecord>,
    /// How many times the sampled series has been halved. The effective
    /// interval is the original one times two to this power, and saying so is
    /// what stops somebody reading a decimated curve as a complete one.
    #[serde(default)]
    pub sample_decimations: u32,
    #[serde(default)]
    pub checkpoints: Vec<CheckpointRecord>,
    #[serde(default)]
    pub profiles: Vec<ProfileRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best: Option<BestMetric>,
}

/// Whether a dataset reference is one anybody can reconstruct.
///
/// One condition, in one place, and it is the whole reason the annotation
/// export is content-addressed: a run recorded against a mutable dataset name
/// cannot prove what it was trained on.
#[must_use]
pub fn is_reproducible(dataset: &str) -> bool {
    dataset.contains('@')
}

impl TrainingRun {
    #[must_use]
    pub fn duration_ms(&self) -> Option<f64> {
        let end = self.ended_at?;
        Some(((end - self.started_at).as_seconds_f64() * 1000.0).max(0.0))
    }

    #[must_use]
    pub fn summary(&self) -> TrainingRunSummary {
        TrainingRunSummary {
            run_id: self.run_id.clone(),
            model: self.model.clone(),
            dataset: self.dataset.clone(),
            status: self.status,
            reproducible: self.reproducible,
            framework: self.framework.clone(),
            device: self.device.clone(),
            started_at: self.started_at,
            ended_at: self.ended_at,
            last_heard_from: self.last_heard_from,
            duration_ms: self.duration_ms(),
            epochs: self.epochs.len(),
            checkpoints: self.checkpoints.len(),
            best: self.best.clone(),
            error: self.error.clone(),
            workflow_run_id: self.workflow_run_id.clone(),
        }
    }

    /// The named metric across every epoch, for a curve.
    #[must_use]
    pub fn series(&self, metric: &str) -> Vec<(u32, f64)> {
        self.epochs
            .iter()
            .filter_map(|epoch| epoch.metrics.get(metric).map(|value| (epoch.epoch, *value)))
            .collect()
    }

    /// Every metric name any epoch reported, in a stable order.
    #[must_use]
    pub fn metric_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .epochs
            .iter()
            .flat_map(|epoch| epoch.metrics.keys().cloned())
            .collect();
        names.sort();
        names.dedup();
        names
    }
}

/// A run as it appears in a list: everything but the curve.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct TrainingRunSummary {
    pub run_id: String,
    pub model: String,
    pub dataset: String,
    pub status: TrainingStatus,
    pub reproducible: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub framework: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub device: String,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub ended_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub last_heard_from: OffsetDateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
    pub epochs: usize,
    pub checkpoints: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best: Option<BestMetric>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run_id: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct TrainingRunPage {
    pub runs: Vec<TrainingRunSummary>,
    pub total: usize,
}

/// What opens a run.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StartRunRequest {
    pub run_id: String,
    pub model: String,
    /// `project@export-sha256`, ideally.
    pub dataset: String,
    #[serde(default)]
    pub schema_version: Option<String>,
    #[serde(default)]
    pub framework: String,
    #[serde(default)]
    pub device: String,
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub params: BTreeMap<String, Value>,
    #[serde(default)]
    pub workflow_run_id: Option<String>,
}

impl StartRunRequest {
    pub fn validate(&self) -> Result<()> {
        validate_slug(&self.run_id, "a run id")?;
        if self.model.trim().is_empty() {
            return Err(Error::Invalid(
                "a training run needs a model name".to_owned(),
            ));
        }
        if self.dataset.trim().is_empty() {
            return Err(Error::Invalid(
                "a training run needs a dataset; use the annotation export's project@version"
                    .to_owned(),
            ));
        }
        Ok(())
    }
}

/// One batched write of progress.
///
/// Batched rather than four endpoints, because the client buffers: a six-hour
/// run should make one request per epoch, not four, and an epoch that is
/// retried after a network blip has to land on the epoch it already wrote
/// rather than beside it.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ProgressRequest {
    #[serde(default)]
    pub epochs: Vec<EpochInput>,
    #[serde(default)]
    pub samples: Vec<SampleInput>,
    #[serde(default)]
    pub checkpoints: Vec<CheckpointInput>,
    #[serde(default)]
    pub profiles: Vec<ProfileInput>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EpochInput {
    pub epoch: u32,
    #[serde(default)]
    pub duration_ms: f64,
    #[serde(default)]
    pub steps: u64,
    #[serde(default)]
    pub metrics: BTreeMap<String, f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SampleInput {
    #[serde(default)]
    pub step: Option<u64>,
    pub metrics: BTreeMap<String, f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckpointInput {
    pub uri: String,
    #[serde(default)]
    pub epoch: Option<u32>,
    #[serde(default)]
    pub step: Option<u64>,
    #[serde(default)]
    pub metric: Option<String>,
    #[serde(default)]
    pub value: Option<f64>,
    #[serde(default)]
    pub best: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileInput {
    #[schema(value_type = Object)]
    pub summary: Value,
    #[serde(default)]
    pub uri: Option<String>,
}

/// What closes a run.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FinishRunRequest {
    pub status: TrainingStatus,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub best: Option<BestMetric>,
}

/// How a run list is narrowed.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct RunFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TrainingStatus>,
    /// Exact `project@version`, or a bare project name to match every export
    /// of it. Both are useful and they are different questions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dataset: Option<String>,
}
