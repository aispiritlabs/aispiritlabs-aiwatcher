//! Shared approved bundle and native Curation rows for source adapter tests.
use super::*;
use aiwatcher_datasets::{PublishDatasetRequest, Registry as Datasets};
use aiwatcher_server::evaluation::LocalSource;
use serde_json::{Value, json};
use std::path::PathBuf;

pub(super) struct Fixture {
    pub(super) root: PathBuf,
    pub(super) store: Arc<dyn ObjectStore>,
    pub(super) datasets: Arc<Datasets>,
    pub(super) request: PublishEvaluation,
    pub(super) rows: PublishDatasetRequest,
}
impl Fixture {
    pub(super) async fn new(label: &str) -> Self {
        let original = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../contracts/fixtures/evaluation-v1");
        let root =
            std::env::temp_dir().join(format!("aiwatcher-curation-{label}-{}", std::process::id()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        for entry in std::fs::read_dir(original).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_file() {
                tokio::fs::copy(entry.path(), root.join(entry.file_name()))
                    .await
                    .unwrap();
            }
        }
        let store: Arc<dyn ObjectStore> = Arc::new(MemoryObjectStore::new());
        let datasets = Arc::new(Datasets::new(store.clone(), "datasets"));
        let cases: Value =
            serde_json::from_slice(&tokio::fs::read(root.join("cases.json")).await.unwrap())
                .unwrap();
        let rows: PublishDatasetRequest = serde_json::from_value(json!({
            "name": "native-answers", "pipeline": "fixture rows", "source": "synthetic",
            "columns": ["case_id", "input", "expected"], "items": cases["cases"]
        }))
        .unwrap();
        let version = datasets
            .publish(rows.clone())
            .await
            .unwrap()
            .dataset
            .latest
            .version;
        let mut request = request(label, 3);
        request.manifest.context.dataset = DatasetReference {
            kind: DatasetKind::Curation,
            name: rows.name.clone(),
            version,
        };
        request.manifest.variant.dataset = request.manifest.context.dataset.clone();
        for (measurement, case) in request
            .cases
            .iter_mut()
            .zip(cases["cases"].as_array().unwrap())
        {
            measurement.case_id = case["case_id"].as_str().unwrap().into();
            measurement.actual = Some(case["expected"].clone());
        }
        let result = Self {
            root,
            store,
            datasets,
            request,
            rows,
        };
        result.approve(&result.request.manifest).await;
        result
    }
    pub(super) async fn approve(&self, manifest: &EvaluationManifest) {
        tokio::fs::write(
            self.root.join("manifest.json"),
            serde_json::to_vec(manifest).unwrap(),
        )
        .await
        .unwrap();
    }
    pub(super) fn source(&self) -> LocalSource {
        LocalSource::new(Some(self.root.to_str().unwrap().into()))
            .with_curation(self.datasets.clone())
    }
    pub(super) fn registry(&self) -> Registry {
        Registry::new(
            self.store.clone(),
            Arc::new(self.source()),
            RegistryConfig::default(),
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
