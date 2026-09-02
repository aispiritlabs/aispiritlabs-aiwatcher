//! What the training registry defends, over an in-memory store.
//!
//! Named after the mistakes rather than the methods, because every one of them
//! produces a working pipeline and a number nobody should act on: a curve with
//! two points at the same epoch, a model promoted on the score its own early
//! stopping maximised, a run that claims a dataset nothing can reconstruct.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Arc;

use aiwatcher_prompts::adapters::memory::MemoryObjectStore;
use aiwatcher_training::{
    CheckpointInput, EpochInput, Error, FinishRunRequest, ModelLabelRequest, ModelMetrics,
    ProfileInput, ProgressRequest, RegisterModelRequest, Registry, RunFilter, SampleInput,
    StartRunRequest, TrainingStatus,
};
use serde_json::json;

const EXPORT: &str = "floor-plans/dom-projekt@9f3c2b1a";
const MUTABLE: &str = "floor-plans/dom-projekt";

fn registry() -> Registry {
    Registry::new(Arc::new(MemoryObjectStore::new()), "training")
}

fn start(run_id: &str, dataset: &str) -> StartRunRequest {
    StartRunRequest {
        run_id: run_id.to_owned(),
        model: "floor-plan-segmenter".to_owned(),
        dataset: dataset.to_owned(),
        schema_version: Some("ab".repeat(32)),
        framework: "pytorch".to_owned(),
        device: "cuda:0".to_owned(),
        code: "git:9f3c2b1".to_owned(),
        params: BTreeMap::from([("batch_size".to_owned(), json!(4))]),
        workflow_run_id: None,
    }
}

fn epoch(index: u32, loss: f64) -> ProgressRequest {
    ProgressRequest {
        epochs: vec![EpochInput {
            epoch: index,
            duration_ms: 1_000.0,
            steps: 25,
            metrics: BTreeMap::from([("loss".to_owned(), loss)]),
        }],
        ..ProgressRequest::default()
    }
}

async fn finished(registry: &Registry, run_id: &str, dataset: &str) {
    registry.start(start(run_id, dataset)).await.unwrap();
    registry
        .progress(
            run_id,
            ProgressRequest {
                checkpoints: vec![CheckpointInput {
                    uri: format!("s3://models/{run_id}.pt"),
                    epoch: Some(7),
                    step: None,
                    metric: Some("val_miou".to_owned()),
                    value: Some(0.73),
                    best: true,
                }],
                ..epoch(0, 1.2)
            },
        )
        .await
        .unwrap();
    registry
        .finish(
            run_id,
            FinishRunRequest {
                status: TrainingStatus::Succeeded,
                error: None,
                best: None,
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn a_retried_epoch_lands_on_the_epoch_it_already_wrote() {
    // The failure this prevents is a curve with two points at the same x, which
    // reads as a training run that went backwards.
    let registry = registry();
    registry.start(start("run-1", EXPORT)).await.unwrap();
    registry.progress("run-1", epoch(0, 1.2)).await.unwrap();
    registry.progress("run-1", epoch(1, 0.9)).await.unwrap();
    let run = registry.progress("run-1", epoch(1, 0.8)).await.unwrap();

    assert_eq!(run.epochs.len(), 2);
    assert_eq!(run.series("loss"), vec![(0, 1.2), (1, 0.8)]);
}

#[tokio::test]
async fn starting_a_run_twice_returns_the_open_one_and_never_reopens_a_closed_one() {
    let registry = registry();
    registry.start(start("run-2", EXPORT)).await.unwrap();
    registry.progress("run-2", epoch(0, 1.0)).await.unwrap();

    // A trainer retrying its own start after a timeout must not lose the
    // epochs it already wrote.
    let again = registry.start(start("run-2", EXPORT)).await.unwrap();
    assert_eq!(again.epochs.len(), 1);

    registry
        .finish(
            "run-2",
            FinishRunRequest {
                status: TrainingStatus::Succeeded,
                error: None,
                best: None,
            },
        )
        .await
        .unwrap();
    // Reusing the id would give the second run the first one's curve.
    let error = registry.start(start("run-2", EXPORT)).await.unwrap_err();
    assert!(error.to_string().contains("already finished"), "{error}");
}

#[tokio::test]
async fn a_closed_run_will_not_take_more_of_a_curve() {
    let registry = registry();
    finished(&registry, "run-3", EXPORT).await;
    let error = registry.progress("run-3", epoch(9, 0.1)).await.unwrap_err();
    assert!(error.to_string().contains("curve is closed"), "{error}");
}

#[tokio::test]
async fn a_diverged_loss_does_not_lose_the_epochs_that_show_the_divergence() {
    // `NaN` is what a diverging run produces, and it is also what `serde_json`
    // refuses to serialise. Failing the write here would discard exactly the
    // epochs somebody needs to see.
    let registry = registry();
    registry.start(start("run-4", EXPORT)).await.unwrap();
    let run = registry
        .progress("run-4", epoch(0, f64::NAN))
        .await
        .unwrap();
    assert_eq!(run.epochs[0].metrics.get("loss"), Some(&0.0));

    let reread = registry.run("run-4").await.unwrap();
    assert_eq!(reread.epochs.len(), 1);
}

#[tokio::test]
async fn the_sampled_series_is_halved_rather_than_truncated_and_says_so() {
    // A truncated head loses the warm-up and a truncated tail loses the
    // divergence. Half the resolution loses neither.
    let registry = registry();
    registry.start(start("run-5", EXPORT)).await.unwrap();
    let samples: Vec<SampleInput> = (0..3_000)
        .map(|index| SampleInput {
            step: Some(index),
            metrics: BTreeMap::from([("lr".to_owned(), f64::from(index as u32))]),
        })
        .collect();
    let run = registry
        .progress(
            "run-5",
            ProgressRequest {
                samples,
                ..ProgressRequest::default()
            },
        )
        .await
        .unwrap();

    assert!(run.samples.len() <= aiwatcher_training::MAX_SAMPLES);
    assert_eq!(run.sample_decimations, 1);
    // The shape survives: first and last are both still there.
    assert_eq!(run.samples.first().and_then(|s| s.step), Some(0));
    assert_eq!(run.samples.last().and_then(|s| s.step), Some(2_998));
}

#[tokio::test]
async fn a_run_with_no_end_stays_running_and_reports_when_it_was_last_heard_from() {
    // The same rule the projector follows for agent runs: nothing here decides
    // a trainer died. A killed process and a thinking one are indistinguishable
    // from this side, so the honest field is the timestamp.
    let registry = registry();
    registry.start(start("run-6", EXPORT)).await.unwrap();
    let before = registry.run("run-6").await.unwrap().last_heard_from;
    let run = registry.progress("run-6", epoch(0, 1.0)).await.unwrap();

    assert_eq!(run.status, TrainingStatus::Running);
    assert!(run.ended_at.is_none());
    assert!(run.last_heard_from >= before);
}

#[tokio::test]
async fn a_model_version_takes_its_provenance_from_the_run_rather_than_the_request() {
    let registry = registry();
    finished(&registry, "run-7", EXPORT).await;
    let registered = registry
        .register_model(RegisterModelRequest {
            name: "floor-plan.segmenter".to_owned(),
            run_id: "run-7".to_owned(),
            checkpoint_uri: "s3://models/run-7.pt".to_owned(),
            description: "The house-plan segmenter".to_owned(),
            metrics: ModelMetrics {
                validation: BTreeMap::from([("miou".to_owned(), 0.81)]),
                test: BTreeMap::from([("miou".to_owned(), 0.74)]),
            },
            notes: String::new(),
        })
        .await
        .unwrap();

    assert!(registered.created);
    assert_eq!(registered.version.dataset, EXPORT);
    assert_eq!(registered.version.framework, "pytorch");
    assert_eq!(registered.version.code, "git:9f3c2b1");
    assert!(registered.version.reproducible);
    assert!(registered.promotion_blocked.is_none());
    // The gap between what selection watched and what nothing watched is the
    // number worth following across a series.
    assert_eq!(
        registered.version.metrics.overfit_gap("miou"),
        Some(0.81 - 0.74)
    );
}

#[tokio::test]
async fn registering_the_same_thing_twice_is_one_version() {
    let registry = registry();
    finished(&registry, "run-8", EXPORT).await;
    let request = || RegisterModelRequest {
        name: "floor-plan.segmenter".to_owned(),
        run_id: "run-8".to_owned(),
        checkpoint_uri: "s3://models/run-8.pt".to_owned(),
        description: String::new(),
        metrics: ModelMetrics {
            validation: BTreeMap::from([("miou".to_owned(), 0.81)]),
            test: BTreeMap::from([("miou".to_owned(), 0.74)]),
        },
        notes: String::new(),
    };
    let first = registry.register_model(request()).await.unwrap();
    let second = registry.register_model(request()).await.unwrap();

    assert!(first.created);
    assert!(!second.created);
    assert_eq!(first.version.version, second.version.version);
    assert_eq!(second.head.versions.len(), 1);
}

#[tokio::test]
async fn a_model_with_no_held_out_score_is_recorded_and_refused_a_label() {
    // The prompt registry's rule, for weights. A validation score is the number
    // early stopping maximised; promoting on it promotes the selection.
    let registry = registry();
    finished(&registry, "run-9", EXPORT).await;
    let registered = registry
        .register_model(RegisterModelRequest {
            name: "floor-plan.segmenter".to_owned(),
            run_id: "run-9".to_owned(),
            checkpoint_uri: "s3://models/run-9.pt".to_owned(),
            description: String::new(),
            metrics: ModelMetrics {
                validation: BTreeMap::from([("miou".to_owned(), 0.91)]),
                test: BTreeMap::new(),
            },
            notes: String::new(),
        })
        .await
        .unwrap();

    // Recorded, and the reason arrives with the registration rather than three
    // days later when somebody tries to ship it.
    assert!(registered.created);
    let blocked = registered.promotion_blocked.expect("a reason");
    assert!(blocked.contains("held-out"), "{blocked}");

    let error = registry
        .set_label(
            "floor-plan.segmenter",
            ModelLabelRequest {
                label: "production".to_owned(),
                version: registered.version.version.clone(),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Refused(_)), "{error}");
}

#[tokio::test]
async fn a_model_trained_on_a_mutable_dataset_name_is_refused_a_label() {
    let registry = registry();
    finished(&registry, "run-10", MUTABLE).await;
    let registered = registry
        .register_model(RegisterModelRequest {
            name: "floor-plan.segmenter".to_owned(),
            run_id: "run-10".to_owned(),
            checkpoint_uri: "s3://models/run-10.pt".to_owned(),
            description: String::new(),
            metrics: ModelMetrics {
                validation: BTreeMap::new(),
                test: BTreeMap::from([("miou".to_owned(), 0.74)]),
            },
            notes: String::new(),
        })
        .await
        .unwrap();

    assert!(!registered.version.reproducible);
    let blocked = registered.promotion_blocked.expect("a reason");
    assert!(blocked.contains("immutable export"), "{blocked}");
}

#[tokio::test]
async fn a_promotable_version_takes_the_label_and_the_head_answers_with_it() {
    let registry = registry();
    finished(&registry, "run-11", EXPORT).await;
    let registered = registry
        .register_model(RegisterModelRequest {
            name: "floor-plan.segmenter".to_owned(),
            run_id: "run-11".to_owned(),
            checkpoint_uri: "s3://models/run-11.pt".to_owned(),
            description: String::new(),
            metrics: ModelMetrics {
                validation: BTreeMap::from([("miou".to_owned(), 0.81)]),
                test: BTreeMap::from([("miou".to_owned(), 0.74)]),
            },
            notes: String::new(),
        })
        .await
        .unwrap();

    let head = registry
        .set_label(
            "floor-plan.segmenter",
            ModelLabelRequest {
                label: "production".to_owned(),
                version: registered.version.version.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        head.labelled("production")
            .map(|entry| entry.version.clone()),
        Some(registered.version.version.clone())
    );

    // Reading the model with no version asked for resolves `production`.
    let detail = registry.model("floor-plan.segmenter", None).await.unwrap();
    assert_eq!(
        detail.current.map(|version| version.version),
        Some(registered.version.version)
    );
}

#[tokio::test]
async fn runs_list_newest_first_and_filter_by_the_export_or_by_the_project() {
    let registry = registry();
    finished(&registry, "run-a", EXPORT).await;
    finished(&registry, "run-b", "floor-plans/dom-projekt@0011aabb").await;
    finished(&registry, "run-c", "other/project@ffee").await;

    let all = registry.runs(&RunFilter::default(), 50).await.unwrap();
    assert_eq!(all.total, 3);

    // "this exact cut" and "every cut of this project" are different questions.
    let exact = registry
        .runs(
            &RunFilter {
                dataset: Some(EXPORT.to_owned()),
                ..RunFilter::default()
            },
            50,
        )
        .await
        .unwrap();
    assert_eq!(exact.total, 1);

    let project = registry
        .runs(
            &RunFilter {
                dataset: Some("floor-plans/dom-projekt".to_owned()),
                ..RunFilter::default()
            },
            50,
        )
        .await
        .unwrap();
    assert_eq!(project.total, 2);
}

#[tokio::test]
async fn a_failed_run_keeps_the_curve_it_had_reached() {
    // A run that died in epoch forty is a different finding from one that died
    // in epoch one, and the difference is only visible if the epochs survive.
    let registry = registry();
    registry.start(start("run-12", EXPORT)).await.unwrap();
    for index in 0..3 {
        registry
            .progress("run-12", epoch(index, 1.0 - f64::from(index) * 0.1))
            .await
            .unwrap();
    }
    let run = registry
        .finish(
            "run-12",
            FinishRunRequest {
                status: TrainingStatus::Failed,
                error: Some("CUDA out of memory".to_owned()),
                best: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(run.status, TrainingStatus::Failed);
    assert_eq!(run.epochs.len(), 3);
    assert!(run.error.as_deref().unwrap().contains("CUDA"));
    assert!(run.duration_ms().is_some());
}

#[tokio::test]
async fn a_profile_is_a_summary_and_a_link_rather_than_a_trace() {
    let registry = registry();
    registry.start(start("run-13", EXPORT)).await.unwrap();
    let run = registry
        .progress(
            "run-13",
            ProgressRequest {
                profiles: vec![ProfileInput {
                    summary: json!({
                        "top_share": 0.41,
                        "operators": [{"name": "aten::conv2d", "self_cpu_us": 410_000.0}],
                    }),
                    uri: Some("s3://profiles/run-13.trace.json".to_owned()),
                }],
                ..ProgressRequest::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(run.profiles.len(), 1);
    assert_eq!(run.profiles[0].summary["top_share"], json!(0.41));
    assert_eq!(
        run.profiles[0].uri.as_deref(),
        Some("s3://profiles/run-13.trace.json")
    );
}

#[tokio::test]
async fn a_run_id_that_could_walk_out_of_its_prefix_is_refused() {
    let registry = registry();
    let error = registry.run("../../../etc/passwd").await.unwrap_err();
    assert!(error.to_string().contains("must start with"), "{error}");
}

#[tokio::test]
async fn a_checkpoint_marked_best_becomes_the_run_s_headline_number() {
    let registry = registry();
    registry.start(start("run-14", EXPORT)).await.unwrap();
    let run = registry
        .progress(
            "run-14",
            ProgressRequest {
                checkpoints: vec![CheckpointInput {
                    uri: "s3://models/run-14-e12.pt".to_owned(),
                    epoch: Some(12),
                    step: None,
                    metric: Some("val_miou".to_owned()),
                    value: Some(0.812),
                    best: true,
                }],
                ..ProgressRequest::default()
            },
        )
        .await
        .unwrap();

    let best = run.best.clone().expect("a best metric");
    assert_eq!(best.metric, "val_miou");
    assert_eq!(best.value, 0.812);
    assert_eq!(best.epoch, Some(12));
    assert_eq!(run.summary().best.map(|entry| entry.value), Some(0.812));
}
