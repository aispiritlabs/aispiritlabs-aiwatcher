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

use crate::{DurableEvaluation, EvaluationContext, EvidenceState, MetricDirection, ResultStatus};

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
fn comparability(
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
