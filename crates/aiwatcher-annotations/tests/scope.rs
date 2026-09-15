//! Physical scope includes blobs, immutable drawings and derived exports.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use aiwatcher_annotations::{
    ExportRequest, ImageFilter, RegisterImageRequest, Registry, ReviewRequest, SaveProjectRequest,
    SaveRevisionRequest,
};
use aiwatcher_iam::{OrganizationId, ProjectId, ProjectScope};
use aiwatcher_prompts::adapters::fs::FileObjectStore;
use serde_json::{Value, json, to_value};
use std::sync::Arc;

fn scope() -> ProjectScope {
    ProjectScope {
        organization: OrganizationId::new(),
        project: ProjectId::new(),
    }
}
fn project() -> SaveProjectRequest {
    serde_json::from_value(json!({"name":"same/collection","classes":[{"name":"region","geometry":"polygon"}],"split_salt":"stable"})).unwrap()
}
fn image(id: &str) -> RegisterImageRequest {
    serde_json::from_value(json!({"project":"same/collection","image_id":id,"uri":format!("aiwatcher://blob/{id}"),"width":100,"height":100,"group_id":"family","rights":{"kind":"owned","grant":"original grant"}})).unwrap()
}
async fn populate(registry: &Registry) -> (String, Value, Value) {
    let blob = registry
        .put_blob(b"original image bytes".to_vec(), "image/png")
        .await
        .unwrap();
    registry.save_project(project()).await.unwrap();
    registry
        .register_image(image(&blob.image_id))
        .await
        .unwrap();
    let drawing:SaveRevisionRequest=serde_json::from_value(json!({"project":"same/collection","image_id":blob.image_id,"accept":true,"annotations":[{"id":"region-1","class":"region","geometry":{"kind":"polygon","exterior":[[0.0,0.0],[50.0,0.0],[50.0,50.0],[0.0,50.0]]},"attributes":{}}]})).unwrap();
    let revision = registry
        .save_revision(drawing, "original-author")
        .await
        .unwrap();
    let export: ExportRequest =
        serde_json::from_value(json!({"project":"same/collection","note":"original export"}))
            .unwrap();
    let exported = registry.export(export).await.unwrap();
    assert_eq!(exported.manifest.counts.images, 1);
    (
        blob.image_id,
        to_value(revision.revision).unwrap(),
        to_value(exported.manifest).unwrap(),
    )
}

#[tokio::test]
async fn physical_namespaces_survive_reopen_with_identical_hashes_and_isolated_review() {
    let a_scope = scope();
    let dir =
        std::env::temp_dir().join(format!("aiwatcher-annotations-scope-{}", a_scope.project.0));
    let legacy = Registry::new(
        Arc::new(FileObjectStore::open(&dir).await.unwrap()),
        "annotations",
    );
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
    let (id, revision, manifest) = populate(&a).await;
    let revision_id = revision["revision"].as_str().unwrap();
    let export_id = manifest["export"].as_str().unwrap();
    for registry in [&legacy, &b, &other] {
        assert!(registry.projects().await.unwrap().projects.is_empty());
        assert!(registry.blob(&id).await.is_err());
        assert!(
            registry
                .image("same/collection", &id, Some(revision_id))
                .await
                .is_err()
        );
        assert!(
            registry
                .export_manifest("same/collection", export_id)
                .await
                .is_err()
        );
        assert!(
            registry
                .coco("same/collection", export_id, None)
                .await
                .is_err()
        );
    }
    let (same_id, same_revision, same_manifest) = populate(&b).await;
    assert_eq!(id, same_id);
    assert_eq!(revision["revision"], same_revision["revision"]);
    assert_eq!(manifest["export"], same_manifest["export"]);
    b.review(
        serde_json::from_value::<ReviewRequest>(
            json!({"project":"same/collection","image_id":id,"review":"rejected","note":"B only"}),
        )
        .unwrap(),
        "other-reviewer",
    )
    .await
    .unwrap();
    let reopened = Registry::new(
        Arc::new(FileObjectStore::open(&dir).await.unwrap()),
        "annotations",
    )
    .for_project(a_scope)
    .unwrap();
    assert_eq!(
        reopened.blob(&id).await.unwrap(),
        (b"original image bytes".to_vec(), "image/png".into())
    );
    let detail = reopened
        .image("same/collection", &id, Some(revision_id))
        .await
        .unwrap();
    assert_eq!(to_value(detail.revision).unwrap(), revision);
    assert_eq!(to_value(detail.head.review).unwrap(), json!("accepted"));
    assert_eq!(
        to_value(
            reopened
                .export_manifest("same/collection", export_id)
                .await
                .unwrap()
        )
        .unwrap(),
        manifest
    );
    assert_eq!(
        reopened
            .coco("same/collection", export_id, None)
            .await
            .unwrap()["images"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // The immutable verification path reads from this namespace as well.
    reopened
        .verified_coco("same/collection", export_id, detail.split)
        .await
        .unwrap();
    assert_eq!(
        reopened
            .images("same/collection", &ImageFilter::default(), 0, 50)
            .await
            .unwrap()
            .total,
        1
    );
    assert!(a.for_project(b_scope).is_err());
    assert!(a.for_project(a_scope).is_ok());
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn local_blob_references_require_bytes_in_the_same_iam_project() {
    let dir =
        std::env::temp_dir().join(format!("aiwatcher-annotations-blob-{}", ProjectId::new().0));
    let legacy = Registry::new(
        Arc::new(FileObjectStore::open(&dir).await.unwrap()),
        "annotations",
    );
    let scoped = legacy.for_project(scope()).unwrap();
    scoped.save_project(project()).await.unwrap();
    let blob = legacy
        .put_blob(b"legacy only".to_vec(), "image/png")
        .await
        .unwrap();
    assert!(scoped.register_image(image(&blob.image_id)).await.is_err());
    assert!(
        scoped
            .images("same/collection", &ImageFilter::default(), 0, 50)
            .await
            .unwrap()
            .images
            .is_empty()
    );
    let copied = scoped
        .put_blob(b"legacy only".to_vec(), "image/png")
        .await
        .unwrap();
    assert!(copied.created);
    assert_eq!(copied.image_id, blob.image_id);
    scoped
        .register_image(image(&copied.image_id))
        .await
        .unwrap();
    let mut mismatch = image(&copied.image_id);
    mismatch.uri = format!("aiwatcher://blob/{}", "f".repeat(64));
    assert!(scoped.register_image(mismatch).await.is_err());
    assert!(scoped.blob("../blobs/secret").await.is_err());
    assert!(
        scoped
            .export_manifest("same/collection", "../../legacy")
            .await
            .is_err()
    );
    let unsafe_root = Registry::new(
        Arc::new(FileObjectStore::open(&dir).await.unwrap()),
        "annotations/../outside",
    );
    assert!(unsafe_root.for_project(scope()).is_err());
    std::fs::remove_dir_all(dir).unwrap();
}
