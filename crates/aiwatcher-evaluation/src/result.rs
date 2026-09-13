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
    /// What answering it took, as the producer measured it. Absent from every
    /// case nobody measured, so no shard written before it moves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<CaseUsage>,
}

/// What making one case's answer took: the time the application spent on it
/// and the tokens its model calls counted, each absent where nobody measured
/// it — which is never zero.
///
/// The producer's measurement, like its answer. The trace the case names is
/// where the same calls were observed, for as long as the log keeps them;
/// this outlives it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CaseUsage {
    /// One case's answer, end to end in the application: one inference, or
    /// the several a case made. Never the whole run's time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
}

/// What a result's answers took, over its own cases, derived when it is
/// published.
///
/// Each figure says how many cases it was counted over, because a latency
/// from three cases of forty is not the variant's latency. Percentiles are of
/// this result's cases and are never combined with another result's: a mean
/// of two p90s is not a p90 of anything.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ResultUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<LatencySummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<TokenSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<TokenSummary>,
}

/// One result's per-case latency, by nearest rank.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LatencySummary {
    pub cases: usize,
    pub p50: f64,
    pub p90: f64,
    pub p99: f64,
    pub max: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TokenSummary {
    pub cases: usize,
    pub total: u64,
}

impl ResultUsage {
    /// What `cases` reported, or `None` when no case reported anything.
    #[must_use]
    pub fn of(cases: &[CaseMeasurement]) -> Option<Self> {
        let mut latencies: Vec<f64> = cases
            .iter()
            .filter_map(|case| case.usage.as_ref()?.latency_ms)
            .collect();
        latencies.sort_by(f64::total_cmp);
        let rank = |quantile: f64| {
            let at = ((quantile * latencies.len() as f64).ceil() as usize).max(1) - 1;
            latencies[at.min(latencies.len() - 1)]
        };
        let tokens = |side: fn(&CaseUsage) -> Option<u64>| {
            let counted: Vec<u64> = cases
                .iter()
                .filter_map(|case| side(case.usage.as_ref()?))
                .collect();
            (!counted.is_empty()).then(|| TokenSummary {
                cases: counted.len(),
                total: counted.iter().sum(),
            })
        };
        let usage = Self {
            latency_ms: (!latencies.is_empty()).then(|| LatencySummary {
                cases: latencies.len(),
                p50: rank(0.5),
                p90: rank(0.9),
                p99: rank(0.99),
                max: latencies[latencies.len() - 1],
            }),
            input_tokens: tokens(|usage| usage.input_tokens),
            output_tokens: tokens(|usage| usage.output_tokens),
        };
        (usage.latency_ms.is_some()
            || usage.input_tokens.is_some()
            || usage.output_tokens.is_some())
        .then_some(usage)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PublishEvaluation {
    pub manifest: EvaluationManifest,
    pub status: ResultStatus,
    /// Missing selected cases remain unscored; never implicitly successful.
    pub cases: Vec<CaseMeasurement>,
    /// How far the judge agreed with the people it was calibrated against.
    /// Present exactly when the context names a judge, and only ever written
    /// by the run that asked it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge: Option<crate::JudgeReport>,
    /// How far the card's calibrated framework metrics agreed with the people
    /// in their calibration set. Present exactly when the context pins one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<crate::ExternalReport>,
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
    /// Whether every number here can be had again by re-reading the evidence.
    /// False once a model judged any of them: those numbers are what it said
    /// at the time, and the agreement beside them is how far to trust it.
    pub reproducible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge: Option<crate::JudgeReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<crate::ExternalReport>,
    /// What its answers took, over the cases that reported it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ResultUsage>,
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
    /// When the last collection pass ran, which is when `damaged` was measured.
    /// Collection is hourly, so this is older than `ran_at` and says how much.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collected_at: Option<i64>,
    /// Published results the last collection pass found with missing bytes.
    ///
    /// A summary answers from the header, so a result whose shards are gone
    /// reads as complete until somebody opens it. Collection already lists what
    /// each result holds in order to delete the rest, so it already knows —
    /// this is that answer written down rather than a second pass to find it.
    /// Bounded: `damaged_count` is all of them, `damaged` the first few.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub damaged: Vec<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub damaged_count: usize,
}

fn is_zero(count: &usize) -> bool {
    *count == 0
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

#[cfg(test)]
mod tests {
    use super::*;

    fn case(at: usize, usage: Option<CaseUsage>) -> CaseMeasurement {
        CaseMeasurement {
            case_id: format!("case-{at}"),
            repetition_id: "measurement-1".into(),
            actual: Some(serde_json::json!("an answer")),
            metrics: BTreeMap::new(),
            error: None,
            trace_id: None,
            span_id: None,
            usage,
        }
    }

    #[test]
    fn a_result_s_usage_is_its_own_cases_by_nearest_rank_and_says_how_many_reported() {
        let mut cases: Vec<CaseMeasurement> = (1..=10)
            .map(|at| {
                case(
                    at,
                    Some(CaseUsage {
                        latency_ms: Some(at as f64 * 100.0),
                        input_tokens: (at <= 3).then_some(10),
                        output_tokens: None,
                    }),
                )
            })
            .collect();
        cases.push(case(11, None));

        let usage = ResultUsage::of(&cases).expect("ten cases reported");
        assert_eq!(
            usage.latency_ms,
            Some(LatencySummary {
                cases: 10,
                p50: 500.0,
                p90: 900.0,
                p99: 1000.0,
                max: 1000.0,
            })
        );
        assert_eq!(
            usage.input_tokens,
            Some(TokenSummary {
                cases: 3,
                total: 30
            })
        );
        assert_eq!(
            usage.output_tokens, None,
            "nobody counted, which is not zero"
        );
        assert_eq!(ResultUsage::of(&[case(1, None)]), None);
    }
}
