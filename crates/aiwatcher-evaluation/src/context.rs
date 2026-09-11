use std::collections::BTreeSet;

use aiwatcher_core::ArtifactRef;
use serde::{Deserialize, Serialize};

use crate::{DatasetReference, Result, VersionReference, reference::artifact, require, text};

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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct JudgeConfiguration {
    pub provider: String,
    pub model: VersionReference,
    pub configuration: ArtifactRef,
    pub calibration_dataset: DatasetReference,
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
    #[schema(min_items = 1, max_items = 128)]
    pub metrics: Vec<MetricDefinition>,
}

impl EvaluationContext {
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
