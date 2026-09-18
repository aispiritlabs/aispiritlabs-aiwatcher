//! Storage boundary only: no project start, worker route or IAM grant is added.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use aiwatcher_core::ports::{AttemptArtifacts, PortError, PortResult};
use aiwatcher_core::prompts::{ObjectEntry, ObjectStore};
use aiwatcher_core::{ArtifactKind, ArtifactRef};
use aiwatcher_execution::{ArtifactCatalog, FailureClass};
use aiwatcher_iam::{OrganizationId, ProjectId, ProjectScope};
use aiwatcher_prompts::adapters::{fs::FileObjectStore, memory::MemoryObjectStore};
use aiwatcher_server::execution::artifacts::{Artifacts, ProjectArtifacts, Receipt, SCHEME};
use aiwatcher_server::execution::measure::{Stored, summarise};
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

/// One project attempt's whole footprint: the bytes, the receipt that says an
/// attempt produced them, and the three catalog families that describe them.
async fn one_attempt(
    pair: &ProjectArtifacts,
    body: &str,
    idempotency_key: &str,
) -> aiwatcher_core::ArtifactRef {
    let artifact = pair
        .artifacts()
        .put_spelled_rows("rows", body.as_bytes().to_vec())
        .await
        .unwrap();
    pair.artifacts()
        .put_receipt(&Receipt {
            idempotency_key: idempotency_key.to_owned(),
            artifact: artifact.clone(),
            rows: 1,
            runtime_digest: "runtime-encoding".to_owned(),
            stored_at: OffsetDateTime::UNIX_EPOCH,
        })
        .await
        .unwrap();
    aiwatcher_execution::artifact::object::record_outputs(
        pair.catalog().as_ref(),
        aiwatcher_execution::Provenance {
            execution_id: aiwatcher_execution::ExecutionId::new("execution-1"),
            step_id: "read".to_owned(),
            attempt: 1,
        },
        &[],
        std::slice::from_ref(&artifact),
        Some("step/read/plan-1"),
        OffsetDateTime::UNIX_EPOCH,
    )
    .await
    .unwrap();
    artifact
}

#[tokio::test]
async fn the_pair_a_project_is_given_binds_both_halves_to_one_scope() {
    // A scoped byte store beside a deployment-wide catalog would record a
    // project's outputs where anybody's cache lookup answers with them, so the
    // two are constructed together or not at all.
    let scratch = Scratch::new();
    let store: Arc<dyn ObjectStore> = Arc::new(FileObjectStore::open(&scratch.0).await.unwrap());
    let mine = scope();
    let theirs = scope();
    let pair = ProjectArtifacts::bind(&store, mine).unwrap();
    assert_eq!(pair.scope(), mine);
    assert_eq!(
        pair.artifacts().project_scope(),
        Some(mine),
        "the byte store is bound"
    );

    let artifact = one_attempt(&pair, r#"[{"answer":1}]"#, "execution-1/read/1").await;
    assert!(artifact.uri.contains(&mine.project.0.to_string()));

    // Its own catalog describes it; nobody else's does, and the deployment-wide
    // one will not even take the pointer.
    let described = pair
        .catalog()
        .by_digest(&artifact.digest, ArtifactKind::Rows)
        .await
        .unwrap()
        .expect("its own manifest");
    assert_eq!(described.artifact, artifact);
    let others = [ProjectArtifacts::bind(&store, theirs).unwrap()];
    for other in &others {
        assert!(
            other
                .catalog()
                .by_digest(&artifact.digest, ArtifactKind::Rows)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            other
                .catalog()
                .cached("step/read/plan-1", OffsetDateTime::UNIX_EPOCH)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            other
                .catalog()
                .produced_by(&aiwatcher_execution::ExecutionId::new("execution-1"))
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            other
                .artifacts()
                .read_bytes(&artifact)
                .await
                .unwrap_err()
                .class,
            FailureClass::Policy
        );
    }
    let global = aiwatcher_execution::ObjectArtifactCatalog::new(Arc::clone(&store));
    assert!(
        global
            .by_digest(&artifact.digest, ArtifactKind::Rows)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        global
            .record(aiwatcher_execution::CatalogedArtifact {
                artifact: artifact.clone(),
                produced_by: None,
                inputs: Vec::new(),
                created_at: OffsetDateTime::UNIX_EPOCH,
            })
            .await
            .is_err(),
        "the deployment-wide catalog will not describe a project's artifact"
    );
}

#[tokio::test]
async fn a_projects_whole_footprint_is_measured_as_that_projects_and_deleted_by_nothing() {
    // Every family an attempt writes — the bytes, the receipt, the manifest,
    // the lineage pointer and the cache entry — lands under one prefix, and the
    // hourly walk counts them as that project's. Counted as the deployment's,
    // they would be a tenant's storage reported as everybody's; and a collector
    // built one day on "no deployment-wide history names this" would read the
    // absence of global history as permission to delete them.
    let scratch = Scratch::new();
    let store: Arc<dyn ObjectStore> = Arc::new(FileObjectStore::open(&scratch.0).await.unwrap());
    let mine = scope();
    let sibling = ProjectScope {
        project: ProjectId::new(),
        ..mine
    };
    let body = r#"[{"answer":1}]"#;

    // The deployment-wide namespace writes the same bytes under the same
    // attempt key and the same cache key.
    let root = Artifacts::new(Arc::clone(&store));
    let global = root
        .put_spelled_rows("rows", body.as_bytes().to_vec())
        .await
        .unwrap();
    root.put_receipt(&receipt(global.clone())).await.unwrap();
    aiwatcher_execution::artifact::object::record_outputs(
        &aiwatcher_execution::ObjectArtifactCatalog::new(Arc::clone(&store)),
        aiwatcher_execution::Provenance {
            execution_id: aiwatcher_execution::ExecutionId::new("execution-1"),
            step_id: "read".to_owned(),
            attempt: 1,
        },
        &[],
        std::slice::from_ref(&global),
        Some("step/read/plan-1"),
        OffsetDateTime::UNIX_EPOCH,
    )
    .await
    .unwrap();

    let mut projects = Vec::new();
    for scope in [mine, sibling] {
        let pair = ProjectArtifacts::bind(&store, scope).unwrap();
        let artifact = one_attempt(&pair, body, "execution-1/read/1").await;
        assert_eq!(artifact.digest, global.digest, "one content address");
        assert_ne!(artifact.uri, global.uri, "and a key per namespace");
        projects.push((scope, pair));
    }

    let entries = store.list("artifacts/").await.unwrap();
    let measured = summarise(&entries, OffsetDateTime::UNIX_EPOCH);
    let families = 5; // data, receipt, manifest, lineage pointer, cache entry
    assert_eq!(
        measured.global.objects, families,
        "the deployment's count is the deployment's alone"
    );
    for (scope, _) in &projects {
        assert_eq!(
            measured.project(*scope).expect("its own bucket").objects,
            families,
            "every family a project writes is that project's"
        );
    }
    assert_eq!(measured.unattributed, Stored::default());
    assert_eq!(measured.elsewhere, Stored::default());
    assert_eq!(
        entries.len() as u64,
        measured.global.objects
            + projects
                .iter()
                .map(|(scope, _)| measured.project(*scope).expect("a bucket").objects)
                .sum::<u64>(),
        "the buckets partition the walk"
    );

    // The walk is a measurement. Everything it counted is still there
    // afterwards, and every namespace still answers for its own.
    assert_eq!(store.list("artifacts/").await.unwrap().len(), entries.len());
    assert_eq!(root.read_bytes(&global).await.unwrap(), body.as_bytes());
    for (_, pair) in &projects {
        let described = pair
            .catalog()
            .produced_by(&aiwatcher_execution::ExecutionId::new("execution-1"))
            .await
            .unwrap();
        assert_eq!(described.len(), 1);
        assert_eq!(
            pair.artifacts()
                .read_bytes(&described[0].artifact)
                .await
                .unwrap(),
            body.as_bytes()
        );
    }
}
