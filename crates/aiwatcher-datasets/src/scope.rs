//! Project ownership lives in storage keys, outside immutable content identities.
//! Constructing a registry does not authorize access: service adapters must check
//! a fresh IAM grant before each operation. There is no fallback to legacy data.
use crate::{Registry, RegistryError, Result, digest};
use aiwatcher_iam::ProjectScope;
use serde::Serialize;
use std::collections::BTreeMap;

impl Registry {
    /// Bind the entire registry (datasets, recipes, pipelines and library) to a
    /// typed scope. A bound registry cannot be rebound to a different project.
    pub fn for_project(&self, scope: ProjectScope) -> Result<Self> {
        if let Some(current) = self.scope {
            return if current == scope {
                Ok(self.clone())
            } else {
                Err(RegistryError::Invalid(
                    "registry is already bound to another project".into(),
                ))
            };
        }
        Ok(Self {
            store: self.store.clone(),
            prefix: format!(
                "{}/scopes/{}/{}/registry",
                self.prefix, scope.organization.0, scope.project.0
            ),
            scope: Some(scope),
        })
    }

    pub(crate) fn check_key(&self, key: &str) -> Result<()> {
        if self.scope.is_some()
            && (!key.starts_with(&format!("{}/", self.prefix))
                || key
                    .split('/')
                    .any(|part| matches!(part, "" | "." | "..") || part.contains('\\')))
        {
            return Err(RegistryError::Invalid(
                "object key escapes the project registry".into(),
            ));
        }
        Ok(())
    }

    /// Operator-only dry run. Reads bytes and inventories targets; never copies,
    /// deletes, republishes or rewrites a head. Call on the legacy root registry.
    /// This inventory is not a cutover authorization or a consistent snapshot.
    pub async fn migration_manifest(&self, target: ProjectScope) -> Result<MigrationManifest> {
        if self.scope.is_some() {
            return Err(RegistryError::Invalid(
                "migration source must be the legacy registry".into(),
            ));
        }
        let destination = self.for_project(target)?;
        let mut objects = Vec::new();
        let mut counts = BTreeMap::new();
        for category in ["collections", "recipes", "pipelines", "library"] {
            counts.insert(category.to_owned(), 0);
            let prefix = format!("{}/{category}/", self.prefix);
            let mut entries = self.store.list(&prefix).await?;
            entries.sort_by(|a, b| a.key.cmp(&b.key));
            for entry in entries {
                let relative = entry
                    .key
                    .strip_prefix(&format!("{}/", self.prefix))
                    .filter(|_| entry.key.starts_with(&prefix))
                    .ok_or_else(|| {
                        RegistryError::Invalid(
                            "store returned an object outside the requested prefix".into(),
                        )
                    })?;
                let target_key = format!("{}/{relative}", destination.prefix);
                destination.check_key(&target_key)?;
                let bytes = self
                    .store
                    .get(&entry.key)
                    .await?
                    .ok_or_else(|| RegistryError::NotFound(entry.key.clone()))?;
                let document: serde_json::Value =
                    serde_json::from_slice(&bytes).map_err(|e| RegistryError::Corrupt {
                        key: entry.key.clone(),
                        message: e.to_string(),
                    })?;
                let target_state = match self.store.get(&target_key).await? {
                    None => TargetState::Absent,
                    Some(existing) if existing == bytes => TargetState::Identical,
                    Some(_) => TargetState::Conflict,
                };
                let mut references = BTreeMap::new();
                collect_references(&document, "", &mut references);
                *counts.entry(category.to_owned()).or_insert(0) += 1;
                objects.push(MigrationObject {
                    source_key: entry.key,
                    target_key,
                    bytes: bytes.len(),
                    sha256: digest(&bytes),
                    target_state,
                    references,
                });
            }
        }
        Ok(MigrationManifest {
            schema: 1,
            target,
            source_prefix: self.prefix.clone(),
            counts,
            objects,
        })
    }
}

/// References are inventoried verbatim, never resolved against another project.
/// Query text may also contain dependencies; these require operator review.
fn collect_references(
    value: &serde_json::Value,
    path: &str,
    out: &mut BTreeMap<String, serde_json::Value>,
) {
    match value {
        serde_json::Value::Object(fields) => {
            for (key, child) in fields {
                let pointer = format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"));
                if matches!(
                    key.as_str(),
                    "recipe"
                        | "produced_by"
                        | "execution_id"
                        | "source"
                        | "dataset"
                        | "notebook"
                        | "revision"
                        | "version"
                ) && !child.is_null()
                {
                    out.insert(pointer.clone(), child.clone());
                }
                collect_references(child, &pointer, out);
            }
        }
        serde_json::Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                // Dataset rows are user data, not registry references.
                if path != "/items" {
                    collect_references(child, &format!("{path}/{index}"), out);
                }
            }
        }
        _ => (),
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TargetState {
    Absent,
    Identical,
    Conflict,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct MigrationObject {
    pub source_key: String,
    pub target_key: String,
    pub bytes: usize,
    pub sha256: String,
    pub target_state: TargetState,
    pub references: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct MigrationManifest {
    pub schema: u32,
    pub source_prefix: String,
    pub target: ProjectScope,
    /// Stored objects per category, including heads and historical revisions.
    pub counts: BTreeMap<String, usize>,
    pub objects: Vec<MigrationObject>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PublishDatasetRequest, SaveRecipeRequest};
    use aiwatcher_core::ObjectStore;
    use aiwatcher_iam::{OrganizationId, ProjectId};
    use aiwatcher_prompts::adapters::{fs::FileObjectStore, memory::MemoryObjectStore};
    use std::sync::Arc;

    fn scope() -> ProjectScope {
        ProjectScope {
            organization: OrganizationId::new(),
            project: ProjectId::new(),
        }
    }
    fn dataset() -> PublishDatasetRequest {
        serde_json::from_value(serde_json::json!({"name":"same/name","recipe":"saved/query",
            "pipeline":"data_frame()->read(default)","columns":["value"],"items":[{"value":"secret"}],
            "source":"legacy-source","produced_by":"pipeline@revision","execution_id":"old-run"})).unwrap()
    }

    #[tokio::test]
    async fn filesystem_namespaces_survive_reopen_and_do_not_change_content_hashes() {
        let dir = std::env::temp_dir().join(format!(
            "aiwatcher-scoped-datasets-{}",
            OrganizationId::new().0
        ));
        let store = Arc::new(FileObjectStore::open(&dir).await.unwrap());
        let legacy = Registry::new(store.clone(), "datasets");
        let a_scope = scope();
        let b_scope = ProjectScope {
            project: ProjectId::new(),
            ..a_scope
        };
        let c_scope = ProjectScope {
            organization: OrganizationId::new(),
            ..a_scope
        };
        let a = legacy.for_project(a_scope).unwrap();
        let b = legacy.for_project(b_scope).unwrap();
        let c = legacy.for_project(c_scope).unwrap();
        let first = a.publish(dataset()).await.unwrap();
        assert!(b.datasets().await.unwrap().datasets.is_empty());
        assert!(c.datasets().await.unwrap().datasets.is_empty());
        assert!(legacy.datasets().await.unwrap().datasets.is_empty());
        let version = first.dataset.latest.version;
        for registry in [&b, &c, &legacy] {
            assert!(
                registry
                    .verified_version("same/name", &version)
                    .await
                    .is_err()
            );
        }
        let same = b.publish(dataset()).await.unwrap();
        assert!(same.created);
        assert_eq!(same.dataset.latest.version, version);
        let historical = legacy.publish(dataset()).await.unwrap();
        assert_eq!(historical.dataset.latest.version, version);
        let reopened = Registry::new(
            Arc::new(FileObjectStore::open(&dir).await.unwrap()),
            "datasets",
        )
        .for_project(a_scope)
        .unwrap();
        let read = reopened
            .verified_version("same/name", &version)
            .await
            .unwrap();
        assert_eq!(read.items, dataset().items);
        assert_eq!(read.summary.execution_id.as_deref(), Some("old-run"));
        assert!(a.for_project(b_scope).is_err());
        assert!(a.for_project(a_scope).is_ok());
        for revision in [
            "../../../../../recipes/head",
            "..\\secret",
            "/secret",
            "x/../y",
        ] {
            assert!(
                a.pipeline("pipeline", Some(revision)).await.is_err(),
                "{revision}"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn dry_run_is_deterministic_read_only_and_detects_target_conflicts() {
        let store = Arc::new(MemoryObjectStore::new());
        let legacy = Registry::new(store.clone(), "datasets");
        let target = scope();
        let saved = legacy.publish(dataset()).await.unwrap();
        legacy
            .save_recipe(SaveRecipeRequest {
                name: "saved/query".into(),
                description: String::new(),
                pipeline: "data_frame()->read(default)".into(),
                engine: crate::QueryEngine::Flow,
            })
            .await
            .unwrap();
        let before = store.list("").await.unwrap();
        let first = legacy.migration_manifest(target).await.unwrap();
        assert_eq!(first, legacy.migration_manifest(target).await.unwrap());
        assert_eq!(store.list("").await.unwrap(), before);
        assert_eq!(first.counts["collections"], 2);
        assert_eq!(first.counts["recipes"], 2);
        assert!(
            first
                .objects
                .iter()
                .all(|o| o.target_state == TargetState::Absent)
        );
        let artifact = first
            .objects
            .iter()
            .find(|o| o.source_key.contains(&saved.dataset.latest.version))
            .unwrap();
        assert_eq!(artifact.references["/execution_id"], "old-run");
        assert_eq!(artifact.references["/recipe"], "saved/query");
        assert_eq!(
            artifact.references["/version"],
            saved.dataset.latest.version
        );
        // Simulate an operator's partial copy; dry-run classifies it without modifying it.
        let bytes = store
            .get(&first.objects[0].source_key)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(digest(&bytes), first.objects[0].sha256);
        store
            .put(&first.objects[0].target_key, bytes.clone())
            .await
            .unwrap();
        store
            .put(&first.objects[1].target_key, b"conflicting data".to_vec())
            .await
            .unwrap();
        let second = legacy.migration_manifest(target).await.unwrap();
        assert_eq!(
            second.objects.len(),
            first.objects.len(),
            "project keys are never migration sources"
        );
        assert_eq!(second.objects[0].target_state, TargetState::Identical);
        assert_eq!(second.objects[1].target_state, TargetState::Conflict);
        assert_eq!(
            store
                .get(&first.objects[0].source_key)
                .await
                .unwrap()
                .unwrap(),
            bytes
        );
        assert_eq!(
            store
                .get(&first.objects[1].target_key)
                .await
                .unwrap()
                .unwrap(),
            b"conflicting data"
        );
        assert!(
            legacy
                .for_project(target)
                .unwrap()
                .migration_manifest(scope())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn corrupt_legacy_bytes_refuse_a_manifest_without_writing_anything() {
        let store = Arc::new(MemoryObjectStore::new());
        store
            .put(
                "datasets/collections/broken/head.json",
                b"not json".to_vec(),
            )
            .await
            .unwrap();
        let registry = Registry::new(store.clone(), "datasets");
        assert!(registry.migration_manifest(scope()).await.is_err());
        assert_eq!(store.list("").await.unwrap().len(), 1);
    }
}
