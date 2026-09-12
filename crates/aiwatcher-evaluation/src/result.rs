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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
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

/// What the last retention pass did, durably, so it survives a restart and is
/// the same answer in every replica.
///
/// A sweep that has been failing for a week looks exactly like one that had
/// nothing to retire — unless it says so. `failures` is what tells them apart;
/// `retired` and `collected` count only what *that* pass did, never a running
/// total that would keep yesterday's success on the screen.
#[derive(Clone, Debug, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RetentionReport {
    pub ran_at: i64,
    pub retired: usize,
    pub collected: usize,
    pub failures: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DurablePage {
    pub evaluations: Vec<DurableEvaluation>,
    pub next_cursor: Option<String>,
    /// Absent only where no pass has ever finished — a fresh instance, or one
    /// whose worker has never run. It is not "nothing to do".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention: Option<RetentionReport>,
}
