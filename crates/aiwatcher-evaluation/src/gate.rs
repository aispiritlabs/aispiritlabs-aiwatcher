//! A regression gate: whether a measured result may ship against a baseline.
//!
//! Decided here and nowhere else, from what the comparison already answered.
//! A pipeline asks and exits by the verdict; the panel asks the same route. So
//! whether two results compare, which way a metric is better and whether a
//! case regressed are `comparison`'s words — the gate only adds the policy:
//! how far worse each metric may be, and which cases must hold whatever the
//! means did.
//!
//! Four verdicts, because a pipeline has to tell them apart. `regression` is a
//! change somebody made; `incomplete` is a measurement that cannot say — a
//! scorer that failed, a case nobody answered — and never passes; `error` is a
//! pair that cannot be compared at all.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::{
    CaseChange, Comparability, EvidenceCaseDelta, EvidenceComparison, EvidenceMetricDelta,
    EvidenceState, MetricDirection, Result, ResultStatus, VersionReference, require,
};

/// What a gate holds a result to.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GatePolicy {
    /// How far worse than the baseline a metric may be, in its own unit. A
    /// metric not named here may not get worse at all.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tolerance: BTreeMap<String, f64>,
    /// Metrics shown and not held.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignore: Vec<String>,
    /// Cases that must be measured and no worse than the baseline on any
    /// metric, whatever the means did: a critical case lost is a regression
    /// even beside a better average.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub critical_cases: Vec<String>,
    /// Every generated answer must be seen on the log made on the variant's
    /// pinned prompt, model and workflow; fewer is `incomplete`. Off by
    /// default, because telemetry is best effort and a trace that never arrived
    /// contradicts nothing — a pipeline that ships only what its traces show
    /// turns it on.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub require_traces: bool,
    /// Every generated answer must also have a witness's word for what the
    /// variant pins that a witness can show — a serving host's for the model
    /// version, a gateway's for the prompt it found in the request — published
    /// under another credential than the application's, and one the
    /// deployment names in `AIWATCHER_WITNESSES` where it names any; fewer is
    /// `incomplete`. Off by default: only a host that reports its own runs can
    /// give one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub require_witness: bool,
    /// Every generated answer must also be, word for word, a reply a witness
    /// relayed for its run — or what the application said, before the reply
    /// came, it would take out of it — to a request that held its case's input
    /// and, where the variant pins a prompt, nothing but that prompt rendered,
    /// the answer not in it, with values that are each the case's input or a
    /// part of it or the reply of another call so made: which an application
    /// calling its provider around the gateway, telling the model what to say
    /// or handing it a value it made cannot show; fewer is `incomplete`. Off by
    /// default.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub require_witnessed_answer: bool,
}

impl GatePolicy {
    pub(crate) fn validate(&self) -> Result<()> {
        for (metric, tolerance) in &self.tolerance {
            require(
                tolerance.is_finite() && *tolerance >= 0.0,
                &format!("policy.tolerance.{metric}"),
                "must be a finite amount, nought or more",
            )?;
        }
        require(
            self.critical_cases.len() <= 1_000,
            "policy.critical_cases",
            "names at most a thousand cases",
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GateVerdict {
    Pass,
    Regression,
    Incomplete,
    Error,
}

/// One side of the pair, as a pipeline records it beside its verdict.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct GateSubject {
    pub evaluation_id: String,
    pub version: String,
    pub state: EvidenceState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suite: Option<VersionReference>,
    /// The code the variant pins — a commit, where a pipeline pinned one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<aiwatcher_core::ArtifactRef>,
}

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct GateMetric {
    #[serde(flatten)]
    pub delta: EvidenceMetricDelta,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerance: Option<f64>,
    pub held: bool,
    /// Worse than the baseline by more than its tolerance.
    pub regressed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct GateCase {
    pub case_id: String,
    /// Absent where neither result holds the case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<CaseChange>,
    pub held: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct GateDecision {
    pub verdict: GateVerdict,
    /// Every reason the verdict is not `pass`, in words.
    pub reasons: Vec<String>,
    pub comparability: Comparability,
    pub candidate: GateSubject,
    pub baseline: GateSubject,
    pub metrics: Vec<GateMetric>,
    pub critical: Vec<GateCase>,
}

fn subject(evaluation: &crate::DurableEvaluation) -> GateSubject {
    GateSubject {
        evaluation_id: evaluation.receipt.evaluation_id.clone(),
        version: evaluation.receipt.version.clone(),
        state: evaluation.state,
        suite: evaluation
            .manifest
            .as_ref()
            .map(|manifest| manifest.context.suite.clone()),
        code: evaluation
            .manifest
            .as_ref()
            .map(|manifest| manifest.variant.code.clone()),
    }
}

/// The verdict on a comparison, the policy, and the rows of the critical
/// cases that either result holds.
#[must_use]
pub fn decide(
    comparison: &EvidenceComparison,
    policy: &GatePolicy,
    critical_rows: &[EvidenceCaseDelta],
) -> GateDecision {
    let mut reasons = Vec::new();
    let candidate = &comparison.current;
    let error = !matches!(
        candidate.state,
        EvidenceState::Complete | EvidenceState::Partial
    ) || candidate.status == Some(ResultStatus::Failed)
        || comparison.comparability == Comparability::Incompatible;
    if error {
        reasons.push(format!(
            "the candidate cannot be held to this baseline: it is {:?}{}",
            candidate.state,
            comparison
                .reasons
                .iter()
                .map(|reason| format!("; {reason}"))
                .collect::<String>()
        ));
    }
    let incomplete = candidate
        .counts
        .as_ref()
        .is_some_and(|counts| counts.failed > 0 || counts.unscored > 0);
    if incomplete && let Some(counts) = &candidate.counts {
        reasons.push(format!(
            "the candidate's measurement is incomplete: {} of {} cases failed and {} went \
             unscored, and a measurement that could not say never passes",
            counts.failed, counts.selected, counts.unscored
        ));
    }
    let untraced =
        policy.require_traces
            && match &candidate.traces {
                None => {
                    reasons.push(
                        "the policy requires every answer seen on the variant's pins, and the \
                     candidate's answers were not generated here, so no trace was read"
                            .to_owned(),
                    );
                    true
                }
                Some(traces) if !traces.complete() => {
                    reasons.extend(traces.shortfall().into_iter().map(|missing| {
                        format!("the policy requires every answer traced: {missing}")
                    }));
                    true
                }
                Some(_) => false,
            };
    let unwitnessed = policy.require_witness
        && match &candidate.traces {
            None => {
                reasons.push(
                    "the policy requires a serving host's word for every answer's model, and the \
                     candidate's answers were not generated here, so no trace was read"
                        .to_owned(),
                );
                true
            }
            Some(traces) if !traces.witnessed() => {
                reasons.extend(traces.unwitnessed().into_iter().map(|missing| {
                    format!("the policy requires every answer witnessed: {missing}")
                }));
                true
            }
            Some(_) => false,
        };
    let unexchanged = policy.require_witnessed_answer
        && match &candidate.traces {
            None => {
                reasons.push(
                    "the policy requires every answer to be a reply a witness relayed, and the \
                     candidate's answers were not generated here, so no trace was read"
                        .to_owned(),
                );
                true
            }
            Some(traces) if !traces.answers_witnessed() => {
                reasons.extend(traces.unwitnessed_answers().into_iter().map(|missing| {
                    format!("the policy requires every answer witnessed as the reply: {missing}")
                }));
                true
            }
            Some(_) => false,
        };
    let incomplete = incomplete || untraced || unwitnessed || unexchanged;

    let ignored: BTreeSet<&str> = policy.ignore.iter().map(String::as_str).collect();
    let mut regressed = false;
    let metrics: Vec<GateMetric> = comparison
        .metrics
        .iter()
        .map(|delta| {
            let held = !ignored.contains(delta.name.as_str());
            let tolerance = policy.tolerance.get(&delta.name).copied();
            let worse_by = match (delta.delta, delta.direction) {
                (Some(change), Some(MetricDirection::Higher)) => -change,
                (Some(change), Some(MetricDirection::Lower)) => change,
                _ => 0.0,
            };
            let lost = held && worse_by > tolerance.unwrap_or(0.0) + 1e-12;
            if lost {
                regressed = true;
                reasons.push(format!(
                    "{} is worse than the baseline by {worse_by}{}",
                    delta.name,
                    tolerance.map_or_else(
                        || ", and may not get worse".to_owned(),
                        |allowed| format!(", past the {allowed} allowed")
                    )
                ));
            }
            GateMetric {
                delta: delta.clone(),
                tolerance,
                held,
                regressed: lost,
            }
        })
        .collect();

    let rows: BTreeMap<&str, &EvidenceCaseDelta> = critical_rows
        .iter()
        .map(|row| (row.case_id.as_str(), row))
        .collect();
    let critical: Vec<GateCase> = policy
        .critical_cases
        .iter()
        .map(|case_id| {
            let row = rows.get(case_id.as_str());
            let reason = match row {
                None => Some("neither result holds this case".to_owned()),
                Some(row) => match (&row.current, row.change) {
                    (None, _) => Some("the candidate never measured it".to_owned()),
                    (Some(outcome), _) if outcome.error.is_some() => Some(format!(
                        "the candidate failed it: {}",
                        outcome.error.as_deref().unwrap_or_default()
                    )),
                    (_, CaseChange::Regressed | CaseChange::Mixed) => {
                        Some("it is worse than the baseline on a metric".to_owned())
                    }
                    _ => None,
                },
            };
            if let Some(reason) = &reason {
                regressed = true;
                reasons.push(format!("critical case {case_id}: {reason}"));
            }
            GateCase {
                case_id: case_id.clone(),
                change: row.map(|row| row.change),
                held: reason.is_none(),
                reason,
            }
        })
        .collect();

    GateDecision {
        verdict: if error {
            GateVerdict::Error
        } else if regressed {
            GateVerdict::Regression
        } else if incomplete {
            GateVerdict::Incomplete
        } else {
            GateVerdict::Pass
        },
        reasons,
        comparability: comparison.comparability,
        candidate: subject(&comparison.current),
        baseline: subject(&comparison.baseline),
        metrics,
        critical,
    }
}
