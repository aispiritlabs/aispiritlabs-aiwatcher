//! Actual source-owner adapters: namespace, pins, source integrity and restart.
use super::*;
use aiwatcher_iam::{OrganizationId, ProjectId, ProjectScope};
use aiwatcher_server::evaluation::LocalSource;
use serde_json::{Value, json};

fn scope() -> ProjectScope {
    ProjectScope {
        organization: OrganizationId::new(),
        project: ProjectId::new(),
    }
}
fn rows(words: &str) -> aiwatcher_datasets::PublishDatasetRequest {
    serde_json::from_value(json!({"name":"golden/cases", "pipeline":"fixture", "source":"fixture",
        "columns":["case_id","input","expected","split"],"items":[
        {"case_id":"dev","input":{"question":"dev"},"expected":{"answer":"dev"},"split":"dev"},
        {"case_id":"test","input":{"question":words},"expected":{"answer":words},"split":"test"},
        {"case_id":"shared","input":{"question":"shared"},"expected":{"answer":"shared"}}
    ]})).unwrap()
}
fn request_for(kind: DatasetKind, name: &str, version: &str) -> CohortRequest {
    CohortRequest {
        dataset: DatasetReference {
            kind,
            name: name.into(),
            version: version.into(),
        },
        split: "test".into(),
        limit: Some(1),
    }
}
fn registry_for(store: Arc<dyn ObjectStore>) -> Registry {
    Registry::new(
        store.clone(),
        Arc::new(
            LocalSource::new(None)
                .with_curation(Arc::new(aiwatcher_datasets::Registry::new(
                    store.clone(),
                    "datasets",
                )))
                .with_annotations(Arc::new(aiwatcher_annotations::Registry::new(
                    store,
                    "annotations",
                ))),
        ),
        Default::default(),
    )
    .unwrap()
}

#[tokio::test]
async fn project_curation_cohorts_preserve_pins_bytes_and_history_without_cross_scope_fallback() {
    let dir = std::env::temp_dir().join(format!("aiwatcher-cohorts-{}", OrganizationId::new().0));
    let store = Arc::new(FileObjectStore::open(&dir).await.unwrap());
    let datasets = aiwatcher_datasets::Registry::new(store.clone(), "datasets");
    let a_scope = scope();
    let b_scope = ProjectScope {
        project: ProjectId::new(),
        ..a_scope
    };
    let c_scope = scope();
    let a = datasets.for_project(a_scope).unwrap();
    let pin = a
        .publish(rows("A secret"))
        .await
        .unwrap()
        .dataset
        .latest
        .version;
    let request = request_for(DatasetKind::Curation, "golden/cases", &pin);
    let legacy = registry_for(store.clone());
    let ar = legacy.for_project_cohorts(a_scope).unwrap();
    let br = legacy.for_project_cohorts(b_scope).unwrap();
    let cr = legacy.for_project_cohorts(c_scope).unwrap();
    let derived = ar.derive_cohort(&request, "author", 100).await.unwrap();
    assert_eq!(
        (
            derived.cohort.case_count,
            derived.available,
            derived.unsplit
        ),
        (1, 2, Some(0))
    );
    let digest = &derived.cohort.case_manifest.digest;
    for other in [&legacy, &br, &cr] {
        assert!(other.derive_cohort(&request, "author", 100).await.is_err());
        assert!(other.derived_cohort(digest).await.unwrap().is_none());
    }
    // Same rows stored independently keep the same dataset and cohort identities.
    for ds in [&datasets, &datasets.for_project(b_scope).unwrap()] {
        assert_eq!(
            ds.publish(rows("A secret"))
                .await
                .unwrap()
                .dataset
                .latest
                .version,
            pin
        );
    }
    assert_eq!(
        legacy.derive_cohort(&request, "author", 100).await.unwrap(),
        derived
    );
    assert_eq!(
        br.derive_cohort(&request, "author", 100).await.unwrap(),
        derived
    );
    a.publish(rows("new A head")).await.unwrap();
    let reopened = registry_for(Arc::new(FileObjectStore::open(&dir).await.unwrap()))
        .for_project_cohorts(a_scope)
        .unwrap();
    assert_eq!(
        reopened.derived_cohort(digest).await.unwrap(),
        Some(derived.clone())
    );
    assert_eq!(
        reopened
            .derive_cohort(&request, "later", 200)
            .await
            .unwrap(),
        derived
    );
    let whole = reopened
        .derive_cohort(
            &CohortRequest {
                limit: None,
                ..request.clone()
            },
            "author",
            201,
        )
        .await
        .unwrap();
    assert_eq!(
        (whole.cohort.case_count, whole.available, whole.unsplit),
        (2, 2, Some(1))
    );
    assert_ne!(whole.cohort.case_manifest.digest, *digest);
    let prefix = format!(
        "evaluation-scopes/{}/{}/registry/",
        a_scope.organization.0, a_scope.project.0
    );
    let relative = format!("evaluation-cohorts/{digest}.json");
    assert_eq!(
        store.get(&format!("{prefix}{relative}")).await.unwrap(),
        store.get(&relative).await.unwrap()
    );
    assert!(ar.for_project_cohorts(b_scope).is_err());
    assert!(ar.for_project_cohorts(a_scope).is_ok());
    // Each derivation verifies source bytes again, while saved metadata remains historical.
    let ds_prefix = format!(
        "datasets/scopes/{}/{}/registry/",
        a_scope.organization.0, a_scope.project.0
    );
    let entry = store
        .list(&ds_prefix)
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.key.ends_with(&format!("/{pin}.json")))
        .unwrap();
    let original = store.get(&entry.key).await.unwrap().unwrap();
    let mut corrupt: Value = serde_json::from_slice(&original).unwrap();
    corrupt["items"][1]["expected"]["answer"] = json!("tampered");
    store
        .put(&entry.key, serde_json::to_vec(&corrupt).unwrap())
        .await
        .unwrap();
    assert!(
        reopened
            .derive_cohort(&request, "author", 202)
            .await
            .is_err()
    );
    store.delete(&entry.key).await.unwrap();
    assert!(
        reopened
            .derive_cohort(&request, "author", 203)
            .await
            .is_err()
    );
    assert!(br.derive_cohort(&request, "author", 203).await.is_ok());
    assert_eq!(
        reopened.derived_cohort(digest).await.unwrap(),
        Some(derived)
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn project_annotation_cohorts_require_local_exports_and_verified_image_bytes() {
    let store = Arc::new(MemoryObjectStore::new());
    let root = aiwatcher_annotations::Registry::new(store.clone(), "annotations");
    let a_scope = scope();
    let a = root.for_project(a_scope).unwrap();
    a.save_project(serde_json::from_value(json!({"name":"images/cases", "classes":[{"name":"point","geometry":"point","color":"#334155"}],"split_overrides":{"held-out":"test"}})).unwrap()).await.unwrap();
    let blob = a
        .put_blob(
            b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"/>".to_vec(),
            "image/svg+xml",
        )
        .await
        .unwrap();
    a.register_image(serde_json::from_value(json!({"project":"images/cases","image_id":blob.image_id,"uri":blob.uri,
        "width":10,"height":10,"group_id":"held-out","rights":{"kind":"owned","grant":"test fixture"}})).unwrap()).await.unwrap();
    a.save_revision(serde_json::from_value(json!({"project":"images/cases","image_id":blob.image_id,"accept":true,
        "annotations":[{"id":"point-1","class":"point","origin":"human","geometry":{"kind":"point","at":[2.0,3.0]}}]})).unwrap(), "author").await.unwrap();
    let export = a
        .export(serde_json::from_value(json!({"project":"images/cases"})).unwrap())
        .await
        .unwrap()
        .manifest;
    let request = request_for(DatasetKind::Annotations, &export.project, &export.export);
    let legacy = registry_for(store.clone());
    let scoped = legacy.for_project_cohorts(a_scope).unwrap();
    let derived = scoped.derive_cohort(&request, "author", 100).await.unwrap();
    assert_eq!(derived.cohort.case_count, 1);
    assert!(legacy.derive_cohort(&request, "author", 100).await.is_err());
    assert!(
        legacy
            .for_project_cohorts(scope())
            .unwrap()
            .derive_cohort(&request, "author", 100)
            .await
            .is_err()
    );
    let prefix = format!(
        "annotations/scopes/{}/{}/registry/",
        a_scope.organization.0, a_scope.project.0
    );
    let key = store
        .list(&prefix)
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.key.contains("/blobs/") && e.key.ends_with(&blob.image_id))
        .unwrap()
        .key;
    store.delete(&key).await.unwrap();
    assert!(scoped.derive_cohort(&request, "author", 101).await.is_err());
}

#[tokio::test]
async fn project_cohort_factory_does_not_keep_instance_bundle_or_content_capabilities() {
    let directory = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/fixtures/evaluation-v1");
    let manifest: EvaluationManifest = serde_json::from_str(include_str!(
        "../../../../contracts/fixtures/evaluation-v1/manifest.json"
    ))
    .unwrap();
    let source = LocalSource::new(Some(directory.to_str().unwrap().into()));
    assert!(source.resolve(&manifest, "admin").await.is_ok());
    let project = scope();
    let scoped = source.for_project_cohorts(project).unwrap();
    assert!(scoped.resolve(&manifest, "admin").await.is_err());
    for kind in [
        DatasetKind::Conversations,
        DatasetKind::External,
        DatasetKind::Assessments,
    ] {
        assert!(
            scoped
                .derive_cohort(&request_for(kind, "private", &"a".repeat(64)), "admin")
                .await
                .is_err()
        );
    }
    assert!(scoped.for_project_cohorts(scope()).is_err());
    assert!(scoped.for_project_cohorts(project).is_ok());
    assert!(
        Source::default().for_project_cohorts(project).is_err(),
        "adapters without explicit support fail closed"
    );
}
