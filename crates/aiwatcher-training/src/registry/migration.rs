//! What this registry holds, and where each object goes in a bound scope.
//!
//! Run ids and model names are digested into their key segments here, which is
//! the reason this adapter lives in this module and not in a copier: nothing
//! outside can turn `house-import-17` into the directory its record sits in,
//! and a tool that tried would be a second implementation of [`Registry::id`].
//!
//! The ordering it reports is this registry's own. A model version is content
//! and its head is the index that names it; a run record is the thing, and the
//! summary beside it is derived from it and written after.

use std::collections::BTreeMap;

use aiwatcher_core::migration::{
    ContentKind, Inventory, InventoryError, InventoryObject, Reference, ReferenceKind, WriteOrder,
    key_is_safe, sha256_hex,
};
use aiwatcher_iam::ProjectScope;

use super::Registry;
use crate::model::{ModelHead, ModelVersion};
use crate::run::{TrainingRun, TrainingRunSummary, is_reproducible};

/// Who owns a `project@export-sha256`, and who owns a `workflow_run_id`.
const ANNOTATIONS_OWNER: &str = "aiwatcher-annotations";
const LOG_OWNER: &str = "the event log";

/// The four documents this registry writes, and nothing else.
enum Document {
    Run,
    RunSummary,
    ModelHead,
    ModelVersion,
}

impl Registry {
    /// Every training object under this registry's prefix, and the key each
    /// one takes in `target`.
    ///
    /// Read-only: it lists, reads and hashes, and writes nothing anywhere.
    ///
    /// # Errors
    ///
    /// [`InventoryError::Damaged`] when an object under this prefix is not a
    /// training document, and [`InventoryError::Refused`] when the source is
    /// already bound to a project.
    pub async fn inventory(&self, target: ProjectScope) -> Result<Inventory, InventoryError> {
        if self.scope.is_some() {
            return Err(InventoryError::Refused(
                "a migration source must be the unscoped training registry".to_owned(),
            ));
        }
        let destination = self
            .for_project(target)
            .map_err(|error| InventoryError::Refused(error.to_string()))?;
        let source_prefix = format!("{}/", self.prefix);
        let target_prefix = format!("{}/", destination.prefix);
        let scopes = format!("{source_prefix}scopes/");
        let mut objects = Vec::new();
        let mut entries = self.store.list(&source_prefix).await?;
        entries.sort_by(|a, b| a.key.cmp(&b.key));
        for entry in entries {
            let Some(rest) = entry.key.strip_prefix(&source_prefix) else {
                return Err(InventoryError::Refused(format!(
                    "the store returned {} outside the requested prefix",
                    entry.key
                )));
            };
            if entry.key.starts_with(&scopes) {
                continue;
            }
            let Some(document) = classify(rest) else {
                continue;
            };
            objects.push(
                self.describe(&entry.key, rest, &target_prefix, &document)
                    .await?,
            );
        }
        Ok(Inventory {
            source_prefix,
            target_prefix,
            objects,
            skipped: BTreeMap::from([(scopes, "already bound to a project scope".to_owned())]),
        })
    }

    async fn describe(
        &self,
        key: &str,
        rest: &str,
        target_prefix: &str,
        document: &Document,
    ) -> Result<InventoryObject, InventoryError> {
        let body = self
            .store
            .get(key)
            .await?
            .ok_or_else(|| InventoryError::Damaged {
                key: key.to_owned(),
                message: "listed by the store and then gone".to_owned(),
            })?;
        let references = match document {
            Document::Run => self.run_references(&body),
            Document::RunSummary => self.run_summary_references(&body),
            Document::ModelHead => self.model_head_references(&body),
            Document::ModelVersion => self.model_version_references(&body),
        }
        .map_err(|message| InventoryError::Damaged {
            key: key.to_owned(),
            message,
        })?;
        let target_key = format!("{target_prefix}{rest}");
        if !key_is_safe(&target_key) {
            return Err(InventoryError::Refused(format!(
                "{key} would map to an unusable project key"
            )));
        }
        Ok(InventoryObject {
            category: match document {
                Document::Run => "runs",
                Document::RunSummary => "run-summaries",
                Document::ModelHead => "model-heads",
                Document::ModelVersion => "model-versions",
            }
            .to_owned(),
            source_key: key.to_owned(),
            target_key,
            order: match document {
                Document::ModelVersion => WriteOrder::Content,
                Document::Run => WriteOrder::Record,
                // Both are derived from something else in this same registry
                // and both are what a list reads, so both go last.
                Document::RunSummary | Document::ModelHead => WriteOrder::Index,
            },
            content: ContentKind::Json,
            bytes: body.len() as u64,
            sha256: sha256_hex(&body),
            references,
        })
    }

    fn run_references(&self, body: &[u8]) -> Result<BTreeMap<String, Reference>, String> {
        let run: TrainingRun = serde_json::from_slice(body).map_err(|e| e.to_string())?;
        let mut out = BTreeMap::from([("/dataset".to_owned(), dataset_reference(&run.dataset))]);
        // The model is what the run trains, not something it points at: a run
        // that never produced a version names a head that was never written,
        // and reporting that as a dangling reference would be reporting the
        // ordinary case as a fault.
        out.insert(
            "/model".to_owned(),
            Reference {
                value: run.model.clone(),
                kind: ReferenceKind::Opaque {
                    reason: "the model being trained; a version is what the run produces"
                        .to_owned(),
                },
            },
        );
        if let Some(workflow) = &run.workflow_run_id {
            out.insert(
                "/workflow_run_id".to_owned(),
                Reference {
                    value: workflow.clone(),
                    kind: ReferenceKind::Foreign {
                        owner: LOG_OWNER.to_owned(),
                    },
                },
            );
        }
        for (index, checkpoint) in run.checkpoints.iter().enumerate() {
            out.insert(
                format!("/checkpoints/{index}/uri"),
                artifact_reference(&checkpoint.uri),
            );
        }
        for (index, profile) in run.profiles.iter().enumerate() {
            if let Some(uri) = &profile.uri {
                out.insert(format!("/profiles/{index}/uri"), artifact_reference(uri));
            }
        }
        Ok(out)
    }

    fn run_summary_references(&self, body: &[u8]) -> Result<BTreeMap<String, Reference>, String> {
        let summary: TrainingRunSummary =
            serde_json::from_slice(body).map_err(|e| e.to_string())?;
        Ok(BTreeMap::from([
            (
                "/run_id".to_owned(),
                Reference {
                    value: summary.run_id.clone(),
                    kind: ReferenceKind::Internal {
                        source_key: self.run_key(&summary.run_id),
                    },
                },
            ),
            ("/dataset".to_owned(), dataset_reference(&summary.dataset)),
        ]))
    }

    fn model_head_references(&self, body: &[u8]) -> Result<BTreeMap<String, Reference>, String> {
        let head: ModelHead = serde_json::from_slice(body).map_err(|e| e.to_string())?;
        let mut out = BTreeMap::new();
        for (label, version) in &head.labels {
            out.insert(
                format!("/labels/{label}"),
                Reference {
                    value: version.clone(),
                    kind: ReferenceKind::Internal {
                        source_key: self.model_version_key(&head.name, version),
                    },
                },
            );
        }
        for (index, summary) in head.versions.iter().enumerate() {
            out.insert(
                format!("/versions/{index}/version"),
                Reference {
                    value: summary.version.clone(),
                    kind: ReferenceKind::Internal {
                        source_key: self.model_version_key(&head.name, &summary.version),
                    },
                },
            );
        }
        Ok(out)
    }

    fn model_version_references(&self, body: &[u8]) -> Result<BTreeMap<String, Reference>, String> {
        let version: ModelVersion = serde_json::from_slice(body).map_err(|e| e.to_string())?;
        let mut out = BTreeMap::from([
            (
                // Registration reads the run from this same registry, so this
                // one really does have to be here.
                "/run_id".to_owned(),
                Reference {
                    value: version.run_id.clone(),
                    kind: ReferenceKind::Internal {
                        source_key: self.run_key(&version.run_id),
                    },
                },
            ),
            ("/dataset".to_owned(), dataset_reference(&version.dataset)),
            (
                "/checkpoint_uri".to_owned(),
                artifact_reference(&version.checkpoint_uri),
            ),
        ]);
        if let Some(package) = &version.package {
            for (index, artifact) in package.artifacts.iter().enumerate() {
                out.insert(
                    format!("/package/artifacts/{index}/uri"),
                    artifact_reference(&artifact.uri),
                );
            }
        }
        Ok(out)
    }
}

/// `project@export-sha256` names an annotation export; a bare name names
/// nothing anybody can reconstruct, which is the registry's own rule for
/// whether a run is reproducible.
fn dataset_reference(dataset: &str) -> Reference {
    Reference {
        value: dataset.to_owned(),
        kind: if is_reproducible(dataset) {
            ReferenceKind::Foreign {
                owner: ANNOTATIONS_OWNER.to_owned(),
            }
        } else {
            ReferenceKind::Opaque {
                reason: "a mutable dataset name; the run is recorded as irreproducible".to_owned(),
            }
        },
    }
}

/// Weights and traces are pointers. Nothing here stores a copy, so nothing
/// here can move one.
fn artifact_reference(uri: &str) -> Reference {
    Reference {
        value: uri.to_owned(),
        kind: ReferenceKind::Opaque {
            reason: "bytes stored outside this registry; the migration moves no artifact"
                .to_owned(),
        },
    }
}

/// Which of the four documents a relative key is, if it is one at all.
fn classify(rest: &str) -> Option<Document> {
    match rest.split('/').collect::<Vec<_>>().as_slice() {
        ["runs", _, "record.json"] => Some(Document::Run),
        ["runs", _, "summary.json"] => Some(Document::RunSummary),
        ["models", _, "head.json"] => Some(Document::ModelHead),
        ["models", _, "versions", file] if file.ends_with(".json") => Some(Document::ModelVersion),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use aiwatcher_core::prompts::ObjectStore as _;
    use aiwatcher_iam::{OrganizationId, ProjectId};
    use aiwatcher_prompts::adapters::memory::MemoryObjectStore;
    use serde_json::json;
    use std::sync::Arc;

    fn scope() -> ProjectScope {
        ProjectScope {
            organization: OrganizationId::new(),
            project: ProjectId::new(),
        }
    }

    async fn seeded(dataset: &str) -> (Arc<MemoryObjectStore>, Registry) {
        let store = Arc::new(MemoryObjectStore::new());
        let registry = Registry::new(store.clone(), "training");
        registry
            .start(
                serde_json::from_value(json!({
                    "run_id": "run-1", "model": "walls", "dataset": dataset,
                    "workflow_run_id": "wf-1"
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
                    "checkpoint_uri": "s3://checkpoints/walls.pt",
                    "metrics": {"validation": {"iou": 0.8}, "test": {"iou": 0.79}}
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        (store, registry)
    }

    #[tokio::test]
    async fn a_model_version_names_the_run_it_came_from_by_the_key_that_run_really_has() {
        let (store, registry) = seeded("plans@sha256abc").await;
        let before = store.list("").await.unwrap();
        let inventory = registry.inventory(scope()).await.unwrap();
        assert_eq!(store.list("").await.unwrap(), before, "reads only");

        let version = inventory
            .objects
            .iter()
            .find(|object| object.category == "model-versions")
            .expect("a model version");
        assert_eq!(version.order, WriteOrder::Content);
        let ReferenceKind::Internal { source_key } = &version.references["/run_id"].kind else {
            panic!("a version names its run inside this registry");
        };
        assert!(
            inventory
                .objects
                .iter()
                .any(|object| &object.source_key == source_key && object.category == "runs"),
            "and the run's digested key is the one the registry really wrote"
        );
        assert!(matches!(
            version.references["/checkpoint_uri"].kind,
            ReferenceKind::Opaque { .. }
        ));
        assert!(matches!(
            version.references["/dataset"].kind,
            ReferenceKind::Foreign { .. }
        ));

        let run = inventory
            .objects
            .iter()
            .find(|object| object.category == "runs")
            .expect("a run record");
        assert_eq!(run.order, WriteOrder::Record);
        assert!(matches!(
            run.references["/workflow_run_id"].kind,
            ReferenceKind::Foreign { .. }
        ));
        assert!(
            matches!(run.references["/model"].kind, ReferenceKind::Opaque { .. }),
            "a run that never produced a version names no model head"
        );
        let head = inventory
            .objects
            .iter()
            .find(|object| object.category == "model-heads")
            .expect("a model head");
        assert_eq!(head.order, WriteOrder::Index);
    }

    #[tokio::test]
    async fn a_mutable_dataset_name_is_opaque_and_an_export_reference_names_its_owner() {
        let (_, mutable) = seeded("plans").await;
        let inventory = mutable.inventory(scope()).await.unwrap();
        let run = inventory
            .objects
            .iter()
            .find(|object| object.category == "runs")
            .expect("a run");
        assert!(
            matches!(
                run.references["/dataset"].kind,
                ReferenceKind::Opaque { .. }
            ),
            "a bare name reconstructs nothing, which is the registry's own rule"
        );
    }

    #[tokio::test]
    async fn a_record_of_another_shape_is_damaged_and_a_scoped_registry_is_no_source() {
        let store = Arc::new(MemoryObjectStore::new());
        let registry = Registry::new(store.clone(), "training");
        store
            .put("training/runs/abcd/record.json", b"{}".to_vec())
            .await
            .unwrap();
        let error = registry.inventory(scope()).await.expect_err("damaged");
        assert!(matches!(error, InventoryError::Damaged { .. }), "{error}");

        let target = scope();
        let scoped = Registry::new(Arc::new(MemoryObjectStore::new()), "training")
            .for_project(target)
            .unwrap();
        assert!(scoped.inventory(target).await.is_err());
    }
}
