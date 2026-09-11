//! Terminal measurements and their durable read contract.

use crate::EvaluationManifest;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceState {
    Complete,
    Partial,
    MissingArtifact,
    CorruptArtifact,
    Expired,
    DeletedSource,
    Forbidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResultStatus {
    Succeeded,
    Failed,
    Partial,
}

/// Producer measurements only. Expected answers are resolved from the source.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CaseMeasurement {
    pub case_id: String,
    pub repetition_id: String,
    pub actual: Option<serde_json::Value>,
    pub metrics: BTreeMap<String, f64>,
    pub error: Option<String>,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PublishEvaluation {
    pub manifest: EvaluationManifest,
    pub status: ResultStatus,
    /// Missing selected cases remain unscored; never implicitly successful.
    pub cases: Vec<CaseMeasurement>,
}

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EvaluationReceipt {
    pub evaluation_id: String,
    pub version: String,
    pub variant_id: String,
    pub context_id: String,
    pub committed_at: i64,
    pub expires_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ResultCounts {
    pub selected: usize,
    pub scored: usize,
    pub failed: usize,
    pub unscored: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DurableEvaluation {
    pub receipt: EvaluationReceipt,
    pub state: EvidenceState,
    /// Absent after erasure; tombstones retain no manifest or metrics.
    pub manifest: Option<EvaluationManifest>,
    pub status: Option<ResultStatus>,
    pub counts: Option<ResultCounts>,
    pub metrics: BTreeMap<String, f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EvidenceCase {
    pub measurement: CaseMeasurement,
    pub expected: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CasePage {
    pub version: String,
    pub cases: Vec<EvidenceCase>,
    pub next_cursor: Option<String>,
    pub state: EvidenceState,
}

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DurablePage {
    pub evaluations: Vec<DurableEvaluation>,
    pub next_cursor: Option<String>,
}
