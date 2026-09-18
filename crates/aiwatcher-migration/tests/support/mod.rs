//! A store worth migrating, and the stores that misbehave while it is.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, dead_code)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use aiwatcher_core::ports::{PortError, PortResult};
use aiwatcher_core::prompts::{ObjectEntry, ObjectStore};
use aiwatcher_iam::{OrganizationId, ProjectId, ProjectScope};
use aiwatcher_prompts::adapters::memory::MemoryObjectStore;
use async_trait::async_trait;
use serde_json::json;

pub fn scope() -> ProjectScope {
    ProjectScope {
        organization: OrganizationId::new(),
        project: ProjectId::new(),
    }
}

/// One of each supported family, with the references that matter: a prompt
/// label and an optimisation's baseline, a model version naming its run, a
/// dataset version naming its recipe, an image head naming its blob.
pub async fn seeded() -> Arc<MemoryObjectStore> {
    let store = Arc::new(MemoryObjectStore::new());
    let object: Arc<dyn ObjectStore> = store.clone();
    seed_prompts(&object).await;
    seed_datasets(&object).await;
    seed_training(&object).await;
    seed_annotations(&object).await;
    store
}

async fn seed_prompts(store: &Arc<dyn ObjectStore>) {
    let registry = aiwatcher_prompts::Registry::new(
        store.clone(),
        aiwatcher_prompts::RegistryConfig::default(),
    );
    let first = registry
        .publish(
            serde_json::from_value(json!({
                "name": "house.extract", "text": "Read {{ page }} carefully.",
                "label": "production", "author": "a person"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    registry
        .record_optimization(
            &first.version.name,
            serde_json::from_value(json!({
                "optimization_id": "opt-one", "algorithm": "test",
                "baseline": first.version.version_id.as_str(),
                "candidate_text": "Read {{ page }} and answer briefly.",
                "primary_metric": "quality",
                "test": [{"metric": "quality", "baseline": 0.5, "candidate": 0.7}],
                "dataset": "cases@v1", "evaluation_id": "report-1"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
}

async fn seed_datasets(store: &Arc<dyn ObjectStore>) {
    let registry = aiwatcher_datasets::Registry::new(store.clone(), "datasets");
    registry
        .save_recipe(
            serde_json::from_value(json!({
                "name": "saved/query", "description": "",
                "pipeline": "data_frame()->read(default)"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    // `produced_by` names a pinned pipeline revision, so the inventory has a
    // real internal reference to resolve rather than a plausible-looking one.
    let pipeline = registry
        .save_pipeline(
            serde_json::from_value(json!({
                "name": "import", "description": "",
                "blocks": [{
                    "id": "source", "title": "Runs",
                    "spec": {"kind": "source", "dataset": "runs"}
                }],
                "edges": []
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    registry
        .publish(
            serde_json::from_value(json!({
                "name": "houses", "recipe": "saved/query",
                "pipeline": "data_frame()->read(default)", "columns": ["value"],
                "items": [{"value": "a row"}], "source": "runs",
                "produced_by": format!("import@{}", pipeline.pipeline.revision),
                "execution_id": "exec-1"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
}

async fn seed_training(store: &Arc<dyn ObjectStore>) {
    let registry = aiwatcher_training::Registry::new(store.clone(), "training");
    registry
        .start(
            serde_json::from_value(json!({
                "run_id": "run-1", "model": "walls", "dataset": "plans@sha256abc",
                "framework": "torch", "code": "deadbeef"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    registry
        .finish(
            "run-1",
            serde_json::from_value(json!({"status": "succeeded"})).unwrap(),
        )
        .await
        .unwrap();
    registry
        .register_model(
            serde_json::from_value(json!({
                "name": "walls", "run_id": "run-1",
                "checkpoint_uri": "s3://checkpoints/walls-3.pt",
                "metrics": {"validation": {"iou": 0.8}, "test": {"iou": 0.79}}
            }))
            .unwrap(),
        )
        .await
        .unwrap();
}

async fn seed_annotations(store: &Arc<dyn ObjectStore>) {
    let registry = aiwatcher_annotations::Registry::new(store.clone(), "annotations");
    registry
        .save_project(
            serde_json::from_value(json!({
                "name": "plans/first", "description": "",
                "classes": [{"name": "wall", "geometry": "polyline"}]
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let blob = registry
        .put_blob(
            b"\x89PNG\r\n\x1a\nnot really a picture".to_vec(),
            "image/png",
        )
        .await
        .unwrap();
    registry
        .register_image(
            serde_json::from_value(json!({
                "project": "plans/first", "image_id": blob.image_id, "uri": blob.uri,
                "width": 100, "height": 80, "group_id": "house-1",
                "source": "a supplier", "rights": {"kind": "unknown"}
            }))
            .unwrap(),
        )
        .await
        .unwrap();
}

/// Everything in a store, for a before-and-after comparison.
pub async fn contents(store: &dyn ObjectStore) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    for entry in store.list("").await.unwrap() {
        out.insert(
            entry.key.clone(),
            store.get(&entry.key).await.unwrap().unwrap(),
        );
    }
    out
}

/// A store that fails, corrupts or refuses a write on purpose.
#[derive(Debug)]
pub struct Faulty {
    inner: Arc<MemoryObjectStore>,
    writes: AtomicUsize,
    fail_write_at: usize,
    corrupt_write_at: usize,
    refuse_create: bool,
}

impl Faulty {
    pub fn new(inner: Arc<MemoryObjectStore>) -> Self {
        Self {
            inner,
            writes: AtomicUsize::new(0),
            fail_write_at: 0,
            corrupt_write_at: 0,
            refuse_create: false,
        }
    }
    /// Refuse the nth write outright: nothing is stored.
    pub fn failing_write(mut self, nth: usize) -> Self {
        self.fail_write_at = nth;
        self
    }
    /// Report the nth write as a success and store something else.
    pub fn corrupting_write(mut self, nth: usize) -> Self {
        self.corrupt_write_at = nth;
        self
    }
    /// Answer `create` the way an adapter without the capability does.
    pub fn without_atomic_create(mut self) -> Self {
        self.refuse_create = true;
        self
    }
    pub fn writes(&self) -> usize {
        self.writes.load(Ordering::SeqCst)
    }
    fn next_write(&self) -> usize {
        self.writes.fetch_add(1, Ordering::SeqCst) + 1
    }
}

#[async_trait]
impl ObjectStore for Faulty {
    async fn put(&self, key: &str, body: Vec<u8>) -> PortResult<()> {
        match self.next_write() {
            nth if nth == self.fail_write_at => Err(PortError::Unavailable {
                target: "object-store",
                message: "the disk went away".into(),
            }),
            nth if nth == self.corrupt_write_at => {
                self.inner
                    .put(key, b"something else entirely".to_vec())
                    .await
            }
            _ => self.inner.put(key, body).await,
        }
    }
    async fn create(&self, key: &str, body: Vec<u8>) -> PortResult<bool> {
        if self.refuse_create {
            return Err(PortError::Rejected {
                target: "object-store",
                message: "atomic object creation is not supported".into(),
            });
        }
        match self.next_write() {
            nth if nth == self.fail_write_at => Err(PortError::Unavailable {
                target: "object-store",
                message: "the disk went away".into(),
            }),
            nth if nth == self.corrupt_write_at => {
                self.inner
                    .create(key, b"something else entirely".to_vec())
                    .await
            }
            _ => self.inner.create(key, body).await,
        }
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
}
