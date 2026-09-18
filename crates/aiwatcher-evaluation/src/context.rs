use std::collections::BTreeSet;

use aiwatcher_core::ArtifactRef;
use serde::{Deserialize, Serialize};

use crate::{
    DatasetReference, Result, SCHEMA_VERSION, VersionReference, digest, reference::artifact,
    require, text,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MetricDirection {
    Higher,
    Lower,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Aggregation {
    Mean,
    Sum,
    Min,
    Max,
    Rate,
    None,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MetricDefinition {
    pub name: String,
    pub unit: String,
    pub direction: MetricDirection,
    pub aggregation: Aggregation,
    /// Who measured this metric, when it was a scorer framework rather than
    /// the scorers compiled in: the adapter at its version, its metric, and the
    /// model it graded with when a model did. Absent from every metric
    /// measured otherwise, so no context from before it moves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measured_by: Option<ExternalMeasure>,
}

/// A metric a scorer framework measured, named where a reader of the result
/// sees the number.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExternalMeasure {
    /// The adapter and the framework release it ran: `deepeval` at `4.2.2`.
    pub adapter: VersionReference,
    pub metric: String,
    /// The model that graded it. A number a model gave is not reproduced by
    /// re-reading, and nothing measured how often this one agrees with people.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<VersionReference>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct JudgeConfiguration {
    pub provider: String,
    pub model: VersionReference,
    pub configuration: ArtifactRef,
    pub calibration_dataset: DatasetReference,
    /// The judge is sent words from the conversation archive — the responses a
    /// conversation cohort answered with, what they answered where the card
    /// shows it, or the answers people judged in a calibration set taken from
    /// conversation evidence. The provider keeps what it is sent outside the
    /// archive's encryption, retention and erasure.
    ///
    /// Derived, never authored, and part of the context: an operator admitting
    /// this pair admits that, and the evidence says so for as long as it is
    /// kept. Absent from the bytes when false, so no earlier context moves.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reads_archive: bool,
}

/// The calibration set a context's framework metrics were held against.
///
/// Pinned like a judge's, and for the same two reasons: an operator admitting
/// the pair admits whose judgements the metrics are measured against, and a
/// result calibrated on other people's judgements is a different claim.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CalibrationPin {
    pub calibration_dataset: DatasetReference,
    /// The set was taken from conversation evidence, so the answers people
    /// judged are sent to the scorer service — and on to the provider of a
    /// model grading the metric. Derived, never authored; absent when false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reads_archive: bool,
}

/// Evidence context is separate from variant identity: a new scorer measures
/// the same variant, and a different case manifest is a different cohort.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EvaluationContext {
    pub dataset: DatasetReference,
    pub case_manifest: ArtifactRef,
    #[schema(minimum = 1)]
    pub case_count: u64,
    /// A producer's split name, not proof of independence or permission to use it.
    pub split: String,
    pub suite: VersionReference,
    pub scorer: VersionReference,
    pub input_schema: ArtifactRef,
    pub expectations_schema: ArtifactRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge: Option<JudgeConfiguration>,
    /// The set a card's calibrated framework metrics were held against. Absent
    /// from every context whose card calibrates none, so none of those moves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_calibration: Option<CalibrationPin>,
    #[schema(min_items = 1, max_items = 128)]
    pub metrics: Vec<MetricDefinition>,
}

impl EvaluationContext {
    /// The content address of this context: the key every result measured on
    /// these cases by this card shares, whoever produced it.
    ///
    /// Identity of what was measured and how, never a verdict about it — and
    /// never a promise that any result exists. It is answerable before one
    /// does, which is what lets a lab pin its measurement and name the id its
    /// submissions will publish under ([ADR_0034](../../../docs/ADR/ADR_0034_WORKSHOP_LABS.md)).
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Encoding`] when the context cannot be canonicalised.
    pub fn id(&self) -> Result<String> {
        digest(&(SCHEMA_VERSION, self))
    }

    pub(crate) fn validate(&self) -> Result<()> {
        self.dataset.validate("context.dataset")?;
        artifact(&self.case_manifest, "context.case_manifest")?;
        require(
            self.case_count > 0,
            "context.case_count",
            "requires at least one selected case",
        )?;
        text(&self.split, "context.split")?;
        self.suite.validate("context.suite")?;
        self.scorer.validate("context.scorer")?;
        artifact(&self.input_schema, "context.input_schema")?;
        artifact(&self.expectations_schema, "context.expectations_schema")?;
        if let Some(judge) = &self.judge {
            text(&judge.provider, "context.judge.provider")?;
            judge.model.validate("context.judge.model")?;
            artifact(&judge.configuration, "context.judge.configuration")?;
            judge
                .calibration_dataset
                .validate("context.judge.calibration_dataset")?;
        }
        if let Some(pin) = &self.external_calibration {
            pin.calibration_dataset
                .validate("context.external_calibration.calibration_dataset")?;
        }
        require(
            !self.metrics.is_empty() && self.metrics.len() <= 128,
            "context.metrics",
            "requires 1–128 metric definitions",
        )?;
        let mut names = BTreeSet::new();
        for metric in &self.metrics {
            text(&metric.name, "context.metrics.name")?;
            text(&metric.unit, "context.metrics.unit")?;
            require(
                names.insert(&metric.name),
                "context.metrics",
                "metric names must be unique",
            )?;
        }
        Ok(())
    }
}
