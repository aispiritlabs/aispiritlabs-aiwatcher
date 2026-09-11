#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use aiwatcher_evaluation::{Evaluation, EvaluationManifest};

fn fixture() -> EvaluationManifest {
    serde_json::from_str(include_str!(
        "../../../contracts/fixtures/evaluation-v1/manifest.json"
    ))
    .unwrap()
}

#[test]
fn the_published_fixture_keeps_its_versioned_identities() {
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../../contracts/fixtures/evaluation-v1/identities.json"
    ))
    .unwrap();
    let prepared = Evaluation::prepare(fixture()).unwrap();
    assert_eq!(
        prepared.variant_id(),
        expected["variant_id"].as_str().unwrap()
    );
    assert_eq!(
        prepared.context_id(),
        expected["context_id"].as_str().unwrap()
    );
}

#[test]
fn large_metadata_is_refused_even_when_individual_required_fields_are_valid() {
    let mut manifest = fixture();
    manifest.variant.code.schema_ref = Some("x".repeat(aiwatcher_evaluation::MAX_MANIFEST_BYTES));
    assert!(
        Evaluation::prepare(manifest)
            .unwrap_err()
            .to_string()
            .starts_with("manifest:")
    );
}

#[test]
fn judge_configuration_and_calibration_are_part_of_the_measurement_context() {
    let mut manifest = fixture();
    let original = Evaluation::prepare(manifest.clone()).unwrap();
    manifest.context.judge = Some(aiwatcher_evaluation::JudgeConfiguration {
        provider: "fixture".into(),
        model: manifest.context.scorer.clone(),
        configuration: manifest.variant.generation_config.clone(),
        calibration_dataset: manifest.context.dataset.clone(),
    });
    let judged = Evaluation::prepare(manifest.clone()).unwrap();
    assert_eq!(original.variant_id(), judged.variant_id());
    assert_ne!(original.context_id(), judged.context_id());
    manifest
        .context
        .judge
        .as_mut()
        .unwrap()
        .configuration
        .digest
        .clear();
    assert!(
        Evaluation::prepare(manifest)
            .unwrap_err()
            .to_string()
            .starts_with("context.judge.configuration")
    );
}

#[test]
fn retries_keep_variant_and_context_while_independent_measurements_keep_their_origin() {
    let manifest = fixture();
    let first = Evaluation::prepare(manifest.clone()).unwrap();
    let retry = Evaluation::prepare(manifest.clone()).unwrap();
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(&retry).unwrap()
    );
    let mut repeat = manifest;
    repeat.origin.evaluation_id = "eval-independent".into();
    repeat.origin.repetition_id = "measurement-2".into();
    let repeat = Evaluation::prepare(repeat).unwrap();
    assert_eq!(first.variant_id(), repeat.variant_id());
    assert_eq!(first.context_id(), repeat.context_id());
    assert_ne!(first.manifest().origin, repeat.manifest().origin);
}

#[test]
fn changing_generation_or_code_changes_the_variant_but_a_new_scorer_only_changes_context() {
    let first = Evaluation::prepare(fixture()).unwrap();
    for field in ["generation", "code", "dataset", "workflow"] {
        let mut next = fixture();
        match field {
            "generation" => next.variant.generation_config.digest = "a".repeat(64),
            "code" => next.variant.code.digest = "b".repeat(64),
            "dataset" => {
                next.variant.dataset.version = "new-data".into();
                next.context.dataset = next.variant.dataset.clone();
            }
            _ => next.variant.workflow.as_mut().unwrap().version = "new-workflow".into(),
        }
        assert_ne!(
            first.variant_id(),
            Evaluation::prepare(next).unwrap().variant_id(),
            "{field}"
        );
    }
    let mut next = fixture();
    next.context.scorer.version = "new-scorer".into();
    let next = Evaluation::prepare(next).unwrap();
    assert_eq!(first.variant_id(), next.variant_id());
    assert_ne!(first.context_id(), next.context_id());
}

#[test]
fn a_new_cohort_split_or_metric_meaning_cannot_reuse_a_context_fingerprint() {
    let first = Evaluation::prepare(fixture()).unwrap();
    for field in ["cases", "split", "unit", "direction", "aggregation"] {
        let mut next = fixture();
        match field {
            "cases" => next.context.case_manifest.digest = "a".repeat(64),
            "split" => next.context.split = "test".into(),
            "unit" => next.context.metrics[0].unit = "percent".into(),
            "direction" => {
                next.context.metrics[0].direction = aiwatcher_evaluation::MetricDirection::Lower
            }
            _ => next.context.metrics[0].aggregation = aiwatcher_evaluation::Aggregation::Mean,
        }
        let next = Evaluation::prepare(next).unwrap();
        assert_eq!(first.variant_id(), next.variant_id());
        assert_ne!(first.context_id(), next.context_id(), "{field}");
    }
}

#[test]
fn missing_evidence_and_ambiguous_origins_are_refused_with_the_field_named() {
    for (field, expected) in [
        ("version", "schema_version"),
        ("alias", "variant.workflow"),
        ("digest", "variant.code"),
        ("size", "context.case_manifest"),
        ("count", "context.case_count"),
        ("dataset", "context.dataset"),
        ("metric", "context.metrics"),
        ("step", "origin.step_id"),
        ("origin", "origin.repetition_id"),
    ] {
        let mut draft = fixture();
        match field {
            "version" => draft.schema_version = 2,
            "alias" => draft.variant.workflow.as_mut().unwrap().version = "production".into(),
            "digest" => draft.variant.code.digest = "A".repeat(64),
            "size" => draft.context.case_manifest.size_bytes = None,
            "count" => draft.context.case_count = 0,
            "dataset" => draft.context.dataset.kind = aiwatcher_evaluation::DatasetKind::Curation,
            "metric" => draft.context.metrics.push(draft.context.metrics[0].clone()),
            "step" => draft.origin.step_id = Some("score".into()),
            _ => draft.origin.repetition_id = " ".into(),
        }
        let error = Evaluation::prepare(draft).unwrap_err().to_string();
        assert!(error.starts_with(expected), "{field}: {error}");
    }
}

#[test]
fn optional_fields_and_object_order_do_not_change_the_snapshot() {
    let original = fixture();
    let value = serde_json::to_value(&original).unwrap();
    let mut reordered = value.clone();
    reordered["variant"]["prompt"] = serde_json::Value::Null;
    reordered["context"]["judge"] = serde_json::Value::Null;
    let prepared = Evaluation::prepare(serde_json::from_value(reordered).unwrap()).unwrap();
    assert_eq!(
        prepared.variant_id(),
        Evaluation::prepare(original).unwrap().variant_id()
    );
    assert_eq!(serde_json::to_value(prepared.manifest()).unwrap(), value);
}

#[test]
fn unknown_fields_are_not_silently_dropped_from_a_fingerprint() {
    let mut value = serde_json::to_value(fixture()).unwrap();
    value["variant"]["temperature"] = serde_json::json!(0.8);
    assert!(serde_json::from_value::<EvaluationManifest>(value).is_err());
}

#[test]
fn a_prepared_snapshot_does_not_follow_changes_to_the_callers_draft() {
    let mut draft = fixture();
    let pinned = Evaluation::prepare(draft.clone()).unwrap();
    draft.variant.workflow.as_mut().unwrap().version = "changed".into();
    assert_ne!(pinned.manifest().variant.workflow, draft.variant.workflow);
}

#[test]
fn both_historical_storage_imports_are_the_same_neutral_port() {
    fn old(
        value: &dyn aiwatcher_core::prompts::ObjectStore,
    ) -> &dyn aiwatcher_core::storage::ObjectStore {
        value
    }
    fn root(value: &dyn aiwatcher_core::ObjectStore) -> &dyn aiwatcher_core::storage::ObjectStore {
        value
    }
    let _: fn(
        &dyn aiwatcher_core::storage::ObjectStore,
    ) -> &dyn aiwatcher_core::storage::ObjectStore = old;
    let _: fn(
        &dyn aiwatcher_core::storage::ObjectStore,
    ) -> &dyn aiwatcher_core::storage::ObjectStore = root;
}
