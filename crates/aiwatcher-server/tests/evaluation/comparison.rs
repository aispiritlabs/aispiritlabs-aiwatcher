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
