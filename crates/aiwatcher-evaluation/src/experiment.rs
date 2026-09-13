//! An experiment: every result published in one context, as rows to set side
//! by side.
//!
//! A context is the content address of the cohort, the split, the suite, the
//! scorer and the metric definitions together, so the results in one are the
//! variants measured on the same cases the same way — which is what makes the
//! rows a comparison rather than a list. Nothing here is a second rule for
//! whether two results compare: a row against the chosen baseline carries
//! `comparison::compare`'s own answer, trimmed of the two headers the row
//! already is.

use serde::{Deserialize, Serialize};

use crate::{
    Comparability, DatasetReference, DurableEvaluation, EvaluationOrigin, EvidenceMetricDelta,
    EvidenceState, MetricDefinition, ResultCounts, ResultStatus, ResultUsage, VariantManifest,
    VersionReference,
};

/// The contexts results were published in, newest first.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ExperimentIndex {
    pub experiments: Vec<ExperimentEntry>,
    /// More results were published than one reading walks; the oldest
    /// contexts may be missing.
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ExperimentEntry {
    pub context_id: String,
    /// Results published in it, among those read.
    pub results: usize,
    /// How many different variants they measured.
    pub variants: usize,
    pub latest_committed_at: i64,
    /// What it measures, from its newest readable result; absent when none
    /// could be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suite: Option<VersionReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dataset: Option<DatasetReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case_count: Option<u64>,
}

/// One context's results, with each compared to the baseline when one is
/// chosen.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Experiment {
    pub context_id: String,
    /// What each metric is, from the context every row shares.
    pub metrics: Vec<MetricDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<String>,
    /// Newest first.
    pub rows: Vec<ExperimentRow>,
    /// More results in this context than one reading walks.
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ExperimentRow {
    pub evaluation_id: String,
    pub variant_id: String,
    pub state: EvidenceState,
    pub committed_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<VariantManifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<EvaluationOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ResultStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counts: Option<ResultCounts>,
    pub metrics: std::collections::BTreeMap<String, f64>,
    pub reproducible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ResultUsage>,
    /// What its cases' model calls cost at the deployment's price table, from
    /// the tokens by model its usage holds. Absent without a table, or when no
    /// case said which model it called.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<aiwatcher_core::prices::TokenCost>,
    /// Against the baseline; absent on the baseline itself and when none was
    /// chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comparison: Option<RowComparison>,
}

/// `comparison::compare`'s answer for a row, without the two headers.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RowComparison {
    pub comparability: Comparability,
    pub reasons: Vec<String>,
    pub same_variant: bool,
    pub judged: bool,
    pub metrics: Vec<EvidenceMetricDelta>,
}

impl ExperimentRow {
    pub(crate) fn of(result: DurableEvaluation, baseline: Option<&DurableEvaluation>) -> Self {
        let comparison = baseline
            .filter(|baseline| baseline.receipt.evaluation_id != result.receipt.evaluation_id)
            .map(|baseline| {
                let compared = crate::comparison::compare(result.clone(), baseline.clone());
                RowComparison {
                    comparability: compared.comparability,
                    reasons: compared.reasons,
                    same_variant: compared.same_variant,
                    judged: compared.judged,
                    metrics: compared.metrics,
                }
            });
        let manifest = result.manifest;
        Self {
            evaluation_id: result.receipt.evaluation_id,
            variant_id: result.receipt.variant_id,
            state: result.state,
            committed_at: result.receipt.committed_at,
            variant: manifest.as_ref().map(|manifest| manifest.variant.clone()),
            origin: manifest.map(|manifest| manifest.origin),
            status: result.status,
            counts: result.counts,
            metrics: result.metrics,
            reproducible: result.reproducible,
            usage: result.usage,
            cost: None,
            comparison,
        }
    }
}

impl Experiment {
    /// Price every row whose usage says which models its cases called.
    ///
    /// At the prices in force on the day each result was committed, which each
    /// cost names with the day it was read: a price is the deployment's, not
    /// the evidence's, so it is never written into a result — and a table that
    /// keeps its history prices one result the same whichever day it is read.
    #[must_use]
    pub fn priced(mut self, prices: &aiwatcher_core::prices::ModelPrices) -> Self {
        for row in &mut self.rows {
            let day = aiwatcher_core::prices::day_of(row.committed_at);
            row.cost = row
                .usage
                .as_ref()
                .filter(|usage| !usage.models.is_empty())
                .map(|usage| prices.cost_on(&usage.models, &day));
        }
        self
    }
}
