//! Deterministic commit/collection interleavings, shared by real storage adapters.
use super::*;
use tokio::sync::Notify;

#[derive(Clone, Copy, Debug)]
enum Pause {
    BeforeArtifact,
    AfterArtifact,
    BeforeCommit,
    AfterCommit,
}

#[derive(Debug)]
struct PausedStore {
    inner: Arc<dyn ObjectStore>,
    at: Pause,
    paused: AtomicU8,
    entered: Notify,
    resume: Notify,
}
impl PausedStore {
    fn new(inner: Arc<dyn ObjectStore>, at: Pause) -> Arc<Self> {
        Arc::new(Self {
            inner,
            at,
            paused: AtomicU8::new(0),
            entered: Notify::new(),
            resume: Notify::new(),
        })
    }
    async fn wait(&self) {
        self.entered.notify_one();
        self.resume.notified().await;
    }
    async fn reached(&self) {
        tokio::time::timeout(std::time::Duration::from_secs(15), self.entered.notified())
            .await
            .expect("publisher must reach the selected interleaving");
    }
}
#[async_trait]
impl ObjectStore for PausedStore {
    async fn put(&self, key: &str, value: Vec<u8>) -> PortResult<()> {
        self.inner.put(key, value).await
    }
    async fn get(&self, key: &str) -> PortResult<Option<Vec<u8>>> {
        self.inner.get(key).await
    }
    async fn list(&self, prefix: &str) -> PortResult<Vec<ObjectEntry>> {
        self.inner.list(prefix).await
    }
    async fn delete(&self, key: &str) -> PortResult<()> {
        self.inner.delete(key).await
    }
    async fn create(&self, key: &str, value: Vec<u8>) -> PortResult<bool> {
        let matches = match self.at {
            Pause::BeforeArtifact | Pause::AfterArtifact => key.contains("/content/"),
            Pause::BeforeCommit | Pause::AfterCommit => key.ends_with("/claim.json"),
        };
        let pause = matches && self.paused.swap(1, Ordering::SeqCst) == 0;
        if pause && matches!(self.at, Pause::BeforeArtifact | Pause::BeforeCommit) {
            self.wait().await;
        }
        let result = self.inner.create(key, value).await;
        if pause && matches!(self.at, Pause::AfterArtifact | Pause::AfterCommit) {
            self.wait().await;
        }
        result
    }
}

async fn content(store: &Arc<dyn ObjectStore>, id: &str) -> Vec<ObjectEntry> {
    use sha2::{Digest, Sha256};
    store
        .list(&format!(
            "evaluations/{}/content/",
            hex::encode(Sha256::digest(id))
        ))
        .await
        .unwrap()
}

pub(super) async fn contract(store: Arc<dyn ObjectStore>) {
    let source = Arc::new(Source::default());
    let collector = registry(store.clone(), source.clone());
    let expired = 100 + PUBLICATION_GRACE_SECONDS;

    // A process stops after one shard: it never advertises a partial upload.
    let paused = PausedStore::new(store.clone(), Pause::AfterArtifact);
    let publisher = registry(paused.clone(), source.clone());
    let task =
        tokio::spawn(
            async move { publish(&publisher, request("gc-crash", 3), "editor", 100).await },
        );
    paused.reached().await;
    assert_eq!(collector.collect_orphans(expired - 1).await.unwrap(), 0);
    assert!(
        collector
            .get("gc-crash", "viewer", 100)
            .await
            .unwrap()
            .is_none()
    );
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(collector.collect_orphans(expired).await.unwrap(), 1);
    assert!(content(&store, "gc-crash").await.is_empty());
    assert!(matches!(
        collector.get("gc-crash", "viewer", expired).await,
        Err(EvaluationError::Unavailable(EvidenceState::Expired))
    ));
    assert!(collector.known_ids().await.unwrap().contains("gc-crash"));
    assert!(
        collector
            .list(None, 200, "viewer", expired)
            .await
            .unwrap()
            .evaluations
            .is_empty()
    );
    assert!(matches!(
        publish(&collector, request("gc-crash", 3), "editor", expired).await,
        Err(EvaluationError::Unavailable(EvidenceState::Expired))
    ));

    // Collection wins exactly where the publisher is about to claim its ID.
    let paused = PausedStore::new(store.clone(), Pause::BeforeCommit);
    let publisher = registry(paused.clone(), source.clone());
    let task =
        tokio::spawn(
            async move { publish(&publisher, request("gc-first", 3), "editor", 100).await },
        );
    paused.reached().await;
    assert!(collector.collect_orphans(expired).await.unwrap() > 0);
    paused.resume.notify_one();
    assert!(matches!(
        task.await.unwrap(),
        Err(EvaluationError::Unavailable(EvidenceState::Expired))
    ));
    assert!(content(&store, "gc-first").await.is_empty());

    // The opposite ordering preserves every byte, even before HTTP returns.
    let paused = PausedStore::new(store.clone(), Pause::AfterCommit);
    let publisher = registry(paused.clone(), source.clone());
    let task = tokio::spawn(async move {
        publish(&publisher, request("commit-first", 3), "editor", 100).await
    });
    paused.reached().await;
    assert_eq!(collector.collect_orphans(expired).await.unwrap(), 0);
    let detail = collector
        .get("commit-first", "viewer", expired)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(detail.state, EvidenceState::Complete);
    paused.resume.notify_one();
    let receipt = task.await.unwrap().unwrap();
    let restarted = registry(store.clone(), source.clone());
    assert_eq!(
        publish(&restarted, request("commit-first", 3), "editor", expired)
            .await
            .unwrap(),
        receipt
    );

    // A delayed object write can finish after the first sweep. The immutable
    // abandoned claim still prevents commit; the next sweep removes late bytes.
    let paused = PausedStore::new(store.clone(), Pause::BeforeArtifact);
    let publisher = registry(paused.clone(), source.clone());
    let task = tokio::spawn(async move {
        publish(&publisher, request("gc-late-write", 3), "editor", 100).await
    });
    paused.reached().await;
    collector.collect_orphans(expired).await.unwrap();
    paused.resume.notify_one();
    assert!(matches!(
        task.await.unwrap(),
        Err(EvaluationError::Unavailable(EvidenceState::Expired))
    ));
    assert!(!content(&store, "gc-late-write").await.is_empty());
    assert!(restarted.collect_orphans(expired).await.unwrap() > 0);
    assert!(content(&store, "gc-late-write").await.is_empty());

    // Losing versions can share expectation/response shards with the winner.
    // Collection protects the winner's full reference set, not just metadata.
    let winner = publish(&collector, request("gc-conflict", 3), "editor", 100)
        .await
        .unwrap();
    let before: Vec<_> = content(&store, "gc-conflict")
        .await
        .into_iter()
        .map(|e| e.key)
        .collect();
    let paused = PausedStore::new(store.clone(), Pause::BeforeCommit);
    let publisher = registry(paused.clone(), source.clone());
    let mut losing = request("gc-conflict", 3);
    losing.cases[0].actual = Some(serde_json::json!({"answer": "different"}));
    let task = tokio::spawn(async move { publish(&publisher, losing, "editor", 100).await });
    paused.reached().await;
    assert_eq!(collector.collect_orphans(100).await.unwrap(), 2);
    paused.resume.notify_one();
    assert!(matches!(
        task.await.unwrap(),
        Err(EvaluationError::Conflict)
    ));
    let after: Vec<_> = content(&store, "gc-conflict")
        .await
        .into_iter()
        .map(|e| e.key)
        .collect();
    assert_eq!(before, after);
    assert_eq!(
        collector
            .cases("gc-conflict", &winner.version, None, None, "viewer", 100)
            .await
            .unwrap()
            .unwrap()
            .cases
            .len(),
        3
    );

    // An ambiguous collection response is recovered from the same atomic gate.
    let paused = PausedStore::new(store.clone(), Pause::BeforeCommit);
    let publisher = registry(paused.clone(), source.clone());
    let task = tokio::spawn(async move {
        publish(&publisher, request("gc-lost-response", 3), "editor", 100).await
    });
    paused.reached().await;
    let lossy = registry(
        Arc::new(LostResponse {
            inner: store.clone(),
            lost: AtomicU8::new(0),
        }),
        source,
    );
    assert!(lossy.collect_orphans(expired).await.is_err());
    collector.collect_orphans(expired).await.unwrap();
    paused.resume.notify_one();
    assert!(matches!(
        task.await.unwrap(),
        Err(EvaluationError::Unavailable(EvidenceState::Expired))
    ));
    assert!(content(&store, "gc-lost-response").await.is_empty());
    assert_eq!(collector.collect_orphans(expired).await.unwrap(), 0);
}

#[tokio::test]
async fn memory_collection_and_commit_have_one_atomic_winner() {
    contract(Arc::new(MemoryObjectStore::new())).await;
}

#[tokio::test]
async fn file_collection_and_commit_have_one_atomic_winner() {
    let path = std::env::temp_dir().join(format!("aiwatcher-b2-gc-{}", std::process::id()));
    let _ = tokio::fs::remove_dir_all(&path).await;
    contract(Arc::new(FileObjectStore::open(&path).await.unwrap())).await;
    tokio::fs::remove_dir_all(path).await.unwrap();
}

#[tokio::test]
async fn collection_preserves_old_receipts_and_cannot_guess_references_from_missing_metadata() {
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryObjectStore::new());
    let registry = registry(store.clone(), Arc::new(Source::default()));
    let receipt = publish(&registry, request("old", 3), "editor", 100)
        .await
        .unwrap();
    let pending = store
        .list("evaluations/")
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.key.ends_with("/pending.json"))
        .unwrap();
    store.delete(&pending.key).await.unwrap();
    assert_eq!(registry.collect_orphans(10000).await.unwrap(), 0);
    assert_eq!(
        registry
            .get("old", "viewer", 10000)
            .await
            .unwrap()
            .unwrap()
            .receipt,
        receipt
    );
    let metadata = content(&store, "old")
        .await
        .into_iter()
        .find(|e| e.key.contains(&receipt.version))
        .unwrap();
    store.delete(&metadata.key).await.unwrap();
    assert_eq!(registry.collect_orphans(10000).await.unwrap(), 0);
    assert_eq!(content(&store, "old").await.len(), 2);
    assert_eq!(
        registry
            .get("old", "viewer", 10000)
            .await
            .unwrap()
            .unwrap()
            .state,
        EvidenceState::MissingArtifact
    );
}
