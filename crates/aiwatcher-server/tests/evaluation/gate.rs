//! A regression gate: a verdict a pipeline exits by, decided from the comparison.
use super::*;

/// `None` is a case attempted and failed.
fn outcomes(id: &str, variant: &str, cases: &[Option<f64>]) -> PublishEvaluation {
    let mut request = request(id, cases.len() as u64);
    request.manifest.variant.experiment_id = variant.into();
    request.status = if cases.iter().any(Option::is_none) {
        ResultStatus::Partial
    } else {
        ResultStatus::Succeeded
    };
    for (case, outcome) in request.cases.iter_mut().zip(cases) {
        match outcome {
            Some(accuracy) => {
                case.metrics.insert("accuracy".into(), *accuracy);
            }
            None => {
                case.metrics.clear();
                case.actual = None;
                case.error = Some("the scorer failed".into());
            }
        }
    }
    request
}

async fn gated(
    baseline: &[Option<f64>],
    candidate: &[Option<f64>],
    policy: GatePolicy,
) -> GateDecision {
    let registry = registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    );
    publish(
        &registry,
        outcomes("baseline-run", "v1", baseline),
        "ci",
        1000,
    )
    .await
    .unwrap();
    publish(
        &registry,
        outcomes("candidate-run", "v2", candidate),
        "ci",
        2000,
    )
    .await
    .unwrap();
    registry
        .gate("candidate-run", "baseline-run", &policy, "ci", 3000)
        .await
        .unwrap()
        .expect("both results are there")
}

#[tokio::test]
async fn a_better_candidate_passes_and_says_what_it_was_held_to() {
    let decision = gated(
        &[Some(1.0), Some(0.0), Some(0.0), Some(1.0)],
        &[Some(1.0), Some(1.0), Some(0.0), Some(1.0)],
        GatePolicy::default(),
    )
    .await;
    assert_eq!(
        decision.verdict,
        GateVerdict::Pass,
        "{:?}",
        decision.reasons
    );
    assert!(decision.reasons.is_empty());
    assert_eq!(decision.candidate.evaluation_id, "candidate-run");
    assert!(decision.candidate.suite.is_some() && decision.candidate.code.is_some());
}

#[tokio::test]
async fn a_critical_case_lost_is_a_regression_even_beside_a_better_average() {
    // The mean rises from a half to three quarters, and case-00000 — the one
    // the policy says must hold — went from right to wrong.
    let decision = gated(
        &[Some(1.0), Some(0.0), Some(0.0), Some(1.0)],
        &[Some(0.0), Some(1.0), Some(1.0), Some(1.0)],
        GatePolicy {
            critical_cases: vec!["case-00000".into(), "case-00003".into()],
            ..GatePolicy::default()
        },
    )
    .await;
    assert_eq!(decision.verdict, GateVerdict::Regression);
    let accuracy = decision
        .metrics
        .iter()
        .find(|metric| metric.delta.name == "accuracy")
        .unwrap();
    assert_eq!(accuracy.delta.delta, Some(0.25));
    assert!(!accuracy.regressed);
    assert_eq!(
        decision
            .critical
            .iter()
            .map(|case| (case.case_id.as_str(), case.held))
            .collect::<Vec<_>>(),
        [("case-00000", false), ("case-00003", true)]
    );
    assert!(
        decision
            .reasons
            .iter()
            .any(|reason| reason.starts_with("critical case case-00000")),
        "{:?}",
        decision.reasons
    );
}

#[tokio::test]
async fn a_metric_worse_by_more_than_its_tolerance_is_a_regression_and_within_it_is_not() {
    let baseline = [Some(1.0), Some(1.0), Some(1.0), Some(1.0)];
    let candidate = [Some(1.0), Some(1.0), Some(1.0), Some(0.0)];
    let strict = gated(&baseline, &candidate, GatePolicy::default()).await;
    assert_eq!(strict.verdict, GateVerdict::Regression);
    assert!(
        strict.reasons[0].contains("may not get worse"),
        "{:?}",
        strict.reasons
    );

    let tolerant = gated(
        &baseline,
        &candidate,
        GatePolicy {
            tolerance: BTreeMap::from([("accuracy".into(), 0.3)]),
            ..GatePolicy::default()
        },
    )
    .await;
    assert_eq!(
        tolerant.verdict,
        GateVerdict::Pass,
        "{:?}",
        tolerant.reasons
    );

    let ignored = gated(
        &baseline,
        &candidate,
        GatePolicy {
            ignore: vec!["accuracy".into()],
            ..GatePolicy::default()
        },
    )
    .await;
    assert_eq!(ignored.verdict, GateVerdict::Pass);
}

#[tokio::test]
async fn a_scorer_that_failed_never_passes() {
    let decision = gated(
        &[Some(1.0), Some(1.0), Some(1.0), Some(1.0)],
        &[Some(1.0), Some(1.0), Some(1.0), None],
        GatePolicy::default(),
    )
    .await;
    // The header withholds its delta over a failed case, so no metric can be
    // held — and the measurement that could not say is not a pass.
    assert_eq!(
        decision.verdict,
        GateVerdict::Incomplete,
        "{:?}",
        decision.reasons
    );
    assert!(decision.reasons[0].contains("1 of 4 cases failed"));
}

#[tokio::test]
async fn a_pair_that_does_not_compare_is_an_error_rather_than_a_verdict() {
    let registry = registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    );
    publish(
        &registry,
        outcomes("baseline-run", "v1", &[Some(1.0), Some(1.0)]),
        "ci",
        1000,
    )
    .await
    .unwrap();
    let mut other = outcomes("candidate-run", "v2", &[Some(1.0), Some(1.0), Some(1.0)]);
    other.manifest.context.split = "validation".into();
    publish(&registry, other, "ci", 2000).await.unwrap();
    let decision = registry
        .gate(
            "candidate-run",
            "baseline-run",
            &GatePolicy {
                critical_cases: vec!["case-00000".into()],
                ..GatePolicy::default()
            },
            "ci",
            3000,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(decision.verdict, GateVerdict::Error);
    assert_eq!(decision.comparability, Comparability::Incompatible);
}

#[tokio::test]
async fn a_line_is_refused_for_evidence_a_producer_measured() {
    let registry = registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    );
    let refused = registry
        .admit_line(&request("any", 2).manifest, "operator", 1000)
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("pair by pair"), "{refused}");
}

#[tokio::test]
async fn a_policy_requiring_traces_holds_a_result_no_trace_was_read_for_incomplete() {
    let decision = gated(
        &[Some(1.0), Some(0.0)],
        &[Some(1.0), Some(1.0)],
        GatePolicy {
            require_traces: true,
            ..GatePolicy::default()
        },
    )
    .await;
    assert_eq!(decision.verdict, GateVerdict::Incomplete);
    assert!(
        decision
            .reasons
            .iter()
            .any(|reason| reason.contains("no trace was read")),
        "{:?}",
        decision.reasons
    );
}
