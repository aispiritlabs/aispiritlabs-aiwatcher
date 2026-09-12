//! Two durable results, and whether their numbers may be subtracted.
//!
//! The folded half of this question compares five optional strings a producer
//! may or may not have sent, so most of its rule is about absence. Durable
//! evidence has no absence to reason about: `context_id` is the content
//! address of the cohort, the split, the suite, the scorer, the schemas and
//! the metric definitions together, so two results share a context or they do
//! not. That equality *is* the comparability rule, and everything else here is
//! either saying which field moved — for a reader who has to fix it — or
//! saying that the evidence behind a number cannot be read right now.
use aiwatcher_core::Comparability;
use serde::{Deserialize, Serialize};

use crate::{
    CaseMeasurement, DurableEvaluation, EvaluationContext, EvidenceState, MetricDefinition,
    MetricDirection, ResultStatus,
};

/// One metric on both sides, with what the context says it means.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EvidenceMetricDelta {
    pub name: String,
    /// From the context's own definition, so a reader is not guessing whether
    /// a number is seconds or a rate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Which way is better, *declared* by whoever pinned the context. Absent
    /// where neither side declares the metric — a number nobody defined.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<MetricDirection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<f64>,
    /// `current - baseline`, and `None` unless the two are comparable and both
    /// reported it. A withheld delta is not a delta of zero.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta: Option<f64>,
}

/// One durable result against another named one.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EvidenceComparison {
    pub current: DurableEvaluation,
    pub baseline: DurableEvaluation,
    pub comparability: Comparability,
    pub reasons: Vec<String>,
    /// Both sides pin the same variant, so a delta here measures repetition
    /// rather than a change. Comparable, and not the question somebody thinks
    /// they are asking — which is why it is a field rather than a reason.
    pub same_variant: bool,
    /// Every metric either side reported or the context declared, so one that
    /// appeared, disappeared or was never measured at all is visible.
    pub metrics: Vec<EvidenceMetricDelta>,
}

fn says(state: EvidenceState) -> &'static str {
    match state {
        EvidenceState::Complete => "complete",
        EvidenceState::Partial => "incomplete",
        EvidenceState::MissingArtifact => "missing its artifact",
        EvidenceState::CorruptArtifact => "corrupt",
        EvidenceState::Expired => "expired",
        EvidenceState::DeletedSource => "gone with its source",
        // Three different things produce this state and the header cannot say
        // which; the row's own badge is where a reader is told them.
        EvidenceState::Forbidden => "not readable",
    }
}

/// What moved between two pinned contexts, for a reader who has to fix it.
fn differences(current: &EvaluationContext, baseline: &EvaluationContext) -> Vec<String> {
    let mut moved = Vec::new();
    for (name, differs) in [
        ("dataset", current.dataset != baseline.dataset),
        (
            "selected cases",
            current.case_manifest != baseline.case_manifest,
        ),
        ("split", current.split != baseline.split),
        ("suite", current.suite != baseline.suite),
        ("scorer", current.scorer != baseline.scorer),
        (
            "case schemas",
            current.input_schema != baseline.input_schema
                || current.expectations_schema != baseline.expectations_schema,
        ),
        ("judge", current.judge != baseline.judge),
        ("metric definitions", current.metrics != baseline.metrics),
    ] {
        if differs {
            moved.push(format!("Different {name}"));
        }
    }
    moved
}

/// A read policy over published evidence, not a promotion policy.
pub(crate) fn comparability(
    current: &DurableEvaluation,
    baseline: &DurableEvaluation,
) -> (Comparability, Vec<String>) {
    let mut incompatible = Vec::new();
    let mut unproven = Vec::new();
    if current.receipt.evaluation_id == baseline.receipt.evaluation_id {
        incompatible.push("An evaluation cannot be its own baseline".to_owned());
    }
    if current.receipt.context_id != baseline.receipt.context_id {
        incompatible.push("Different evaluation context".to_owned());
        // A tombstone keeps no manifest, so a retired side can say that the
        // context differs and nothing about how.
        if let (Some(left), Some(right)) = (&current.manifest, &baseline.manifest) {
            incompatible.extend(differences(&left.context, &right.context));
        }
    }
    for (label, result) in [("This result", current), ("The baseline", baseline)] {
        match (result.state, result.status) {
            (EvidenceState::Complete, _) => {}
            (EvidenceState::Partial, Some(ResultStatus::Failed)) => {
                incompatible.push(format!("{label} recorded a failed measurement"));
            }
            (EvidenceState::Partial, _) => {
                unproven.push(format!("{label} left cases unscored or failed"));
            }
            (state, _) => unproven.push(format!("{label}'s evidence is {}", says(state))),
        }
    }
    if !incompatible.is_empty() {
        incompatible.extend(unproven);
        (Comparability::Incompatible, incompatible)
    } else if !unproven.is_empty() {
        (Comparability::Unverified, unproven)
    } else {
        (Comparability::Comparable, Vec::new())
    }
}

/// Pure: two headers in, one answer out. The registry supplies the headers.
#[must_use]
pub(crate) fn compare(
    current: DurableEvaluation,
    baseline: DurableEvaluation,
) -> EvidenceComparison {
    let (comparability, reasons) = comparability(&current, &baseline);
    let comparable = comparability == Comparability::Comparable;
    let contexts: Vec<&EvaluationContext> = [&current.manifest, &baseline.manifest]
        .into_iter()
        .flatten()
        .map(|manifest| &manifest.context)
        .collect();

    let mut names: Vec<&String> = current.metrics.keys().collect();
    names.extend(baseline.metrics.keys());
    names.extend(
        contexts
            .iter()
            .flat_map(|context| context.metrics.iter().map(|metric| &metric.name)),
    );
    names.sort_unstable();
    names.dedup();

    let metrics = names
        .into_iter()
        .map(|name| {
            let defined = contexts
                .iter()
                .find_map(|context| context.metrics.iter().find(|metric| &metric.name == name));
            let now = current.metrics.get(name).copied();
            let then = baseline.metrics.get(name).copied();
            EvidenceMetricDelta {
                name: name.clone(),
                unit: defined.map(|metric| metric.unit.clone()),
                direction: defined.map(|metric| metric.direction),
                current: now,
                baseline: then,
                delta: now
                    .zip(then)
                    .filter(|_| comparable)
                    .map(|(now, then)| now - then),
            }
        })
        .collect();

    EvidenceComparison {
        same_variant: current.receipt.variant_id == baseline.receipt.variant_id,
        current,
        baseline,
        comparability,
        reasons,
        metrics,
    }
}

/// What one case did between two comparable results.
///
/// Five words rather than the two lists the folded half keeps, because the
/// evidence underneath is not the same. There a producer sent `passed` per
/// case and a regression is that boolean flipping; here a case carries the
/// declared metrics and the pinned context declares which way each of them is
/// better. So a case can lose accuracy and gain latency at once — the state a
/// single verdict would have to hide, and the one most worth arguing about
/// before a release.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CaseChange {
    /// Every metric that moved, moved the wrong way — or the case stopped
    /// being measurable at all, which is the sharpest regression there is.
    Regressed,
    Improved,
    /// Better at one thing and worse at another.
    Mixed,
    Unchanged,
    /// One of the two results never measured this case, so there is nothing to
    /// subtract. A difference between the two runs, and not a movement.
    Unmeasured,
}

/// Which cases to keep, as the *question* a reader is asking rather than as
/// the verdict a case was given.
///
/// `worse` is what a release gate reads, and it includes `mixed` deliberately:
/// a gate that hid the cases which lost something *and* gained something would
/// hide the ones somebody has to decide about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CaseFilter {
    Worse,
    Better,
    /// Anything that is not the same on both sides, absences included.
    Changed,
}

impl CaseFilter {
    #[must_use]
    pub const fn keeps(self, change: CaseChange) -> bool {
        match self {
            Self::Worse => matches!(change, CaseChange::Regressed | CaseChange::Mixed),
            Self::Better => matches!(change, CaseChange::Improved | CaseChange::Mixed),
            Self::Changed => !matches!(change, CaseChange::Unchanged),
        }
    }
}

/// What one side recorded for a case.
///
/// Its *answer* is not here. A diff is read to find which cases moved, and the
/// case page beside it is where what a case said is read — carrying both
/// responses here would make the one route that already has to walk both
/// results carry both of their contents as well.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CaseOutcome {
    /// Present where the case was attempted and failed. A failed case carries
    /// no metrics, which is why it is the one movement a diff cannot express
    /// as a number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span_id: Option<String>,
}

/// One case on both sides. Named for the evidence it comes from, because the
/// folded half already has a `CaseDelta` and the contract's components block is
/// one global namespace.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EvidenceCaseDelta {
    pub case_id: String,
    pub change: CaseChange,
    /// Absent where this side never measured the case at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<CaseOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<CaseOutcome>,
    /// Every declared metric at least one side reported, in the order the
    /// context declares them. Empty for a case that failed on both sides.
    pub metrics: Vec<EvidenceMetricDelta>,
}

/// What one page of a diff is asked for.
///
/// A struct rather than three more positional arguments beside two result IDs:
/// a cursor, a limit and a filter passed as bare `Option`s is a call whose
/// arguments have to be counted to be read.
#[derive(Clone, Copy, Debug, Default)]
pub struct DiffQuery<'a> {
    pub cursor: Option<&'a str>,
    pub limit: Option<usize>,
    pub only: Option<CaseFilter>,
}

/// One page of the diff, with the verdict that decided there was one.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CaseDiffPage {
    /// The same answer `GET .../comparison` gives, because it is the same
    /// question: rows are produced only where the two may be subtracted, and a
    /// diff over a pair the server has just refused to subtract would be the
    /// second answer to it.
    pub comparability: Comparability,
    pub reasons: Vec<String>,
    pub cases: Vec<EvidenceCaseDelta>,
    /// Another *case*, not another match: a filtered page stops at the rows it
    /// was asked for or the cases it was allowed to walk, whichever comes
    /// first. The catalogue narrowed by context already has this property, and
    /// for the same reason — the order belongs to the cases, not to the filter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub state: EvidenceState,
}

/// The state the rows on one page were read under, from two sides.
///
/// Which side is `reasons`' answer, because that sentence already exists on the
/// comparison and a second copy here would be free to disagree with it.
pub(crate) fn merged(current: EvidenceState, baseline: EvidenceState) -> EvidenceState {
    for state in [current, baseline] {
        if !matches!(state, EvidenceState::Complete | EvidenceState::Partial) {
            return state;
        }
    }
    if current == EvidenceState::Partial || baseline == EvidenceState::Partial {
        EvidenceState::Partial
    } else {
        EvidenceState::Complete
    }
}

/// Pure: one case as both sides recorded it, under the directions the pinned
/// context declares.
///
/// Exact arithmetic, with no tolerance of its own: the two numbers are what
/// two producers published, and an epsilon here would be a threshold nobody
/// declared deciding which of their measurements count.
pub(crate) fn diff_case(
    declared: &[MetricDefinition],
    current: Option<&CaseMeasurement>,
    baseline: Option<&CaseMeasurement>,
) -> EvidenceCaseDelta {
    fn outcome(case: &CaseMeasurement) -> CaseOutcome {
        CaseOutcome {
            error: case.error.clone(),
            trace_id: case.trace_id.clone(),
            span_id: case.span_id.clone(),
        }
    }

    let case_id = current
        .or(baseline)
        .map_or_else(String::new, |case| case.case_id.clone());
    let metrics: Vec<EvidenceMetricDelta> = declared
        .iter()
        .filter_map(|metric| {
            let now = current.and_then(|case| case.metrics.get(&metric.name).copied());
            let then = baseline.and_then(|case| case.metrics.get(&metric.name).copied());
            (now.is_some() || then.is_some()).then(|| EvidenceMetricDelta {
                name: metric.name.clone(),
                unit: Some(metric.unit.clone()),
                direction: Some(metric.direction),
                current: now,
                baseline: then,
                delta: now.zip(then).map(|(now, then)| now - then),
            })
        })
        .collect();

    let change = match (current, baseline) {
        (Some(now), Some(then)) => {
            // A case that stopped being measurable carries no metrics to
            // subtract, so the transition is the whole of its movement.
            let mut better = then.error.is_some() && now.error.is_none();
            let mut worse = now.error.is_some() && then.error.is_none();
            // Over the rows rather than over the declarations: a metric only
            // one side reported has no delta, and the row carries the
            // direction it was built from.
            for metric in &metrics {
                let Some(delta) = metric.delta.filter(|delta| *delta != 0.0) else {
                    continue;
                };
                match metric.direction {
                    Some(MetricDirection::Higher) => {
                        better |= delta > 0.0;
                        worse |= delta < 0.0;
                    }
                    Some(MetricDirection::Lower) => {
                        better |= delta < 0.0;
                        worse |= delta > 0.0;
                    }
                    // A number nobody said which way to read moves without
                    // saying anything about better or worse.
                    Some(MetricDirection::None) | None => {}
                }
            }
            match (better, worse) {
                (true, true) => CaseChange::Mixed,
                (true, false) => CaseChange::Improved,
                (false, true) => CaseChange::Regressed,
                (false, false) => CaseChange::Unchanged,
            }
        }
        _ => CaseChange::Unmeasured,
    };

    EvidenceCaseDelta {
        case_id,
        change,
        current: current.map(outcome),
        baseline: baseline.map(outcome),
        metrics,
    }
}
