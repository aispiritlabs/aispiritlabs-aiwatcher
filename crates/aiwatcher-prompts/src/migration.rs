//! What this registry holds, and where each object goes in a bound scope.
//!
//! The key layout is this crate's (`{prefix}/{name}/head.json`,
//! `versions/{id}.json`, `optimizations/{id}.json`) and so is the ordering it
//! is written in: the version object before the head that indexes it. Both
//! stay here. A migration tool asks; it does not re-derive.
//!
//! Two things this adapter refuses to do. It does not walk heads to find
//! versions — it classifies every key under the prefix, so a version whose
//! head was never written is inventoried rather than lost, and a key it does
//! not recognise is visibly neither migrated nor skipped. And it resolves no
//! label, dataset name or evaluation id against anything: a reference is
//! recorded as it was stored, with the owner's word for what kind of thing it
//! is.

use std::collections::BTreeMap;

use aiwatcher_core::migration::{
    ContentKind, Inventory, InventoryError, InventoryObject, Reference, ReferenceKind, WriteOrder,
    key_is_safe, sha256_hex,
};
use aiwatcher_core::prompts::{
    OptimizationRecord, PromptHead, PromptName, PromptVersion, PromptVersionId,
};
use aiwatcher_iam::ProjectScope;

use crate::Registry;

/// Whoever a reader of an `evaluation_id` would have to ask instead.
const EVALUATION_OWNER: &str = "the event log and aiwatcher-evaluation";

/// The three documents this registry writes, and nothing else.
enum Document {
    Head,
    Version,
    Optimization,
}

impl Registry {
    /// Every prompt object under this registry's prefix, and the key each one
    /// takes in `target`.
    ///
    /// Read-only: it lists, reads and hashes, and writes nothing anywhere.
    ///
    /// # Errors
    ///
    /// [`InventoryError::Damaged`] when an object under this prefix is not a
    /// prompt document — a migration must stop rather than copy bytes nobody
    /// can read into a project. [`InventoryError::Refused`] when called on a
    /// registry already bound to a project: a scope is a destination here,
    /// never a source.
    pub async fn inventory(&self, target: ProjectScope) -> Result<Inventory, InventoryError> {
        if self.scope.is_some() {
            return Err(InventoryError::Refused(
                "a migration source must be the unscoped prompt registry".to_owned(),
            ));
        }
        let destination = self
            .for_project(target)
            .map_err(|error| InventoryError::Refused(error.to_string()))?;
        let source_prefix = format!("{}/", self.config.prefix);
        let target_prefix = format!("{}/", destination.config.prefix);
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
            // Somebody's project already. Never a source: re-scoping a scoped
            // object is how one project's prompts reach another's.
            if entry.key.starts_with(&scopes) {
                continue;
            }
            let Some((name, document)) = classify(rest) else {
                continue;
            };
            objects.push(
                self.describe(&entry.key, rest, &target_prefix, &name, &document)
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
        name: &PromptName,
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
            Document::Head => self.head_references(&body, name),
            Document::Version => self.version_references(&body, name),
            Document::Optimization => self.optimization_references(&body, name),
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
                Document::Head => "heads",
                Document::Version => "versions",
                Document::Optimization => "optimizations",
            }
            .to_owned(),
            source_key: key.to_owned(),
            target_key,
            order: match document {
                // The head is the index. It goes last, so an interrupted copy
                // leaves an unindexed version rather than a row that 404s.
                Document::Head => WriteOrder::Index,
                Document::Version | Document::Optimization => WriteOrder::Content,
            },
            content: ContentKind::Json,
            bytes: body.len() as u64,
            sha256: sha256_hex(&body),
            references,
        })
    }

    fn head_references(
        &self,
        body: &[u8],
        name: &PromptName,
    ) -> Result<BTreeMap<String, Reference>, String> {
        let head: PromptHead = serde_json::from_slice(body).map_err(|e| e.to_string())?;
        let mut out = BTreeMap::new();
        for (label, version) in &head.labels {
            // A label lives nowhere but the head. It is the one fact a rebuild
            // cannot recover, which is why it is named here rather than left
            // for the version objects to imply.
            out.insert(
                format!("/labels/{label}"),
                self.version_reference(name, version),
            );
        }
        for (index, summary) in head.versions.iter().enumerate() {
            out.insert(
                format!("/versions/{index}/version_id"),
                self.version_reference(name, &summary.version_id),
            );
        }
        for (index, summary) in head.optimizations.iter().enumerate() {
            out.insert(
                format!("/optimizations/{index}/optimization_id"),
                Reference {
                    value: summary.optimization_id.clone(),
                    kind: ReferenceKind::Internal {
                        source_key: self.optimization_key(name, &summary.optimization_id),
                    },
                },
            );
        }
        Ok(out)
    }

    fn version_references(
        &self,
        body: &[u8],
        name: &PromptName,
    ) -> Result<BTreeMap<String, Reference>, String> {
        let version: PromptVersion = serde_json::from_slice(body).map_err(|e| e.to_string())?;
        let mut out = BTreeMap::new();
        if let Some(parent) = &version.parent {
            out.insert("/parent".to_owned(), self.version_reference(name, parent));
        }
        Ok(out)
    }

    fn optimization_references(
        &self,
        body: &[u8],
        name: &PromptName,
    ) -> Result<BTreeMap<String, Reference>, String> {
        let record: OptimizationRecord = serde_json::from_slice(body).map_err(|e| e.to_string())?;
        let mut out = BTreeMap::from([
            (
                "/baseline".to_owned(),
                self.version_reference(name, &record.baseline),
            ),
            (
                "/candidate".to_owned(),
                self.version_reference(name, &record.candidate),
            ),
        ]);
        // Neither of the next two is resolved, for the reason that keeps the
        // verdict computed here: the dataset name is the optimiser's word for
        // what it measured on, and an evaluation id points into a log this
        // registry deliberately outlives.
        if let Some(dataset) = &record.dataset {
            out.insert(
                "/dataset".to_owned(),
                Reference {
                    value: dataset.clone(),
                    kind: ReferenceKind::Opaque {
                        reason: "the optimiser's name for the cases it measured on".to_owned(),
                    },
                },
            );
        }
        for (pointer, value) in [
            ("/evaluation_id", &record.evaluation_id),
            ("/baseline_evaluation", &record.baseline_evaluation),
            ("/candidate_evaluation", &record.candidate_evaluation),
        ] {
            if let Some(value) = value {
                out.insert(
                    pointer.to_owned(),
                    Reference {
                        value: value.clone(),
                        kind: ReferenceKind::Foreign {
                            owner: EVALUATION_OWNER.to_owned(),
                        },
                    },
                );
            }
        }
        Ok(out)
    }

    fn version_reference(&self, name: &PromptName, version: &PromptVersionId) -> Reference {
        Reference {
            value: version.as_str().to_owned(),
            kind: ReferenceKind::Internal {
                source_key: self.version_key(name, version),
            },
        }
    }
}

/// Which of the three documents a relative key is, if it is one at all.
fn classify(rest: &str) -> Option<(PromptName, Document)> {
    let segments: Vec<&str> = rest.split('/').collect();
    let name = PromptName::parse(*segments.first()?).ok()?;
    match segments.as_slice() {
        [_, "head.json"] => Some((name, Document::Head)),
        [_, "versions", file] if file.ends_with(".json") => Some((name, Document::Version)),
        [_, "optimizations", file] if file.ends_with(".json") => {
            Some((name, Document::Optimization))
        }
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::RegistryConfig;
    use crate::adapters::memory::MemoryObjectStore;
    use aiwatcher_core::migration::ContentKind;
    use aiwatcher_core::prompts::ObjectStore as _;
    use aiwatcher_iam::{OrganizationId, ProjectId};
    use serde_json::json;
    use std::sync::Arc;

    fn scope() -> ProjectScope {
        ProjectScope {
            organization: OrganizationId::new(),
            project: ProjectId::new(),
        }
    }

    async fn seeded() -> (Arc<MemoryObjectStore>, Registry) {
        let store = Arc::new(MemoryObjectStore::new());
        let registry = Registry::new(store.clone(), RegistryConfig::default());
        let first = registry
            .publish(
                serde_json::from_value(json!({
                    "name": "house.extract", "text": "Read {{ page }}.", "label": "production"
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
                    "candidate_text": "Read {{ page }} briefly.",
                    "primary_metric": "quality",
                    "test": [{"metric": "quality", "baseline": 0.5, "candidate": 0.7}],
                    "evaluation_id": "report-1"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        (store, registry)
    }

    #[tokio::test]
    async fn an_inventory_names_the_version_a_label_points_at_and_writes_nothing() {
        let (store, registry) = seeded().await;
        let before = store.list("").await.unwrap();
        let target = scope();
        let inventory = registry.inventory(target).await.unwrap();
        assert_eq!(store.list("").await.unwrap(), before, "reads only");

        let head = inventory
            .objects
            .iter()
            .find(|object| object.category == "heads")
            .expect("a head");
        assert_eq!(head.order, WriteOrder::Index, "the index goes last");
        assert_eq!(head.content, ContentKind::Json);
        let label = head
            .references
            .get("/labels/production")
            .expect("a label lives only in the head");
        let ReferenceKind::Internal { source_key } = &label.kind else {
            panic!("a label names a version in this same registry: {label:?}");
        };
        assert!(
            inventory
                .objects
                .iter()
                .any(|object| &object.source_key == source_key),
            "and that version is inventoried"
        );

        let optimization = inventory
            .objects
            .iter()
            .find(|object| object.category == "optimizations")
            .expect("an optimisation");
        assert_eq!(optimization.order, WriteOrder::Content);
        assert!(matches!(
            optimization.references["/evaluation_id"].kind,
            ReferenceKind::Foreign { .. }
        ));
        assert!(matches!(
            optimization.references["/baseline"].kind,
            ReferenceKind::Internal { .. }
        ));

        for object in &inventory.objects {
            assert!(object.target_key.starts_with(&inventory.target_prefix));
            assert!(
                object.target_key.ends_with(
                    object
                        .source_key
                        .strip_prefix(&inventory.source_prefix)
                        .unwrap()
                ),
                "scope goes in front of the key and changes nothing else"
            );
        }
    }

    #[tokio::test]
    async fn a_scoped_registry_is_never_a_source_and_its_keys_are_never_inventoried() {
        let (store, registry) = seeded().await;
        let target = scope();
        let first = registry.inventory(target).await.unwrap();
        let scoped = registry.for_project(target).unwrap();
        assert!(scoped.inventory(scope()).await.is_err());

        // Copy it, then inventory again: the destination is not a source.
        for object in &first.objects {
            let bytes = store.get(&object.source_key).await.unwrap().unwrap();
            store.put(&object.target_key, bytes).await.unwrap();
        }
        let second = registry.inventory(target).await.unwrap();
        assert_eq!(first.objects, second.objects);
    }

    #[tokio::test]
    async fn a_document_that_is_not_a_prompt_is_damaged_and_a_stray_file_is_neither() {
        let store = Arc::new(MemoryObjectStore::new());
        let registry = Registry::new(store.clone(), RegistryConfig::default());
        store
            .put("prompts/house.extract/head.json", b"not json".to_vec())
            .await
            .unwrap();
        let error = registry.inventory(scope()).await.expect_err("damaged");
        assert!(matches!(error, InventoryError::Damaged { .. }), "{error}");

        let store = Arc::new(MemoryObjectStore::new());
        let registry = Registry::new(store.clone(), RegistryConfig::default());
        store
            .put("prompts/notes.txt", b"a stray file".to_vec())
            .await
            .unwrap();
        let inventory = registry.inventory(scope()).await.unwrap();
        assert!(
            inventory.objects.is_empty(),
            "a key this registry never wrote is not migrated"
        );
        assert!(
            !inventory
                .skipped
                .keys()
                .any(|prefix| "prompts/notes.txt".starts_with(prefix.as_str())),
            "and it is not declared skipped either, so the tool can refuse it"
        );
    }
}
