//! Project ownership lives in storage keys, outside immutable content identities.
//! Constructing a registry does not authorize access: service adapters must check
//! a fresh IAM grant before each operation. There is no fallback to legacy data.
//!
//! The key rules live here and nowhere else. [`Registry::inventory`] is the one
//! that classifies a key; [`Registry::migration_manifest`] is the read-only
//! operator dry-run layered on top of it, kept because the IAM README's tool
//! invocation names it. Two classifiers over one layout would be two answers to
//! "is this object a dataset version", and the day they disagreed one of them
//! would copy bytes under a key nothing reads.
use crate::{Registry, RegistryError};
use aiwatcher_core::migration::{
    ContentKind, Inventory, InventoryError, InventoryObject, Reference, ReferenceKind, WriteOrder,
    key_is_safe, sha256_hex,
};
use aiwatcher_iam::ProjectScope;
use serde::Serialize;
use std::collections::BTreeMap;

/// Who owns a managed execution id.
const EXECUTION_OWNER: &str = "aiwatcher-execution and the event log";

/// The documents this registry writes, and nothing else.
///
/// Four categories with the same two-object shape, plus the library, which is
/// the same shape again under an id that is not digested because a library id
/// is already `[a-z0-9-]`.
enum Document {
    DatasetHead,
    DatasetVersion,
    RecipeHead,
    RecipeVersion,
    PipelineHead,
    PipelineVersion,
    LibraryHead,
    LibraryVersion,
}

impl Document {
    fn category(&self) -> &'static str {
        match self {
            Self::DatasetHead => "dataset-heads",
            Self::DatasetVersion => "dataset-versions",
            Self::RecipeHead => "recipe-heads",
            Self::RecipeVersion => "recipe-versions",
            Self::PipelineHead => "pipeline-heads",
            Self::PipelineVersion => "pipeline-versions",
            Self::LibraryHead => "library-heads",
            Self::LibraryVersion => "library-versions",
        }
    }

    /// The head is the index in every one of the four families: it holds the
    /// summaries a list reads and names the versions beside it.
    fn order(&self) -> WriteOrder {
        match self {
            Self::DatasetHead | Self::RecipeHead | Self::PipelineHead | Self::LibraryHead => {
                WriteOrder::Index
            }
            Self::DatasetVersion
            | Self::RecipeVersion
            | Self::PipelineVersion
            | Self::LibraryVersion => WriteOrder::Content,
        }
    }

    /// The legacy category name the operator dry-run counts by.
    fn legacy_bucket(&self) -> &'static str {
        match self {
            Self::DatasetHead | Self::DatasetVersion => "collections",
            Self::RecipeHead | Self::RecipeVersion => "recipes",
            Self::PipelineHead | Self::PipelineVersion => "pipelines",
            Self::LibraryHead | Self::LibraryVersion => "library",
        }
    }
}

impl Registry {
    /// Bind the entire registry (datasets, recipes, pipelines and library) to a
    /// typed scope. A bound registry cannot be rebound to a different project.
    pub fn for_project(&self, scope: ProjectScope) -> crate::Result<Self> {
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

    pub(crate) fn check_key(&self, key: &str) -> crate::Result<()> {
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

    /// Every curation object under this registry's prefix, and the key each one
    /// takes in `target`.
    ///
    /// Read-only: it lists, reads and hashes, and writes nothing anywhere.
    ///
    /// # Errors
    ///
    /// [`InventoryError::Damaged`] when an object under this prefix is not a
    /// curation document, and [`InventoryError::Refused`] when the source is
    /// already bound to a project.
    pub async fn inventory(&self, target: ProjectScope) -> Result<Inventory, InventoryError> {
        if self.scope.is_some() {
            return Err(InventoryError::Refused(
                "a migration source must be the unscoped dataset registry".to_owned(),
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
        let references =
            self.references(&body, document)
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
            category: document.category().to_owned(),
            source_key: key.to_owned(),
            target_key,
            order: document.order(),
            content: ContentKind::Json,
            bytes: body.len() as u64,
            sha256: sha256_hex(&body),
            references,
        })
    }

    /// What one document points at.
    ///
    /// Every document is parsed as the type this crate wrote it as, which is
    /// what makes a foreign object under this prefix a refusal rather than a
    /// copy. What is deliberately *not* read is the query text: a pipeline may
    /// name datasets inside a `data_frame()->read(...)`, and analysing that
    /// would be a dependency analysis of a language whose parser lives in a PHP
    /// service. It is reported as text nobody resolved.
    fn references(
        &self,
        body: &[u8],
        document: &Document,
    ) -> std::result::Result<BTreeMap<String, Reference>, String> {
        let mut out = BTreeMap::new();
        match document {
            Document::DatasetVersion => {
                let version: crate::DatasetVersion =
                    serde_json::from_slice(body).map_err(|e| e.to_string())?;
                self.summary_references("", &version.summary, &mut out);
                out.insert(
                    "/source".to_owned(),
                    opaque(&version.source, "what the rows were read from"),
                );
                out.insert(
                    "/pipeline".to_owned(),
                    opaque(
                        &version.pipeline,
                        "query text; dependencies inside it are not analysed",
                    ),
                );
            }
            Document::DatasetHead => {
                let head: crate::DatasetSummary =
                    serde_json::from_slice(body).map_err(|e| e.to_string())?;
                self.summary_references("/latest", &head.latest, &mut out);
                for (index, summary) in head.versions.iter().enumerate() {
                    self.summary_references(&format!("/versions/{index}"), summary, &mut out);
                }
                out.insert(
                    "/name".to_owned(),
                    Reference {
                        value: head.name.clone(),
                        kind: ReferenceKind::Opaque {
                            reason: "this dataset's own name".to_owned(),
                        },
                    },
                );
            }
            Document::RecipeHead | Document::RecipeVersion => {
                let recipe: crate::CurationRecipe =
                    serde_json::from_slice(body).map_err(|e| e.to_string())?;
                out.insert(
                    "/revision".to_owned(),
                    opaque(&recipe.revision, "this revision's own content address"),
                );
                out.insert(
                    "/pipeline".to_owned(),
                    opaque(
                        &recipe.pipeline,
                        "query text; dependencies inside it are not analysed",
                    ),
                );
            }
            Document::PipelineHead | Document::PipelineVersion => {
                let pipeline: crate::CurationPipeline =
                    serde_json::from_slice(body).map_err(|e| e.to_string())?;
                out.insert(
                    "/revision".to_owned(),
                    opaque(&pipeline.revision, "this revision's own content address"),
                );
                for (index, block) in pipeline.blocks.iter().enumerate() {
                    out.insert(
                        format!("/blocks/{index}/spec"),
                        opaque(
                            &block.id,
                            "an authored block spec; what it names is not resolved here",
                        ),
                    );
                }
            }
            Document::LibraryHead | Document::LibraryVersion => {
                let template: crate::BlockTemplate =
                    serde_json::from_slice(body).map_err(|e| e.to_string())?;
                out.insert(
                    "/revision".to_owned(),
                    opaque(&template.revision, "this revision's own content address"),
                );
            }
        }
        Ok(out)
    }

    /// The four references a version summary carries, wherever it is embedded.
    fn summary_references(
        &self,
        at: &str,
        summary: &crate::DatasetVersionSummary,
        out: &mut BTreeMap<String, Reference>,
    ) {
        out.insert(
            format!("{at}/version"),
            opaque(&summary.version, "this version's own content address"),
        );
        if let Some(recipe) = &summary.recipe {
            out.insert(
                format!("{at}/recipe"),
                Reference {
                    value: recipe.clone(),
                    kind: ReferenceKind::Internal {
                        source_key: self.recipe_head_key(recipe),
                    },
                },
            );
        }
        if let Some(produced_by) = &summary.produced_by {
            // `pipeline@revision`. Provenance rather than identity, and still
            // an object in this same registry when it parses as one.
            out.insert(
                format!("{at}/produced_by"),
                match produced_by.split_once('@') {
                    Some((name, revision)) if !name.is_empty() && !revision.is_empty() => {
                        Reference {
                            value: produced_by.clone(),
                            kind: ReferenceKind::Internal {
                                source_key: self.pipeline_version_key(name, revision),
                            },
                        }
                    }
                    _ => opaque(produced_by, "provenance that names no pinned revision"),
                },
            );
        }
        if let Some(execution) = &summary.execution_id {
            out.insert(
                format!("{at}/execution_id"),
                Reference {
                    value: execution.clone(),
                    kind: ReferenceKind::Foreign {
                        owner: EXECUTION_OWNER.to_owned(),
                    },
                },
            );
        }
    }

    /// Operator-only dry run. Reads bytes and inventories targets; never copies,
    /// deletes, republishes or rewrites a head. Call on the legacy root registry.
    /// This inventory is not a cutover authorization or a consistent snapshot.
    ///
    /// # Errors
    ///
    /// Whatever [`Registry::inventory`] refuses, plus a store that cannot be
    /// read while the target states are probed.
    pub async fn migration_manifest(
        &self,
        target: ProjectScope,
    ) -> crate::Result<MigrationManifest> {
        let inventory = self.inventory(target).await.map_err(|error| match error {
            InventoryError::Damaged { key, message } => RegistryError::Corrupt { key, message },
            InventoryError::Store(port) => RegistryError::Store(port),
            InventoryError::Refused(message) => RegistryError::Invalid(message),
        })?;
        let mut counts = BTreeMap::from([
            ("collections".to_owned(), 0),
            ("recipes".to_owned(), 0),
            ("pipelines".to_owned(), 0),
            ("library".to_owned(), 0),
        ]);
        let mut objects = Vec::new();
        for object in inventory.objects {
            let bytes = self
                .store
                .get(&object.source_key)
                .await?
                .ok_or_else(|| RegistryError::NotFound(object.source_key.clone()))?;
            let target_state = match self.store.get(&object.target_key).await? {
                None => TargetState::Absent,
                Some(existing) if existing == bytes => TargetState::Identical,
                Some(_) => TargetState::Conflict,
            };
            if let Some(document) = classify(
                object
                    .source_key
                    .strip_prefix(&inventory.source_prefix)
                    .unwrap_or(&object.source_key),
            ) {
                *counts
                    .entry(document.legacy_bucket().to_owned())
                    .or_insert(0) += 1;
            }
            objects.push(MigrationObject {
                source_key: object.source_key,
                target_key: object.target_key,
                bytes: object.bytes as usize,
                sha256: object.sha256,
                target_state,
                references: object
                    .references
                    .into_iter()
                    .map(|(pointer, reference)| {
                        (pointer, serde_json::Value::String(reference.value))
                    })
                    .collect(),
            });
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

fn opaque(value: &str, reason: &str) -> Reference {
    Reference {
        value: value.to_owned(),
        kind: ReferenceKind::Opaque {
            reason: reason.to_owned(),
        },
    }
}

/// Which document a relative key is, if it is one at all.
fn classify(rest: &str) -> Option<Document> {
    let json = |file: &str| file.ends_with(".json");
    match rest.split('/').collect::<Vec<_>>().as_slice() {
        ["collections", _, "head.json"] => Some(Document::DatasetHead),
        ["collections", _, "versions", file] if json(file) => Some(Document::DatasetVersion),
        ["recipes", _, "head.json"] => Some(Document::RecipeHead),
        ["recipes", _, "versions", file] if json(file) => Some(Document::RecipeVersion),
        ["pipelines", _, "head.json"] => Some(Document::PipelineHead),
        ["pipelines", _, "versions", file] if json(file) => Some(Document::PipelineVersion),
        ["library", _, "head.json"] => Some(Document::LibraryHead),
        ["library", _, "versions", file] if json(file) => Some(Document::LibraryVersion),
        _ => None,
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
    use crate::{PublishDatasetRequest, SaveRecipeRequest, digest};
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

    #[tokio::test]
    async fn a_pinned_revision_is_a_reference_and_query_text_is_reported_as_unanalysed() {
        use aiwatcher_core::migration::ReferenceKind;
        let store = Arc::new(MemoryObjectStore::new());
        let legacy = Registry::new(store.clone(), "datasets");
        let pipeline = legacy
            .save_pipeline(
                serde_json::from_value(serde_json::json!({
                    "name": "import", "description": "",
                    "blocks": [{"id": "source", "title": "Runs",
                                "spec": {"kind": "source", "dataset": "runs"}}],
                    "edges": []
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        legacy
            .save_recipe(
                serde_json::from_value(serde_json::json!({
                    "name": "saved/query", "description": "",
                    "pipeline": "data_frame()->read(default)"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        legacy
            .publish(
                serde_json::from_value(serde_json::json!({
                    "name": "houses", "recipe": "saved/query",
                    "pipeline": "data_frame()->read(runs)->write(to_output())",
                    "columns": ["value"], "items": [{"value": "a row"}],
                    "source": "runs", "execution_id": "exec-1",
                    "produced_by": format!("import@{}", pipeline.pipeline.revision)
                }))
                .unwrap(),
            )
            .await
            .unwrap();

        let before = store.list("").await.unwrap();
        let inventory = legacy.inventory(scope()).await.unwrap();
        assert_eq!(store.list("").await.unwrap(), before, "reads only");

        let version = inventory
            .objects
            .iter()
            .find(|object| object.category == "dataset-versions")
            .expect("a dataset version");
        let ReferenceKind::Internal { source_key } = &version.references["/recipe"].kind else {
            panic!("a recipe name is a head in this registry");
        };
        assert!(
            inventory
                .objects
                .iter()
                .any(|object| &object.source_key == source_key)
        );
        let ReferenceKind::Internal { source_key } = &version.references["/produced_by"].kind
        else {
            panic!("`name@revision` pins a pipeline revision this registry holds");
        };
        assert!(inventory.objects.iter().any(
            |object| &object.source_key == source_key && object.category == "pipeline-versions"
        ));
        assert!(matches!(
            version.references["/execution_id"].kind,
            ReferenceKind::Foreign { .. }
        ));
        let ReferenceKind::Opaque { reason } = &version.references["/pipeline"].kind else {
            panic!("query text is not a resolved dependency");
        };
        assert!(
            reason.contains("not analysed"),
            "the query names `runs`, and saying so would be a dependency analysis of a \
             language whose parser is in another service: {reason}"
        );
    }
}
