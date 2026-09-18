//! Storage boundary only: no project start, worker route or IAM grant is added.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use aiwatcher_core::ports::{AttemptArtifacts, PortError, PortResult};
use aiwatcher_core::prompts::{ObjectEntry, ObjectStore};
use aiwatcher_core::{ArtifactKind, ArtifactRef};
use aiwatcher_execution::FailureClass;
use aiwatcher_iam::{OrganizationId, ProjectId, ProjectScope};
use aiwatcher_prompts::adapters::{fs::FileObjectStore, memory::MemoryObjectStore};
use aiwatcher_server::execution::artifacts::{Artifacts, Receipt, SCHEME};
use async_trait::async_trait;
use time::OffsetDateTime;

fn scope() -> ProjectScope {
    ProjectScope {
        organization: OrganizationId::new(),
        project: ProjectId::new(),
    }
}

fn receipt(artifact: ArtifactRef) -> Receipt {
    Receipt {
        idempotency_key: "execution/step/1".to_owned(),
        artifact,
        rows: 1,
        runtime_digest: "runtime-encoding".to_owned(),
        stored_at: OffsetDateTime::UNIX_EPOCH,
    }
}

fn receipt_key(scope: ProjectScope) -> String {
    format!(
        "artifacts/scopes/{}/{}/registry/receipts/{}.json",
        scope.organization.0,
        scope.project.0,
        aiwatcher_jobs::digest(b"execution/step/1")
    )
}

struct Scratch(std::path::PathBuf);
impl Scratch {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "aiwatcher-project-artifacts-{}",
            ProjectId::new().0
        )))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn identical_bytes_and_attempt_keys_stay_separate_across_projects_and_reopen() {
    let scratch = Scratch::new();
    let store = Arc::new(FileObjectStore::open(&scratch.0).await.unwrap());
    let root = Artifacts::new(store.clone());
    let first = scope();
    let second = ProjectScope {
        project: ProjectId::new(),
        ..first
    };
    let other_org = ProjectScope {
        organization: OrganizationId::new(),
        ..first
    };
    let stores = [
        root.clone(),
        root.for_project(first).unwrap(),
        root.for_project(second).unwrap(),
        root.for_project(other_org).unwrap(),
    ];
    let body = r#"[{"answer":123456789012345678901234567890}]"#;
    let mut references = Vec::new();
    let mut logs = Vec::new();
    for artifacts in &stores {
        let artifact = AttemptArtifacts::put_rows_as_spelled(artifacts, "answers", body.to_owned())
            .await
            .unwrap();
        assert_eq!(artifact.digest, aiwatcher_jobs::digest(body.as_bytes()));
        assert_eq!(artifact.size_bytes, Some(body.len() as u64));
        assert_eq!(artifact.kind, ArtifactKind::Rows);
        artifacts
            .put_receipt(&receipt(artifact.clone()))
            .await
            .unwrap();
        references.push(artifact);
        logs.push(artifacts.put_log(b"attempt output").await.unwrap());
    }
    for (index, artifacts) in stores.iter().enumerate() {
        for (other, reference) in references.iter().enumerate() {
            if index == other {
                assert_eq!(
                    artifacts.read_bytes(reference).await.unwrap(),
                    body.as_bytes()
                );
                assert_eq!(
                    artifacts.read_bytes(&logs[other]).await.unwrap(),
                    b"attempt output"
                );
            } else {
                assert_ne!(reference.uri, references[index].uri);
                assert_eq!(
                    artifacts.read_bytes(reference).await.unwrap_err().class,
                    FailureClass::Policy
                );
                assert_eq!(
                    artifacts.holds(&logs[other]).await.unwrap_err().class,
                    FailureClass::Policy
                );
                assert!(matches!(
                    AttemptArtifacts::read_bytes(artifacts, reference).await,
                    Err(PortError::Rejected { .. })
                ));
            }
        }
        assert_eq!(
            artifacts
                .receipt("execution/step/1")
                .await
                .unwrap()
                .unwrap()
                .artifact,
            references[index]
        );
    }

    drop(stores);
    drop(root);
    drop(store);
    let reopened_store = Arc::new(FileObjectStore::open(&scratch.0).await.unwrap());
    let reopened = Artifacts::new(reopened_store.clone());
    let project = reopened.for_project(first).unwrap();
    assert_eq!(
        project.read_bytes(&references[1]).await.unwrap(),
        body.as_bytes()
    );
    assert_eq!(
        project
            .receipt("execution/step/1")
            .await
            .unwrap()
            .unwrap()
            .artifact,
        references[1]
    );
    assert_eq!(
        AttemptArtifacts::put_rows_as_spelled(&project, "answers", body.to_owned())
            .await
            .unwrap(),
        references[1]
    );
    reopened_store
        .delete(references[1].uri.strip_prefix(SCHEME).unwrap())
        .await
        .unwrap();
    assert!(!project.holds(&references[1]).await.unwrap());
    assert_eq!(
        project.read_bytes(&references[1]).await.unwrap_err().class,
        FailureClass::Infrastructure
    );
    assert_eq!(
        reopened.read_bytes(&references[0]).await.unwrap(),
        body.as_bytes()
    );
    assert_eq!(
        reopened
            .for_project(second)
            .unwrap()
            .read_bytes(&references[2])
            .await
            .unwrap(),
        body.as_bytes()
    );
    assert_eq!(
        reopened
            .for_project(other_org)
            .unwrap()
            .read_bytes(&references[3])
            .await
            .unwrap(),
        body.as_bytes()
    );
}

#[test]
fn a_bound_artifact_store_cannot_be_rebound_to_another_project() {
    let root = Artifacts::new(Arc::new(MemoryObjectStore::new()));
    let first = scope();
    let project = root.for_project(first).unwrap();
    assert_eq!(root.project_scope(), None);
    assert_eq!(
        project.for_project(first).unwrap().project_scope(),
        Some(first)
    );
    assert_eq!(
        project.for_project(scope()).unwrap_err().class,
        FailureClass::Policy
    );
    assert_eq!(
        project
            .for_project(ProjectScope {
                project: ProjectId::new(),
                ..first
            })
            .unwrap_err()
            .class,
        FailureClass::Policy
    );
}

#[derive(Debug, Default)]
struct CountedStore {
    inner: MemoryObjectStore,
    reads: AtomicUsize,
    writes: AtomicUsize,
}
#[async_trait]
impl ObjectStore for CountedStore {
    async fn get(&self, key: &str) -> PortResult<Option<Vec<u8>>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.get(key).await
    }
    async fn put(&self, key: &str, bytes: Vec<u8>) -> PortResult<()> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.inner.put(key, bytes).await
    }
    async fn create(&self, key: &str, bytes: Vec<u8>) -> PortResult<bool> {
        self.inner.create(key, bytes).await
    }
    async fn list(&self, prefix: &str) -> PortResult<Vec<ObjectEntry>> {
        self.inner.list(prefix).await
    }
    async fn delete(&self, key: &str) -> PortResult<()> {
        self.inner.delete(key).await
    }
}

#[tokio::test]
async fn forged_references_are_refused_before_any_object_store_io() {
    let store = Arc::new(CountedStore::default());
    let root = Artifacts::new(store.clone());
    let first = scope();
    let project = root.for_project(first).unwrap();
    for artifacts in [&root, &project] {
        let valid = artifacts
            .put_spelled_rows("rows", b"[]".to_vec())
            .await
            .unwrap();
        let mut forged = Vec::new();
        for uri in [
            "object://evaluation-scopes/private/recording".to_owned(),
            "object://prompts/private/versions/secret".to_owned(),
            format!("{}?project=other", valid.uri),
            valid.uri.replace("/rows/", "/rows/../rows/"),
            valid.uri.replace("/rows/", "/rows/%2e%2e/rows/"),
            valid.uri.replace("/rows/", "/rows//"),
            format!("object://{}", receipt_key(first)),
        ] {
            forged.push(ArtifactRef {
                uri,
                ..valid.clone()
            });
        }
        for digest in [
            "../secret".to_owned(),
            "ab".to_owned(),
            "AB".repeat(32),
            "00".repeat(32),
        ] {
            forged.push(ArtifactRef {
                digest,
                ..valid.clone()
            });
        }
        forged.push(ArtifactRef {
            kind: ArtifactKind::Log,
            ..valid.clone()
        });
        let reads = store.reads.load(Ordering::SeqCst);
        let writes = store.writes.load(Ordering::SeqCst);
        for reference in forged {
            assert_eq!(
                artifacts.read_bytes(&reference).await.unwrap_err().class,
                FailureClass::Policy
            );
            assert!(matches!(
                AttemptArtifacts::holds(artifacts, &reference).await,
                Err(PortError::Rejected { .. })
            ));
            assert_eq!(
                artifacts
                    .put_receipt(&receipt(reference))
                    .await
                    .unwrap_err()
                    .class,
                FailureClass::Policy
            );
        }
        assert_eq!(store.reads.load(Ordering::SeqCst), reads);
        assert_eq!(store.writes.load(Ordering::SeqCst), writes);
    }
}

#[tokio::test]
async fn receipts_cannot_import_foreign_references_or_another_attempts_identity() {
    let store = Arc::new(MemoryObjectStore::new());
    let root = Artifacts::new(store.clone());
    let first = scope();
    let project = root.for_project(first).unwrap();
    let foreign = root.put_spelled_rows("rows", b"[]".to_vec()).await.unwrap();
    assert_eq!(
        project
            .put_receipt(&receipt(foreign.clone()))
            .await
            .unwrap_err()
            .class,
        FailureClass::Policy
    );
    assert!(project.receipt("execution/step/1").await.unwrap().is_none());
    // Corruption in storage must not bypass the check made on writes.
    store
        .put(
            &receipt_key(first),
            serde_json::to_vec(&receipt(foreign)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        project.receipt("execution/step/1").await.unwrap_err().class,
        FailureClass::Policy
    );
    let local = project
        .put_spelled_rows("rows", b"[]".to_vec())
        .await
        .unwrap();
    let mut wrong_attempt = receipt(local.clone());
    wrong_attempt.idempotency_key = "another/step/2".to_owned();
    store
        .put(
            &receipt_key(first),
            serde_json::to_vec(&wrong_attempt).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        project.receipt("execution/step/1").await.unwrap_err().class,
        FailureClass::Policy
    );
    store
        .put(&receipt_key(first), b"{unreadable".to_vec())
        .await
        .unwrap();
    assert!(project.receipt("execution/step/1").await.unwrap().is_none());
    project.put_receipt(&receipt(local)).await.unwrap();
    assert!(project.receipt("execution/step/1").await.unwrap().is_some());
    assert!(project.receipt("execution/step/2").await.unwrap().is_none());
}

#[tokio::test]
async fn project_bytes_are_verified_and_worker_row_validation_still_applies() {
    let store = Arc::new(MemoryObjectStore::new());
    let project = Artifacts::new(store.clone()).for_project(scope()).unwrap();
    let artifact = project.put_log(b"original").await.unwrap();
    store
        .put(
            artifact.uri.strip_prefix(SCHEME).unwrap(),
            b"changed".to_vec(),
        )
        .await
        .unwrap();
    assert_eq!(
        project.read_bytes(&artifact).await.unwrap_err().class,
        FailureClass::Infrastructure
    );
    assert!(matches!(
        AttemptArtifacts::put_rows_as_spelled(&project, "rows", "[1]".to_owned()).await,
        Err(PortError::Rejected { .. })
    ));
    assert!(matches!(
        AttemptArtifacts::put_rows(&project, "rows", vec![serde_json::json!(1)]).await,
        Err(PortError::Rejected { .. })
    ));
}
