//! A durable project boundary, including the legacy route's storage namespace.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
use aiwatcher_iam::{OrganizationId, ProjectId, ProjectScope};
use aiwatcher_prompts::adapters::fs::FileObjectStore;
use aiwatcher_training::{
    ModelLabelRequest, ProgressRequest, RegisterModelRequest, Registry, RunFilter, StartRunRequest,
};
use serde_json::{json, to_value};
use std::sync::Arc;

fn start(dataset: &str) -> StartRunRequest {
    serde_json::from_value(json!({"run_id":"same-run", "model":"same-model", "dataset":dataset, "workflow_run_id":"original-workflow", "params":{"seed":42}})).unwrap()
}
fn model() -> RegisterModelRequest {
    serde_json::from_value(json!({"name":"same-model", "run_id":"same-run", "checkpoint_uri":"s3://original/weights", "metrics":{"test":{"accuracy":0.8}}, "notes":"original notes"})).unwrap()
}
fn scope() -> ProjectScope {
    ProjectScope {
        organization: OrganizationId::new(),
        project: ProjectId::new(),
    }
}

#[tokio::test]
async fn reopened_projects_keep_training_curves_model_hashes_and_labels_separate() {
    let a_scope = scope();
    let dir = std::env::temp_dir().join(format!("aiwatcher-training-scope-{}", a_scope.project.0));
    let store = Arc::new(FileObjectStore::open(&dir).await.unwrap());
    let legacy = Registry::new(store, "training");
    let a = legacy.for_project(a_scope).unwrap();
    let b_scope = ProjectScope {
        project: ProjectId::new(),
        ..a_scope
    };
    let b = legacy.for_project(b_scope).unwrap();
    let other = legacy
        .for_project(ProjectScope {
            organization: OrganizationId::new(),
            ..a_scope
        })
        .unwrap();
    let run = a.start(start("data@abcd")).await.unwrap();
    a.progress("same-run",serde_json::from_value::<ProgressRequest>(json!({"epochs":[{"epoch":0,"duration_ms":12.0,"steps":2,"metrics":{"loss":0.25}}],"profiles":[{"summary":{"private":"profile"}}]})).unwrap()).await.unwrap();
    let first = a.register_model(model()).await.unwrap();
    let label = || ModelLabelRequest {
        label: "production".into(),
        version: first.version.version.clone(),
    };
    a.set_label("same-model", label()).await.unwrap();
    for registry in [&b, &other, &legacy] {
        assert!(
            registry
                .runs(&RunFilter::default(), 50)
                .await
                .unwrap()
                .runs
                .is_empty()
        );
        assert!(registry.models().await.unwrap().models.is_empty());
        assert!(registry.run("same-run").await.is_err());
        assert!(registry.register_model(model()).await.is_err());
        assert!(
            registry
                .verified_version("same-model", &first.version.version)
                .await
                .is_err()
        );
        assert!(registry.set_label("same-model", label()).await.is_err());
    }
    b.start(start("data@abcd")).await.unwrap();
    let same = b.register_model(model()).await.unwrap();
    assert!(same.created);
    assert_eq!(first.version.version, same.version.version);
    assert!(
        b.model("same-model", None)
            .await
            .unwrap()
            .head
            .labels
            .is_empty()
    );
    assert!(b.run("same-run").await.unwrap().epochs.is_empty());
    let retry = a.start(start("replacement@ffff")).await.unwrap();
    assert_eq!(retry.dataset, run.dataset);
    assert_eq!(retry.epochs.len(), 1);
    let reopened = Registry::new(
        Arc::new(FileObjectStore::open(&dir).await.unwrap()),
        "training",
    )
    .for_project(a_scope)
    .unwrap();
    assert_eq!(
        to_value(reopened.run("same-run").await.unwrap()).unwrap(),
        to_value(retry).unwrap()
    );
    assert_eq!(
        to_value(
            reopened
                .verified_version("same-model", &first.version.version)
                .await
                .unwrap()
        )
        .unwrap(),
        to_value(&first.version).unwrap()
    );
    assert_eq!(
        reopened
            .model("same-model", None)
            .await
            .unwrap()
            .head
            .labels["production"],
        first.version.version
    );
    assert!(!reopened.register_model(model()).await.unwrap().created);
    assert!(a.for_project(b_scope).is_err());
    assert!(a.for_project(a_scope).is_ok());
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn both_scoped_and_legacy_model_reads_refuse_traversal_without_mutating_storage() {
    let dir = std::env::temp_dir().join(format!(
        "aiwatcher-training-traversal-{}",
        ProjectId::new().0
    ));
    let legacy = Registry::new(
        Arc::new(FileObjectStore::open(&dir).await.unwrap()),
        "training",
    );
    let scoped = legacy.for_project(scope()).unwrap();
    for registry in [&legacy, &scoped] {
        registry.start(start("data@abcd")).await.unwrap();
        registry.register_model(model()).await.unwrap();
        for bad in ["../head", "../../../../scopes/secret", "a/b", "a\\b"] {
            assert!(registry.model("same-model", Some(bad)).await.is_err());
            assert!(
                registry
                    .set_label(
                        "same-model",
                        ModelLabelRequest {
                            label: "production".into(),
                            version: bad.into()
                        }
                    )
                    .await
                    .is_err()
            );
            assert!(registry.verified_version("same-model", bad).await.is_err());
        }
        assert!(
            registry
                .model("same-model", None)
                .await
                .unwrap()
                .head
                .labels
                .is_empty()
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}
