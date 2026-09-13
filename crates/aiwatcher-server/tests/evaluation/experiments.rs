//! An experiment: every variant published in one context, as rows beside a
//! baseline.
use super::*;

fn measured(id: &str, variant: &str, passed: usize, latencies: &[f64]) -> PublishEvaluation {
    let mut request = request(id, 4);
    request.manifest.variant.experiment_id = variant.into();
    for (n, case) in request.cases.iter_mut().enumerate() {
        case.metrics
            .insert("accuracy".into(), f64::from(u8::from(n < passed)));
        case.usage = latencies.get(n).map(|latency| CaseUsage {
            latency_ms: Some(*latency),
            input_tokens: Some(100),
            output_tokens: Some(10 * (n as u64 + 1)),
            models: vec![aiwatcher_core::prices::ModelUsage {
                model: "gpt-4o".into(),
                calls: 1,
                input_tokens: 100,
                output_tokens: 10 * (n as i64 + 1),
                cached_tokens: 0,
            }],
        });
    }
    request
}

#[tokio::test]
async fn an_experiment_is_its_context_s_results_each_against_the_baseline_with_what_they_took() {
    let registry = registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    );
    publish(
        &registry,
        measured("baseline-run", "prompt-v1", 2, &[]),
        "editor",
        1000,
    )
    .await
    .unwrap();
    publish(
        &registry,
        measured("candidate-run", "prompt-v2", 3, &[120.0, 80.0, 400.0]),
        "editor",
        2000,
    )
    .await
    .unwrap();

    let index = registry.experiments("viewer", 3000).await.unwrap();
    assert_eq!(index.experiments.len(), 1, "two variants, one context");
    let entry = &index.experiments[0];
    assert_eq!((entry.results, entry.variants), (2, 2));
    assert_eq!(entry.case_count, Some(4));
    assert!(!index.truncated);

    let experiment = registry
        .experiment(&entry.context_id, Some("baseline-run"), "viewer", 3000)
        .await
        .unwrap()
        .expect("both results are in it");
    assert_eq!(
        experiment
            .rows
            .iter()
            .map(|row| row.evaluation_id.as_str())
            .collect::<Vec<_>>(),
        ["candidate-run", "baseline-run"],
        "newest first"
    );
    let candidate = &experiment.rows[0];
    let compared = candidate.comparison.as_ref().expect("against the baseline");
    assert_eq!(compared.comparability, Comparability::Comparable);
    let accuracy = compared
        .metrics
        .iter()
        .find(|metric| metric.name == "accuracy")
        .unwrap();
    assert_eq!(accuracy.delta, Some(0.25));
    assert!(
        experiment.rows[1].comparison.is_none(),
        "the baseline is not compared with itself"
    );

    let usage = candidate.usage.as_ref().expect("three cases reported");
    let latency = usage.latency_ms.as_ref().unwrap();
    assert_eq!((latency.cases, latency.p50, latency.max), (3, 120.0, 400.0));
    assert_eq!(
        usage.output_tokens.as_ref().map(|t| (t.cases, t.total)),
        Some((3, 60))
    );
    assert!(
        experiment.rows[1].usage.is_none(),
        "a result nobody measured says nothing rather than zero"
    );

    // Priced at a table, from the tokens by model its cases said they called.
    let priced = experiment
        .clone()
        .priced(&aiwatcher_core::prices::ModelPrices {
            currency: "USD".into(),
            prices: vec![aiwatcher_core::prices::ModelPrice {
                model: "gpt-4o".into(),
                input_per_million: 2.5,
                output_per_million: 10.0,
                cached_input_per_million: None,
                source: "https://openai.com/api/pricing".into(),
                as_of: "2026-09-01".into(),
            }],
        });
    let cost = priced.rows[0]
        .cost
        .as_ref()
        .expect("three cases named gpt-4o");
    assert!((cost.amount - (300.0 * 2.5 + 60.0 * 10.0) / 1e6).abs() < 1e-12);
    assert_eq!(
        (cost.priced_calls, cost.prices[0].as_of.as_str()),
        (3, "2026-09-01")
    );
    assert!(priced.rows[1].cost.is_none(), "nothing to price");

    assert!(
        registry
            .experiment(&entry.context_id, Some("not-in-it"), "viewer", 3000)
            .await
            .unwrap()
            .is_none(),
        "a baseline from outside the context is no baseline for it"
    );
}

#[tokio::test]
async fn a_negative_or_endless_latency_is_refused_by_name() {
    let registry = registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    );
    let refused = publish(
        &registry,
        measured("candidate-run", "prompt-v2", 3, &[f64::INFINITY]),
        "editor",
        2000,
    )
    .await
    .unwrap_err();
    assert!(refused.to_string().contains("latency_ms"), "{refused}");
}
