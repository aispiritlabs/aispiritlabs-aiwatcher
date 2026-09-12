//! Two published results, and whether their numbers may be subtracted.
use super::*;

/// One variant of one pinned context, scoring `passed` of its four cases.
///
/// The context is the fixture's, untouched — which is the point: two variants
/// measured the same way share a `context_id` by content, and nothing here has
/// to agree about which five fields to compare.
fn measured(id: &str, variant: &str, passed: usize) -> PublishEvaluation {
    let mut request = request(id, 4);
    request.manifest.variant.experiment_id = variant.into();
    for (n, case) in request.cases.iter_mut().enumerate() {
        case.metrics
            .insert("accuracy".into(), f64::from(u8::from(n < passed)));
    }
    request
}

fn accuracy(comparison: &EvidenceComparison) -> &EvidenceMetricDelta {
    comparison
        .metrics
        .iter()
        .find(|metric| metric.name == "accuracy")
        .expect("the context declares accuracy")
}

async fn two_variants(registry: &Registry) {
    publish(
        registry,
        measured("baseline-run", "prompt-v1", 2),
        "editor",
        1000,
    )
    .await
    .unwrap();
    publish(
        registry,
        measured("candidate-run", "prompt-v2", 3),
        "editor",
        2000,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn two_variants_of_one_pinned_context_are_comparable_and_the_delta_is_theirs() {
    let registry = registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    );
    two_variants(&registry).await;

    let comparison = registry
        .compare("candidate-run", "baseline-run", "viewer", 3000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(comparison.comparability, Comparability::Comparable);
    assert!(comparison.reasons.is_empty(), "{:?}", comparison.reasons);
    assert!(!comparison.same_variant);
    let accuracy = accuracy(&comparison);
    assert_eq!(accuracy.current, Some(0.75));
    assert_eq!(accuracy.baseline, Some(0.5));
    assert_eq!(accuracy.delta, Some(0.25));
    // Which way is better is declared by whoever pinned the context, so the
    // reader is not told a number improved by something that cannot know.
    assert_eq!(accuracy.direction, Some(MetricDirection::Higher));
    assert_eq!(accuracy.unit.as_deref(), Some("ratio"));
}

#[tokio::test]
async fn a_second_measurement_of_one_variant_compares_and_says_it_is_not_a_change() {
    let registry = registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    );
    publish(
        &registry,
        measured("first-go", "prompt-v1", 2),
        "editor",
        1000,
    )
    .await
    .unwrap();
    publish(
        &registry,
        measured("second-go", "prompt-v1", 3),
        "editor",
        2000,
    )
    .await
    .unwrap();

    let comparison = registry
        .compare("second-go", "first-go", "viewer", 3000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(comparison.comparability, Comparability::Comparable);
    assert!(
        comparison.same_variant,
        "one variant measured twice is repetition, not an effect"
    );
    assert_eq!(accuracy(&comparison).delta, Some(0.25));
}

#[tokio::test]
async fn a_different_pinned_context_names_what_moved_and_withholds_the_delta() {
    let registry = registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    );
    two_variants(&registry).await;
    let mut elsewhere = measured("other-split", "prompt-v2", 3);
    elsewhere.manifest.context.split = "holdout".into();
    publish(&registry, elsewhere, "editor", 2100).await.unwrap();

    let comparison = registry
        .compare("other-split", "baseline-run", "viewer", 3000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(comparison.comparability, Comparability::Incompatible);
    assert!(
        comparison
            .reasons
            .iter()
            .any(|reason| reason == "Different split"),
        "{:?}",
        comparison.reasons
    );
    let accuracy = accuracy(&comparison);
    // Both numbers are facts; only their difference is a claim nobody measured.
    assert_eq!(accuracy.current, Some(0.75));
    assert_eq!(accuracy.baseline, Some(0.5));
    assert_eq!(accuracy.delta, None);
}

#[tokio::test]
async fn an_evaluation_is_never_its_own_baseline() {
    let registry = registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    );
    two_variants(&registry).await;
    let comparison = registry
        .compare("candidate-run", "candidate-run", "viewer", 3000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(comparison.comparability, Comparability::Incompatible);
    assert_eq!(accuracy(&comparison).delta, None);
}

#[tokio::test]
async fn evidence_nobody_may_read_any_more_withholds_the_delta_rather_than_showing_it() {
    let registry = registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    );
    two_variants(&registry).await;
    let baseline = registry
        .get("baseline-run", "viewer", 3000)
        .await
        .unwrap()
        .unwrap();
    registry
        .withdraw(
            &approval_id(&baseline.receipt.variant_id, &baseline.receipt.context_id).unwrap(),
            "operator",
            2500,
        )
        .await
        .unwrap()
        .unwrap();

    let comparison = registry
        .compare("candidate-run", "baseline-run", "viewer", 3000)
        .await
        .unwrap()
        .unwrap();
    // Nothing about the two contexts differs — there is no longer evidence
    // behind one of the numbers, which is a different sentence.
    assert_eq!(comparison.comparability, Comparability::Unverified);
    assert_eq!(comparison.baseline.state, EvidenceState::Forbidden);
    let accuracy = accuracy(&comparison);
    assert_eq!(accuracy.current, Some(0.75));
    assert_eq!(accuracy.baseline, None);
    assert_eq!(accuracy.delta, None);
}

#[tokio::test]
async fn the_catalogue_narrowed_to_a_context_offers_what_may_be_compared_and_nothing_else() {
    let registry = registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    );
    two_variants(&registry).await;
    let mut elsewhere = measured("other-split", "prompt-v2", 3);
    elsewhere.manifest.context.split = "holdout".into();
    publish(&registry, elsewhere, "editor", 2100).await.unwrap();

    let context = registry
        .get("candidate-run", "viewer", 3000)
        .await
        .unwrap()
        .unwrap()
        .receipt
        .context_id;
    let page = registry
        .list(None, 200, None, Some(&context), "viewer", 3000)
        .await
        .unwrap();
    let ids: Vec<&str> = page
        .evaluations
        .iter()
        .map(|evidence| evidence.receipt.evaluation_id.as_str())
        .collect();
    assert_eq!(ids, ["candidate-run", "baseline-run"]);
    assert!(page.next_cursor.is_none());

    // The filter narrows a published order rather than replacing it.
    let head = registry
        .list(None, 1, None, Some(&context), "viewer", 3000)
        .await
        .unwrap();
    assert_eq!(
        head.evaluations[0].receipt.evaluation_id, "candidate-run",
        "newest first, as the unfiltered catalogue is"
    );
    let rest = registry
        .list(
            head.next_cursor.as_deref(),
            1,
            None,
            Some(&context),
            "viewer",
            3000,
        )
        .await
        .unwrap();
    assert_eq!(rest.evaluations[0].receipt.evaluation_id, "baseline-run");
}

// ── Which cases moved ────────────────────────────────────────────────────────

/// One variant of one pinned context, with each case's outcome spelled out.
///
/// `None` is a case that was attempted and failed, which carries no metrics —
/// the one movement a diff reads without subtracting anything.
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
                case.error = Some("the model timed out".into());
            }
        }
    }
    request
}

async fn published(registry: &Registry, current: PublishEvaluation, baseline: PublishEvaluation) {
    publish(registry, baseline, "editor", 1000).await.unwrap();
    publish(registry, current, "editor", 2000).await.unwrap();
}

async fn diff(registry: &Registry, only: Option<CaseFilter>) -> CaseDiffPage {
    registry
        .compare_cases(
            "candidate-run",
            "baseline-run",
            DiffQuery {
                only,
                ..Default::default()
            },
            "viewer",
            3000,
        )
        .await
        .unwrap()
        .unwrap()
}

fn case<'a>(page: &'a CaseDiffPage, id: &str) -> &'a EvidenceCaseDelta {
    page.cases
        .iter()
        .find(|delta| delta.case_id == id)
        .unwrap_or_else(|| panic!("{id} is on the page: {:?}", page.cases))
}

#[tokio::test]
async fn a_case_that_stopped_being_measurable_is_a_regression_with_no_number_behind_it() {
    let registry = registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    );
    published(
        &registry,
        outcomes("candidate-run", "prompt-v2", &[Some(1.0), None, Some(1.0)]),
        outcomes(
            "baseline-run",
            "prompt-v1",
            &[Some(1.0), Some(1.0), Some(1.0)],
        ),
    )
    .await;

    let page = diff(&registry, None).await;
    // A result holding a case that failed is `Partial`, so the header withheld
    // its delta — and the rows are still here, because that failed case is the
    // whole of what somebody opened this to read.
    assert_eq!(page.comparability, Comparability::Unverified);
    assert!(
        page.reasons
            .iter()
            .any(|reason| reason.contains("unscored or failed")),
        "{:?}",
        page.reasons,
    );
    assert_eq!(page.cases.len(), 3);
    let broken = case(&page, "case-00001");
    assert_eq!(broken.change, CaseChange::Regressed);
    assert_eq!(
        broken.current.as_ref().unwrap().error.as_deref(),
        Some("the model timed out"),
    );
    // The metric is still on the row with one side missing: a case that failed
    // reports no score, and a delta of zero would say it scored the same.
    let accuracy = &broken.metrics[0];
    assert_eq!(accuracy.current, None);
    assert_eq!(accuracy.baseline, Some(1.0));
    assert_eq!(accuracy.delta, None);
    assert_eq!(case(&page, "case-00000").change, CaseChange::Unchanged);
}

#[tokio::test]
async fn a_case_that_became_measurable_again_is_an_improvement() {
    let registry = registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    );
    published(
        &registry,
        outcomes(
            "candidate-run",
            "prompt-v2",
            &[Some(1.0), Some(1.0), Some(1.0)],
        ),
        outcomes("baseline-run", "prompt-v1", &[Some(1.0), None, Some(1.0)]),
    )
    .await;

    let page = diff(&registry, None).await;
    assert_eq!(case(&page, "case-00001").change, CaseChange::Improved);
    assert!(
        case(&page, "case-00001").baseline.as_ref().unwrap().error
            == Some("the model timed out".into()),
    );
}

/// A second metric the context declares as better when it falls.
fn with_latency(request: &mut PublishEvaluation, values: &[f64]) {
    request.manifest.context.metrics.push(MetricDefinition {
        name: "latency_ms".into(),
        unit: "ms".into(),
        direction: MetricDirection::Lower,
        aggregation: Aggregation::Mean,
    });
    for (case, value) in request.cases.iter_mut().zip(values) {
        if case.error.is_none() {
            case.metrics.insert("latency_ms".into(), *value);
        }
    }
}

#[tokio::test]
async fn a_case_better_at_one_thing_and_worse_at_another_is_neither_a_regression_nor_a_fix() {
    let registry = registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    );
    let mut current = outcomes(
        "candidate-run",
        "prompt-v2",
        &[Some(1.0), Some(1.0), Some(1.0)],
    );
    let mut baseline = outcomes(
        "baseline-run",
        "prompt-v1",
        &[Some(0.0), Some(1.0), Some(1.0)],
    );
    with_latency(&mut current, &[200.0, 50.0, 100.0]);
    with_latency(&mut baseline, &[100.0, 100.0, 100.0]);
    published(&registry, current, baseline).await;

    let page = diff(&registry, None).await;
    // More accurate and slower: the state a single verdict would have to hide.
    assert_eq!(case(&page, "case-00000").change, CaseChange::Mixed);
    // Faster, and the context is what says that is the better direction.
    assert_eq!(case(&page, "case-00001").change, CaseChange::Improved);
    assert_eq!(case(&page, "case-00002").change, CaseChange::Unchanged);

    let worse = diff(&registry, Some(CaseFilter::Worse)).await;
    let kept: Vec<&str> = worse
        .cases
        .iter()
        .map(|delta| delta.case_id.as_str())
        .collect();
    assert_eq!(
        kept,
        ["case-00000"],
        "a release gate reads the cases that lost something, mixed ones included",
    );
    let better = diff(&registry, Some(CaseFilter::Better)).await;
    assert_eq!(better.cases.len(), 2, "{:?}", better.cases);
}

#[tokio::test]
async fn a_case_only_one_side_measured_is_an_absence_rather_than_a_movement() {
    let registry = registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    );
    let mut current = outcomes(
        "candidate-run",
        "prompt-v2",
        &[Some(1.0), Some(1.0), Some(1.0)],
    );
    // The cohort is pinned and both sides declare all three; this run only
    // reported two of them, which is the `unscored` its header already counts.
    current.cases.pop();
    current.status = ResultStatus::Partial;
    published(
        &registry,
        current,
        outcomes(
            "baseline-run",
            "prompt-v1",
            &[Some(1.0), Some(1.0), Some(1.0)],
        ),
    )
    .await;

    let page = diff(&registry, None).await;
    let missing = case(&page, "case-00002");
    assert_eq!(missing.change, CaseChange::Unmeasured);
    assert!(missing.current.is_none());
    assert!(missing.baseline.is_some());
    let changed = diff(&registry, Some(CaseFilter::Changed)).await;
    assert_eq!(
        changed
            .cases
            .iter()
            .map(|delta| delta.case_id.as_str())
            .collect::<Vec<_>>(),
        ["case-00002"],
    );
    assert!(
        diff(&registry, Some(CaseFilter::Worse))
            .await
            .cases
            .is_empty(),
        "an absence is a difference between two runs, not a case that got worse",
    );
}

#[tokio::test]
async fn two_results_the_server_will_not_subtract_have_no_case_rows_and_say_why() {
    let registry = registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    );
    two_variants(&registry).await;
    let mut elsewhere = measured("other-split", "prompt-v2", 3);
    elsewhere.manifest.context.split = "holdout".into();
    publish(&registry, elsewhere, "editor", 2100).await.unwrap();

    let page = registry
        .compare_cases(
            "other-split",
            "baseline-run",
            DiffQuery::default(),
            "viewer",
            3000,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(page.comparability, Comparability::Incompatible);
    assert!(page.cases.is_empty());
    assert!(page.next_cursor.is_none());
    assert!(
        page.reasons
            .iter()
            .any(|reason| reason == "Different split"),
        "{:?}",
        page.reasons,
    );
}

#[tokio::test]
async fn a_diff_over_more_cases_than_one_shard_holds_walks_both_sides_in_lockstep() {
    let registry = registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    );
    let scores: Vec<Option<f64>> = (0..250)
        .map(|n| Some(f64::from(u8::from(n != 7))))
        .collect();
    published(
        &registry,
        outcomes("candidate-run", "prompt-v2", &scores),
        outcomes("baseline-run", "prompt-v1", &vec![Some(1.0); scores.len()]),
    )
    .await;

    let mut cursor = None;
    let mut seen: Vec<String> = Vec::new();
    loop {
        let page = registry
            .compare_cases(
                "candidate-run",
                "baseline-run",
                DiffQuery {
                    cursor: cursor.as_deref(),
                    limit: Some(100),
                    only: None,
                },
                "viewer",
                3000,
            )
            .await
            .unwrap()
            .unwrap();
        seen.extend(page.cases.iter().map(|delta| delta.case_id.clone()));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen.len(), 250);
    let mut sorted = seen.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted, seen,
        "one row per case, in the order both sides hold"
    );

    // The one case that moved, found without the reader paging past the rest.
    let regressed = registry
        .compare_cases(
            "candidate-run",
            "baseline-run",
            DiffQuery {
                limit: Some(100),
                only: Some(CaseFilter::Worse),
                ..Default::default()
            },
            "viewer",
            3000,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        regressed
            .cases
            .iter()
            .map(|delta| delta.case_id.as_str())
            .collect::<Vec<_>>(),
        ["case-00007"],
    );
}

#[tokio::test]
async fn a_cursor_from_another_pair_of_results_is_refused_rather_than_resolved() {
    let registry = registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    );
    two_variants(&registry).await;
    let refused = registry
        .compare_cases(
            "candidate-run",
            "baseline-run",
            DiffQuery {
                cursor: Some("deadbeef:0:deadbeef:0"),
                ..Default::default()
            },
            "viewer",
            3000,
        )
        .await;
    assert!(
        matches!(refused, Err(EvaluationError::Invalid { ref field, .. }) if field == "cursor"),
        "{refused:?}",
    );
}

#[tokio::test]
async fn a_diff_row_says_where_its_case_is_so_the_answers_come_from_the_route_that_has_them() {
    let registry = registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    );
    published(
        &registry,
        outcomes(
            "candidate-run",
            "prompt-v2",
            &[Some(1.0), Some(0.0), Some(1.0)],
        ),
        outcomes(
            "baseline-run",
            "prompt-v1",
            &[Some(1.0), Some(1.0), Some(1.0)],
        ),
    )
    .await;

    let page = diff(&registry, Some(CaseFilter::Worse)).await;
    let row = &page.cases[0];
    assert_eq!(row.case_id, "case-00001");
    // The diff row carries no answer; it carries where the answer is, in the
    // words the case route already speaks.
    for (id, side) in [
        ("candidate-run", row.current.as_ref().unwrap()),
        ("baseline-run", row.baseline.as_ref().unwrap()),
    ] {
        let receipt = registry.get(id, "viewer", 3000).await.unwrap().unwrap();
        let one = registry
            .cases(
                id,
                &receipt.receipt.version,
                Some(&side.at),
                Some(1),
                "viewer",
                3000,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(one.cases.len(), 1);
        assert_eq!(
            one.cases[0].measurement.case_id, row.case_id,
            "{id}'s cursor points at the case the row is about",
        );
        assert!(one.cases[0].expected.is_object(), "with what was expected");
    }
}
