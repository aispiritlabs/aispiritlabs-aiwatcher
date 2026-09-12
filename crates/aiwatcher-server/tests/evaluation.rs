#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! The same durable protocol over the actual adapters, without a projection.
use aiwatcher_core::ports::{PortError, PortResult};
use aiwatcher_core::storage::{ObjectEntry, ObjectStore};
use aiwatcher_evaluation::*;
use aiwatcher_prompts::adapters::{fs::FileObjectStore, memory::MemoryObjectStore};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

#[path = "evaluation/approvals.rs"]
mod approvals;

#[path = "evaluation/assessments.rs"]
mod assessments;

#[path = "evaluation/scorecards.rs"]
mod scorecards;

#[path = "evaluation/cost.rs"]
mod cost;

#[path = "evaluation/gc.rs"]
mod gc;

#[path = "evaluation/curation.rs"]
mod curation;

#[path = "evaluation/fixture.rs"]
mod fixture;

#[path = "evaluation/catalogue.rs"]
mod catalogue;
#[path = "evaluation/comparison.rs"]
mod comparison;
#[path = "evaluation/prompts.rs"]
mod prompts;
#[path = "evaluation/staging.rs"]
mod staging;

#[derive(Debug, Default)]
struct Source {
    mode: AtomicU8,
}
#[async_trait]
impl SourceAuthority for Source {
    async fn resolve(&self, manifest: &EvaluationManifest, _: &str) -> Result<SourceEvidence> {
        match self.mode.load(Ordering::SeqCst) {
            1 => return Err(EvaluationError::Unavailable(EvidenceState::DeletedSource)),
            2 => return Err(EvaluationError::Unavailable(EvidenceState::Forbidden)),
            _ => {}
        }
        Ok(SourceEvidence {
            expected: (0..manifest.context.case_count)
                .map(|n| (format!("case-{n:05}"), serde_json::json!({"answer": ""})))
                .collect(),
            ..Default::default()
        })
    }
}
fn request(id: &str, count: u64) -> PublishEvaluation {
    let mut manifest: EvaluationManifest = serde_json::from_str(include_str!(
        "../../../contracts/fixtures/evaluation-v1/manifest.json"
    ))
    .unwrap();
    manifest.origin.evaluation_id = id.into();
    manifest.context.case_count = count;
    PublishEvaluation {
        cases: (0..count)
            .map(|n| CaseMeasurement {
                case_id: format!("case-{n:05}"),
                repetition_id: manifest.origin.repetition_id.clone(),
                actual: Some(serde_json::json!({"answer": ""})),
                metrics: BTreeMap::from([("accuracy".into(), 1.0)]),
                error: None,
                trace_id: None,
                span_id: None,
            })
            .collect(),
        manifest,
        status: ResultStatus::Succeeded,
    }
}
fn registry(store: Arc<dyn ObjectStore>, source: Arc<Source>) -> Registry {
    Registry::new(store, source, RegistryConfig::default()).unwrap()
}
/// What this evidence reads as, from its header and from the page behind it.
///
/// A summary answers from one verified object; a shard is verified when the
/// page it is on is read. So damage shows on whichever of the two holds it,
/// and neither surface ever serves the damaged bytes as data.
async fn evidence(
    registry: &Registry,
    receipt: &EvaluationReceipt,
    subject: &str,
    now: i64,
) -> EvidenceState {
    let detail = registry
        .get(&receipt.evaluation_id, subject, now)
        .await
        .unwrap()
        .unwrap();
    if !matches!(
        detail.state,
        EvidenceState::Complete | EvidenceState::Partial
    ) {
        return detail.state;
    }
    let mut cursor = None;
    loop {
        let page = registry
            .cases(
                &receipt.evaluation_id,
                &receipt.version,
                cursor.as_deref(),
                Some(200),
                subject,
                now,
            )
            .await
            .unwrap()
            .unwrap();
        if !matches!(page.state, EvidenceState::Complete | EvidenceState::Partial) {
            assert!(
                page.cases.is_empty(),
                "damaged evidence never arrives as rows"
            );
            return page.state;
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            return detail.state;
        }
    }
}

/// Admit a pair over HTTP, the way an operator does: `Admin`, because an
/// editor is what a producer's own ingest token holds.
async fn admit(client: &reqwest::Client, base: &str, manifest: &EvaluationManifest) {
    let response = client
        .post(format!("{base}/api/v1/evaluation-approvals"))
        .header("x-authentik-username", "operator")
        .header("x-authentik-groups", "aiwatcher-admins")
        .json(manifest)
        .send()
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "{}",
        response.text().await.unwrap()
    );
}
/// Publication needs an admitted pair. Admitting one is the operator act these
/// tests are not about, so every fixture admits its own declaration first — and
/// ignores a refusal, because a source that cannot be resolved cannot be
/// approved either, and publication is about to fail for that same reason.
async fn publish(
    registry: &Registry,
    request: PublishEvaluation,
    subject: &str,
    now: i64,
) -> Result<EvaluationReceipt> {
    let _ = registry.approve(&request.manifest, "operator", now).await;
    registry.publish(request, subject, now).await
}
async fn contract(store: Arc<dyn ObjectStore>) {
    let source = Arc::new(Source::default());
    let first = registry(store.clone(), source.clone());
    let second = registry(store.clone(), source.clone());
    let lossy = registry(
        Arc::new(LostResponse {
            inner: store.clone(),
            lost: AtomicU8::new(0),
        }),
        source.clone(),
    );
    assert!(
        publish(&lossy, request("lost-adapter", 3), "editor", 100)
            .await
            .is_err()
    );
    let recovered = publish(&lossy, request("lost-adapter", 3), "editor", 200)
        .await
        .unwrap();
    assert_eq!(recovered.committed_at, 100);
    let a = request("concurrent", 1201);
    let mut b = a.clone();
    b.cases[0].metrics.insert("accuracy".into(), 0.0);
    let (ra, rb) = tokio::join!(
        publish(&first, a.clone(), "editor", 100),
        publish(&second, b.clone(), "editor", 101)
    );
    assert_ne!(ra.is_ok(), rb.is_ok(), "exactly one immutable result wins");
    let winning = if ra.is_ok() { a } else { b };
    assert!(matches!(
        ra.as_ref().err().or(rb.as_ref().err()),
        Some(EvaluationError::Conflict)
    ));
    let receipt = publish(&first, winning.clone(), "editor", 999)
        .await
        .unwrap();
    assert!(receipt.committed_at <= 101, "retry never resets retention");
    // Reconstruct the registry, with no event log or in-memory read model.
    drop(first);
    drop(second);
    let restarted = registry(store.clone(), source.clone());
    let detail = restarted
        .get("concurrent", "viewer", 1000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(detail.state, EvidenceState::Complete);
    assert_eq!(detail.counts.unwrap().scored, 1201);
    let mut cursor = None;
    let mut rows = Vec::new();
    loop {
        let page = restarted
            .cases(
                "concurrent",
                &receipt.version,
                cursor.as_deref(),
                Some(137),
                "viewer",
                1000,
            )
            .await
            .unwrap()
            .unwrap();
        assert!(page.cases.len() <= 137);
        rows.extend(page.cases);
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(rows.len(), 1201);
    assert_eq!(rows[0].expected, serde_json::json!({"answer": ""}));
    assert!(
        restarted
            .cases(
                "concurrent",
                &receipt.version,
                Some("wrong:137"),
                None,
                "viewer",
                1000
            )
            .await
            .is_err()
    );
    assert_eq!(
        restarted
            .list(None, 1, None, None, "viewer", 1000)
            .await
            .unwrap()
            .evaluations
            .len(),
        1
    );
    source.mode.store(2, Ordering::SeqCst);
    let denied = restarted
        .get("concurrent", "viewer", 1000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(denied.state, EvidenceState::Forbidden);
    assert!(denied.manifest.is_none() && denied.metrics.is_empty());
    source.mode.store(0, Ordering::SeqCst);
    assert_eq!(
        restarted
            .get("concurrent", "viewer", 1000)
            .await
            .unwrap()
            .unwrap()
            .state,
        EvidenceState::Complete
    );
    source.mode.store(1, Ordering::SeqCst);
    assert_eq!(restarted.sweep("worker", 1000).await.unwrap(), 2);
    source.mode.store(0, Ordering::SeqCst);
    assert_eq!(
        restarted
            .get("concurrent", "viewer", 1000)
            .await
            .unwrap()
            .unwrap()
            .state,
        EvidenceState::DeletedSource
    );
    assert!(
        publish(&restarted, winning, "editor", 1001).await.is_err(),
        "cannot resurrect erased data"
    );
    assert!(
        store
            .list("evaluations/")
            .await
            .unwrap()
            .iter()
            .all(|entry| !entry.key.contains("/content/"))
    );
}
#[tokio::test]
async fn memory_proves_the_same_protocol_as_durable_adapters() {
    contract(Arc::new(MemoryObjectStore::new())).await;
}
#[tokio::test]
async fn independent_file_writers_survive_reopen_and_revocation() {
    let path = std::env::temp_dir().join(format!("aiwatcher-b2-{}", std::process::id()));
    let _ = tokio::fs::remove_dir_all(&path).await;
    contract(Arc::new(FileObjectStore::open(&path).await.unwrap())).await;
    let reopened = registry(
        Arc::new(FileObjectStore::open(&path).await.unwrap()),
        Arc::new(Source::default()),
    );
    assert_eq!(
        reopened
            .get("concurrent", "viewer", 1000)
            .await
            .unwrap()
            .unwrap()
            .state,
        EvidenceState::DeletedSource
    );
    tokio::fs::remove_dir_all(path).await.unwrap();
}
#[tokio::test]
async fn missing_and_corrupt_artifacts_remain_known_and_never_look_complete() {
    for corrupt in [false, true] {
        let store: Arc<dyn ObjectStore> = Arc::new(MemoryObjectStore::new());
        let registry = registry(store.clone(), Arc::new(Source::default()));
        let receipt = publish(&registry, request("damaged", 3), "editor", 100)
            .await
            .unwrap();
        let shard = store
            .list("evaluations/")
            .await
            .unwrap()
            .into_iter()
            .find(|e| e.key.contains("/content/") && !e.key.contains(&receipt.version))
            .unwrap();
        if corrupt {
            store.put(&shard.key, b"[]".to_vec()).await.unwrap();
        } else {
            store.delete(&shard.key).await.unwrap();
        }
        assert_eq!(
            evidence(&registry, &receipt, "viewer", 200).await,
            if corrupt {
                EvidenceState::CorruptArtifact
            } else {
                EvidenceState::MissingArtifact
            }
        );
        // The header is one intact object and still answers how many cases
        // were measured. Nothing here turns the damage into a smaller result.
        let header = registry
            .get("damaged", "viewer", 200)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(header.counts.unwrap().scored, 3);
        // Damaging the header itself is the other half, and hides everything.
        let metadata = store
            .list("evaluations/")
            .await
            .unwrap()
            .into_iter()
            .find(|e| e.key.contains(&receipt.version))
            .unwrap();
        store.put(&metadata.key, b"[]".to_vec()).await.unwrap();
        let hidden = registry
            .get("damaged", "viewer", 200)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(hidden.state, EvidenceState::CorruptArtifact);
        assert!(hidden.manifest.is_none());
    }
}
#[tokio::test]
async fn partial_counts_and_retention_never_turn_missing_scores_into_zeroes() {
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryObjectStore::new());
    let registry = registry(store.clone(), Arc::new(Source::default()));
    let mut request = request("partial", 3);
    request.cases.pop();
    assert!(
        publish(&registry, request.clone(), "editor", 100)
            .await
            .is_err()
    );
    request.status = ResultStatus::Partial;
    request.cases[0].error = Some("scorer failed".into());
    request.cases[0].metrics.clear();
    let receipt = publish(&registry, request, "editor", 100).await.unwrap();
    let result = registry
        .get("partial", "viewer", 101)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.state, EvidenceState::Partial);
    let counts = result.counts.unwrap();
    assert_eq!(
        (
            counts.selected,
            counts.scored,
            counts.failed,
            counts.unscored
        ),
        (3, 1, 1, 1)
    );
    assert_eq!(result.metrics["accuracy"], 1.0);
    assert_eq!(
        registry.sweep("worker", receipt.expires_at).await.unwrap(),
        1
    );
    assert_eq!(
        registry
            .get("partial", "viewer", receipt.expires_at)
            .await
            .unwrap()
            .unwrap()
            .state,
        EvidenceState::Expired
    );
    assert!(
        store
            .list("evaluations/")
            .await
            .unwrap()
            .iter()
            .all(|e| !e.key.contains("/content/"))
    );
}

#[derive(Debug)]
struct LostResponse {
    inner: Arc<dyn ObjectStore>,
    lost: AtomicU8,
}
#[async_trait]
impl ObjectStore for LostResponse {
    async fn put(&self, k: &str, v: Vec<u8>) -> PortResult<()> {
        self.inner.put(k, v).await
    }
    async fn get(&self, k: &str) -> PortResult<Option<Vec<u8>>> {
        self.inner.get(k).await
    }
    async fn list(&self, k: &str) -> PortResult<Vec<ObjectEntry>> {
        self.inner.list(k).await
    }
    async fn delete(&self, k: &str) -> PortResult<()> {
        self.inner.delete(k).await
    }
    async fn create(&self, k: &str, v: Vec<u8>) -> PortResult<bool> {
        let result = self.inner.create(k, v).await?;
        if result && k.ends_with("claim.json") && self.lost.swap(1, Ordering::SeqCst) == 0 {
            return Err(PortError::Unavailable {
                target: "lost-response",
                message: "committed, response lost".into(),
            });
        }
        Ok(result)
    }
}
#[tokio::test]
async fn a_lost_commit_response_is_recoverable_without_a_duplicate_measurement() {
    let registry = registry(
        Arc::new(LostResponse {
            inner: Arc::new(MemoryObjectStore::new()),
            lost: AtomicU8::new(0),
        }),
        Arc::new(Source::default()),
    );
    assert!(
        publish(&registry, request("lost", 3), "editor", 100)
            .await
            .is_err()
    );
    let recovered = publish(&registry, request("lost", 3), "editor", 200)
        .await
        .unwrap();
    assert_eq!(recovered.committed_at, 100);
    assert_eq!(
        registry
            .list(None, 200, None, None, "viewer", 201)
            .await
            .unwrap()
            .evaluations
            .len(),
        1
    );
}

#[tokio::test]
async fn approved_local_fixture_is_verified_and_unknown_native_sources_are_refused() {
    let source = aiwatcher_server::evaluation::LocalSource::new(Some(format!(
        "{}/../../contracts/fixtures/evaluation-v1",
        env!("CARGO_MANIFEST_DIR")
    )));
    let mut manifest: EvaluationManifest = serde_json::from_str(include_str!(
        "../../../contracts/fixtures/evaluation-v1/manifest.json"
    ))
    .unwrap();
    let evidence = source.resolve(&manifest, "viewer").await.unwrap();
    assert_eq!(
        evidence.expected["empty"],
        serde_json::json!({"answer": ""})
    );
    manifest.context.dataset.kind = DatasetKind::Conversations;
    manifest.variant.dataset.kind = DatasetKind::Conversations;
    assert!(matches!(
        source.resolve(&manifest, "viewer").await,
        Err(EvaluationError::Unavailable(EvidenceState::Forbidden))
    ));
}

#[tokio::test]
#[ignore = "requires an isolated RustFS; set AIWATCHER_EVALUATION_TEST_S3_ENDPOINT"]
async fn real_s3_conditional_writes_enforce_the_same_publication_contract() {
    use aiwatcher_prompts::adapters::s3::{S3Config, S3ObjectStore};
    use aiwatcher_prompts::sigv4::Credentials;
    let endpoint =
        std::env::var("AIWATCHER_EVALUATION_TEST_S3_ENDPOINT").expect("explicit isolated endpoint");
    let config = S3Config {
        endpoint,
        bucket: format!("aiwatcher-fti-b2-{}", std::process::id()),
        credentials: Credentials {
            access_key_id: "rustfsadmin".into(),
            secret_access_key: "rustfsadmin".into(),
            session_token: None,
            region: "us-east-1".into(),
        },
        ..S3Config::default()
    };
    let store: Arc<dyn ObjectStore> =
        Arc::new(S3ObjectStore::connect(config.clone()).await.unwrap());
    contract(store.clone()).await;
    let reopened = registry(
        Arc::new(S3ObjectStore::connect(config).await.unwrap()),
        Arc::new(Source::default()),
    );
    assert_eq!(
        reopened
            .get("concurrent", "viewer", 1000)
            .await
            .unwrap()
            .unwrap()
            .state,
        EvidenceState::DeletedSource
    );
    for entry in store.list("").await.unwrap() {
        store.delete(&entry.key).await.unwrap();
    }
    gc::contract(store.clone()).await;
    for entry in store.list("").await.unwrap() {
        store.delete(&entry.key).await.unwrap();
    }
}

#[derive(Debug)]
struct ShortenedRetention(AtomicU8);
#[async_trait]
impl SourceAuthority for ShortenedRetention {
    async fn resolve(
        &self,
        manifest: &EvaluationManifest,
        subject: &str,
    ) -> Result<SourceEvidence> {
        let mut evidence = Source::default().resolve(manifest, subject).await?;
        evidence.expires_at = Some(if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            1000
        } else {
            150
        });
        Ok(evidence)
    }
}
#[tokio::test]
async fn a_source_retention_change_before_commit_shortens_the_receipt() {
    let registry = Registry::new(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(ShortenedRetention(AtomicU8::new(0))),
        RegistryConfig::default(),
    )
    .unwrap();
    let receipt = publish(&registry, request("shortened", 3), "editor", 100)
        .await
        .unwrap();
    assert_eq!(receipt.expires_at, 150);
    assert_eq!(
        registry
            .get("shortened", "viewer", 150)
            .await
            .unwrap()
            .unwrap()
            .state,
        EvidenceState::Expired
    );
}

#[tokio::test]
async fn changed_or_deleted_approved_files_are_not_replaced_by_trusted_declarations() {
    let original = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/fixtures/evaluation-v1");
    let root = std::env::temp_dir().join(format!("aiwatcher-b2-source-{}", std::process::id()));
    let _ = tokio::fs::remove_dir_all(&root).await;
    tokio::fs::create_dir_all(&root).await.unwrap();
    for entry in std::fs::read_dir(original).unwrap() {
        let entry = entry.unwrap();
        if !entry.file_type().unwrap().is_file() {
            continue;
        }
        tokio::fs::copy(entry.path(), root.join(entry.file_name()))
            .await
            .unwrap();
    }
    let manifest = serde_json::from_str(include_str!(
        "../../../contracts/fixtures/evaluation-v1/manifest.json"
    ))
    .unwrap();
    let source =
        aiwatcher_server::evaluation::LocalSource::new(Some(root.to_str().unwrap().into()));
    assert!(source.resolve(&manifest, "viewer").await.is_ok());
    tokio::fs::write(root.join("scorer.py"), "changed")
        .await
        .unwrap();
    assert!(matches!(
        source.resolve(&manifest, "viewer").await,
        Err(EvaluationError::Unavailable(EvidenceState::CorruptArtifact))
    ));
    tokio::fs::remove_file(root.join("scorer.py"))
        .await
        .unwrap();
    assert!(matches!(
        source.resolve(&manifest, "viewer").await,
        Err(EvaluationError::Unavailable(EvidenceState::DeletedSource))
    ));
    tokio::fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn the_full_byte_budget_is_rejected_before_any_artifact_is_written() {
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryObjectStore::new());
    let request = request("over-budget", 3);
    // The body fits; adding source expectations and immutable metadata does not.
    let config = RegistryConfig {
        max_bytes: serde_json::to_vec(&request).unwrap().len() + 200,
        ..Default::default()
    };
    let registry = Registry::new(store.clone(), Arc::new(Source::default()), config).unwrap();
    assert!(matches!(
        publish(&registry, request, "editor", 100).await,
        Err(EvaluationError::Invalid { .. })
    ));
    assert!(
        store
            .list("evaluations/")
            .await
            .unwrap()
            .iter()
            .all(|entry| entry.key.starts_with("evaluations/approvals/")),
        "an approved pair is not a published result"
    );
}

#[path = "evaluation/models.rs"]
mod models;

#[path = "evaluation/annotations.rs"]
mod annotations;

#[path = "evaluation/conversations.rs"]
mod conversations;
