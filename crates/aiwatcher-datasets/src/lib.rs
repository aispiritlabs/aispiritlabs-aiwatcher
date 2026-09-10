//! Durable curation recipes and the versioned dataset artifacts they produce.
//!
//! Flow PHP owns transformation and execution. This crate owns the other half
//! of that boundary: keeping the exact script and rows that were executed so
//! an evaluation can name an immutable dataset version later.

use std::collections::BTreeMap;
use std::sync::Arc;

use aiwatcher_core::ports::{PortError, PortResult};
use aiwatcher_core::prompts::ObjectStore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use utoipa::ToSchema;

/// Which query engine a piece of authored text was written for.
mod engine;
mod library;
/// A curation assembled out of blocks rather than written as one script.
mod pipeline;

pub use engine::{QueryEngine, UnknownEngine};
pub use library::{BlockTemplate, BlockTemplatePage, SaveBlockTemplateRequest};

pub use pipeline::{
    BlockPosition, BlockSpec, CurationPipeline, PipelineBlock, PipelineEdge, PipelinePage,
    SavePipelineRequest, SavedPipeline, order_of,
};

const MAX_NAME_BYTES: usize = 160;
const MAX_DESCRIPTION_BYTES: usize = 8 * 1024;
const MAX_PIPELINE_BYTES: usize = 128 * 1024;
const MAX_ITEMS: usize = 1_000;
const MAX_ARTIFACT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PAGE_ROWS: usize = 100;
const MAX_SEARCH_BYTES: usize = 256;

/// A rejected or unavailable registry operation.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("{0}")]
    Invalid(String),
    /// A request that failed in several ways at once, listed together.
    ///
    /// A pipeline is the case that needs it: somebody wiring a canvas fixes
    /// what they can see, and a validator that reports one problem per round
    /// trip teaches people to press the button again instead of reading it.
    #[error("the pipeline was refused: {}", .0.join("; "))]
    Rejected(Vec<String>),
    #[error("{0} not found")]
    NotFound(String),
    #[error("{what} is {size} bytes; the limit is {limit}")]
    TooLarge {
        what: &'static str,
        size: usize,
        limit: usize,
    },
    #[error("the dataset registry could not use its object store: {0}")]
    Store(#[from] PortError),
    #[error("stored object {key} is not a dataset registry document: {message}")]
    Corrupt { key: String, message: String },
}

pub type Result<T> = std::result::Result<T, RegistryError>;

/// A saved query, in the language of the engine it names.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct CurationRecipe {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub pipeline: String,
    /// The engine `pipeline` was written for. Absent is Flow, and stays absent
    /// when saved, so a revision from before the field keeps its digest.
    #[serde(default, skip_serializing_if = "QueryEngine::is_flow")]
    pub engine: QueryEngine,
    /// SHA-256 of the authored fields. The stable identity of this revision.
    pub revision: String,
    #[serde(with = "time::serde::rfc3339")]
    pub saved_at: OffsetDateTime,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct SaveRecipeRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub pipeline: String,
    /// The engine `pipeline` was written for; absent is Flow. Part of the
    /// revision only when it is not Flow — see [`QueryEngine`].
    #[serde(default, skip_serializing_if = "QueryEngine::is_flow")]
    pub engine: QueryEngine,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct SavedRecipe {
    pub recipe: CurationRecipe,
    /// False when this exact immutable revision was already present.
    pub created: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct RecipePage {
    pub recipes: Vec<CurationRecipe>,
}

/// What one completed query execution contributes to the registry.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct PublishDatasetRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Saved recipe name, when the run came from one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe: Option<String>,
    /// The exact script that produced `items`, even when it was not saved first.
    pub pipeline: String,
    /// The engine `pipeline` is written for; absent is Flow. Without it a
    /// DataFusion script read months later is text in an unnamed language.
    /// Part of the version's identity only when it is not Flow, so every
    /// version published before the field keeps its id.
    #[serde(default, skip_serializing_if = "QueryEngine::is_flow")]
    pub engine: QueryEngine,
    #[serde(default)]
    pub columns: Vec<String>,
    pub items: Vec<BTreeMap<String, Value>>,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_seconds: Option<u64>,
    /// The block chain that produced these rows, as `name@revision`.
    ///
    /// Present when the execution came from the pipeline canvas rather than
    /// from one script, and provenance rather than identity — like the recipe
    /// name and the description, and for the same reason: a block dragged
    /// across the canvas is a new pipeline revision and the same rows, and a
    /// dataset version per canvas tidy-up would be a version history about
    /// layout.
    ///
    /// It matters because the Flow script alone does not describe this
    /// execution: a notebook block ran between that script and these rows, and
    /// the only place its settings and its pinned source are written down is
    /// the pipeline revision this names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub produced_by: Option<String>,
    /// The managed execution that produced these rows, when one did.
    ///
    /// Provenance and not identity, for `produced_by`'s reason and with a
    /// second one of its own: the same rows published by a rerun are the same
    /// version, and a version per run would be a version history about *when*.
    /// What it buys is the join the other way — from a published dataset back
    /// to the run, its waterfall, and its `step.*` on the event log. Absent for
    /// a version the panel published from an ad-hoc query (ADR_0025 withdrew
    /// that path for managed runs only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct DatasetVersionSummary {
    pub version: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    pub row_count: usize,
    pub columns: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub produced_by: Option<String>,
    /// The managed execution that produced this version, when one did. Kept in
    /// the summary as well as the request so a dataset list can link to a run
    /// without reading every version artifact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct DatasetVersion {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(flatten)]
    pub summary: DatasetVersionSummary,
    pub pipeline: String,
    /// The engine `pipeline` is written for; absent is Flow.
    #[serde(default, skip_serializing_if = "QueryEngine::is_flow")]
    pub engine: QueryEngine,
    pub items: Vec<BTreeMap<String, Value>>,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_seconds: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct DatasetSummary {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub latest: DatasetVersionSummary,
    /// Newest first. Kept in the head so listing does not read every artifact.
    pub versions: Vec<DatasetVersionSummary>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct DatasetPage {
    pub datasets: Vec<DatasetSummary>,
}

/// One original row and its stable position inside an immutable version.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct DatasetRow {
    pub row_index: usize,
    #[schema(value_type = Object)]
    pub row: BTreeMap<String, Value>,
}

/// A small slice suitable for an interactive data viewer.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct DatasetRowsPage {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub version: DatasetVersionSummary,
    pub pipeline: String,
    /// The engine `pipeline` is written for; absent is Flow.
    #[serde(default, skip_serializing_if = "QueryEngine::is_flow")]
    pub engine: QueryEngine,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_seconds: Option<u64>,
    pub rows: Vec<DatasetRow>,
    /// Rows in the immutable artifact before search is applied.
    pub total_rows: usize,
    /// Rows matching the current search across all pages.
    pub matching_rows: usize,
    pub offset: usize,
    pub limit: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct PublishedDataset {
    pub dataset: DatasetSummary,
    /// False when this exact pipeline and exact ordered set of rows already existed.
    pub created: bool,
}

/// One object-store namespace for recipes and artifacts.
#[derive(Clone, Debug)]
pub struct Registry {
    store: Arc<dyn ObjectStore>,
    prefix: String,
}

impl Registry {
    #[must_use]
    pub fn new(store: Arc<dyn ObjectStore>, prefix: impl Into<String>) -> Self {
        Self {
            store,
            prefix: prefix.into().trim_matches('/').to_owned(),
        }
    }

    /// Save an immutable recipe revision and move its small head document to it.
    pub async fn save_recipe(&self, request: SaveRecipeRequest) -> Result<SavedRecipe> {
        validate_name(&request.name, "recipe")?;
        validate_authored(&request.description, &request.pipeline)?;

        let revision = digest(&recipe_identity(&request)?);
        let version_key = self.recipe_version_key(&request.name, &revision);
        let existing: Option<CurationRecipe> = self.read_json(&version_key).await?;
        let created = existing.is_none();
        let recipe = match existing {
            Some(recipe) => recipe,
            None => CurationRecipe {
                name: request.name,
                description: request.description,
                pipeline: request.pipeline,
                engine: request.engine,
                revision,
                saved_at: OffsetDateTime::now_utc(),
            },
        };

        if created {
            self.write_json(&version_key, &recipe).await?;
        }
        self.write_json(&self.recipe_head_key(&recipe.name), &recipe)
            .await?;

        Ok(SavedRecipe { recipe, created })
    }

    pub async fn recipes(&self) -> Result<RecipePage> {
        let marker = "/head.json";
        let mut recipes = Vec::new();
        for entry in self
            .store
            .list(&format!("{}/recipes/", self.prefix))
            .await?
        {
            if !entry.key.ends_with(marker) {
                continue;
            }
            if let Some(recipe) = self.read_json::<CurationRecipe>(&entry.key).await? {
                recipes.push(recipe);
            }
        }
        recipes.sort_by_key(|recipe| std::cmp::Reverse(recipe.saved_at));
        Ok(RecipePage { recipes })
    }

    /// Store the exact output of one query run as a content-addressed version.
    pub async fn publish(&self, request: PublishDatasetRequest) -> Result<PublishedDataset> {
        validate_name(&request.name, "dataset")?;
        if let Some(recipe) = &request.recipe {
            validate_name(recipe, "recipe")?;
        }
        validate_authored(&request.description, &request.pipeline)?;
        if request.items.len() > MAX_ITEMS {
            return Err(RegistryError::TooLarge {
                what: "the dataset",
                size: request.items.len(),
                limit: MAX_ITEMS,
            });
        }

        if let Some(produced_by) = &request.produced_by
            && produced_by.len() > MAX_PROVENANCE_BYTES
        {
            return Err(RegistryError::TooLarge {
                what: "the provenance reference",
                size: produced_by.len(),
                limit: MAX_PROVENANCE_BYTES,
            });
        }

        let requested_description = request.description.clone();
        let identity = dataset_identity(&request)?;
        if identity.len() > MAX_ARTIFACT_BYTES {
            return Err(RegistryError::TooLarge {
                what: "the encoded dataset artifact",
                size: identity.len(),
                limit: MAX_ARTIFACT_BYTES,
            });
        }
        let version_id = digest(&identity);
        let version_key = self.dataset_version_key(&request.name, &version_id);
        let existing: Option<DatasetVersion> = self.read_json(&version_key).await?;
        let created = existing.is_none();
        let version = match existing {
            Some(version) => version,
            None => {
                let summary = DatasetVersionSummary {
                    version: version_id,
                    created_at: OffsetDateTime::now_utc(),
                    row_count: request.items.len(),
                    columns: request.columns.clone(),
                    recipe: request.recipe.clone(),
                    produced_by: request.produced_by.clone(),
                    execution_id: request.execution_id.clone(),
                };
                DatasetVersion {
                    name: request.name.clone(),
                    description: request.description.clone(),
                    summary,
                    pipeline: request.pipeline,
                    engine: request.engine,
                    items: request.items,
                    source: request.source,
                    window_seconds: request.window_seconds,
                }
            }
        };

        if created {
            self.write_json(&version_key, &version).await?;
        }

        let head_key = self.dataset_head_key(&version.name);
        let mut head = self
            .read_json::<DatasetSummary>(&head_key)
            .await?
            .unwrap_or_else(|| DatasetSummary {
                name: version.name.clone(),
                description: version.description.clone(),
                latest: version.summary.clone(),
                versions: Vec::new(),
            });
        head.description = requested_description;
        head.latest = version.summary.clone();
        head.versions
            .retain(|item| item.version != version.summary.version);
        head.versions.insert(0, version.summary);
        self.write_json(&head_key, &head).await?;

        Ok(PublishedDataset {
            dataset: head,
            created,
        })
    }

    pub async fn datasets(&self) -> Result<DatasetPage> {
        let marker = "/head.json";
        let mut datasets = Vec::new();
        for entry in self
            .store
            .list(&format!("{}/collections/", self.prefix))
            .await?
        {
            if !entry.key.ends_with(marker) {
                continue;
            }
            if let Some(dataset) = self.read_json::<DatasetSummary>(&entry.key).await? {
                datasets.push(dataset);
            }
        }
        datasets.sort_by_key(|dataset| std::cmp::Reverse(dataset.latest.created_at));
        Ok(DatasetPage { datasets })
    }

    /// Read one immutable version in bounded slices, with optional text search.
    pub async fn rows(
        &self,
        name: &str,
        version: Option<&str>,
        offset: usize,
        limit: usize,
        search: Option<&str>,
    ) -> Result<DatasetRowsPage> {
        validate_name(name, "dataset")?;
        let search = search.map(str::trim).filter(|value| !value.is_empty());
        if search.is_some_and(|value| value.len() > MAX_SEARCH_BYTES) {
            return Err(RegistryError::TooLarge {
                what: "the dataset search",
                size: search.map_or(0, str::len),
                limit: MAX_SEARCH_BYTES,
            });
        }

        let head = self
            .read_json::<DatasetSummary>(&self.dataset_head_key(name))
            .await?
            .ok_or_else(|| RegistryError::NotFound(format!("dataset {name}")))?;
        let summary = match version {
            Some(version) => head
                .versions
                .iter()
                .find(|candidate| candidate.version == version)
                .cloned()
                .ok_or_else(|| {
                    RegistryError::NotFound(format!("dataset {name} version {version}"))
                })?,
            None => head.latest.clone(),
        };
        let artifact = self
            .read_json::<DatasetVersion>(&self.dataset_version_key(name, &summary.version))
            .await?
            .ok_or_else(|| {
                RegistryError::NotFound(format!("dataset {name} version {}", summary.version))
            })?;

        let needle = search.map(str::to_lowercase);
        let matching: Vec<(usize, &BTreeMap<String, Value>)> = artifact
            .items
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                needle
                    .as_deref()
                    .is_none_or(|needle| row.values().any(|value| value_contains(value, needle)))
            })
            .collect();
        let matching_rows = matching.len();
        let limit = limit.clamp(1, MAX_PAGE_ROWS);
        let rows = matching
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|(row_index, row)| DatasetRow {
                row_index,
                row: row.clone(),
            })
            .collect::<Vec<_>>();
        let consumed = offset.saturating_add(rows.len());
        let next_offset = (consumed < matching_rows).then_some(consumed);

        Ok(DatasetRowsPage {
            name: artifact.name,
            description: head.description,
            version: summary,
            pipeline: artifact.pipeline,
            engine: artifact.engine,
            source: artifact.source,
            window_seconds: artifact.window_seconds,
            rows,
            total_rows: artifact.items.len(),
            matching_rows,
            offset,
            limit,
            next_offset,
        })
    }

    fn id(name: &str) -> String {
        digest(name.as_bytes())
    }

    fn recipe_head_key(&self, name: &str) -> String {
        format!("{}/recipes/{}/head.json", self.prefix, Self::id(name))
    }

    fn recipe_version_key(&self, name: &str, revision: &str) -> String {
        format!(
            "{}/recipes/{}/versions/{revision}.json",
            self.prefix,
            Self::id(name)
        )
    }

    fn dataset_head_key(&self, name: &str) -> String {
        format!("{}/collections/{}/head.json", self.prefix, Self::id(name))
    }

    fn dataset_version_key(&self, name: &str, version: &str) -> String {
        format!(
            "{}/collections/{}/versions/{version}.json",
            self.prefix,
            Self::id(name)
        )
    }

    async fn read_json<T: for<'de> Deserialize<'de>>(&self, key: &str) -> Result<Option<T>> {
        let Some(body) = self.store.get(key).await? else {
            return Ok(None);
        };
        serde_json::from_slice(&body)
            .map(Some)
            .map_err(|error| RegistryError::Corrupt {
                key: key.to_owned(),
                message: error.to_string(),
            })
    }

    async fn write_json<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let body = serde_json::to_vec(value).map_err(|error| RegistryError::Corrupt {
            key: key.to_owned(),
            message: error.to_string(),
        })?;
        self.store.put(key, body).await?;
        Ok(())
    }
}

fn validate_name(name: &str, what: &str) -> Result<()> {
    if name.is_empty() || name.len() > MAX_NAME_BYTES {
        return Err(RegistryError::Invalid(format!(
            "a {what} name must be between 1 and {MAX_NAME_BYTES} bytes"
        )));
    }
    for segment in name.split('/') {
        let mut characters = segment.chars();
        let starts_well = characters
            .next()
            .is_some_and(|value| value.is_ascii_alphanumeric());
        let continues_well = characters
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, '-' | '_' | '.'));
        if !starts_well || !continues_well || matches!(segment, "." | "..") {
            return Err(RegistryError::Invalid(format!(
                "{what} name segments must start with a letter or number and contain only letters, numbers, '.', '_' or '-'"
            )));
        }
    }
    Ok(())
}

const MAX_PROVENANCE_BYTES: usize = 240;

fn validate_authored(description: &str, pipeline: &str) -> Result<()> {
    if pipeline.trim().is_empty() {
        return Err(RegistryError::Invalid(
            "the Flow PHP pipeline is empty".to_owned(),
        ));
    }
    for (what, size, limit) in [
        ("the description", description.len(), MAX_DESCRIPTION_BYTES),
        ("the Flow PHP pipeline", pipeline.len(), MAX_PIPELINE_BYTES),
    ] {
        if size > limit {
            return Err(RegistryError::TooLarge { what, size, limit });
        }
    }
    Ok(())
}

fn recipe_identity(request: &SaveRecipeRequest) -> Result<Vec<u8>> {
    serde_json::to_vec(request).map_err(|error| RegistryError::Invalid(error.to_string()))
}

fn dataset_identity(request: &PublishDatasetRequest) -> Result<Vec<u8>> {
    // Description, recipe name, `produced_by` and `execution_id` are mutable
    // catalogue and provenance fields. The version identity is only what
    // changes the repeatable execution: the exact script, ordered rows and
    // source window. Two runs of one plan over unchanged rows are therefore
    // one version — which is what makes a rerun idempotent rather than a
    // version history about *when*.
    //
    // The engine joins it only when it is not Flow: the same text is a
    // different execution in another language, and every version published
    // before the field was Flow and must keep the id it was published under.
    let executed = (
        &request.pipeline,
        &request.columns,
        &request.items,
        &request.source,
        request.window_seconds,
    );
    if request.engine.is_flow() {
        serde_json::to_vec(&executed)
    } else {
        serde_json::to_vec(&(executed, request.engine))
    }
    .map_err(|error| RegistryError::Invalid(error.to_string()))
}

fn digest(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

fn value_contains(value: &Value, needle: &str) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => value.to_string().contains(needle),
        Value::Number(value) => value.to_string().contains(needle),
        Value::String(value) => value.to_lowercase().contains(needle),
        Value::Array(values) => values.iter().any(|value| value_contains(value, needle)),
        Value::Object(values) => values.values().any(|value| value_contains(value, needle)),
    }
}

/// Keep the object-store failure vocabulary consistent with the other registry.
impl From<RegistryError> for PortError {
    fn from(error: RegistryError) -> Self {
        match error {
            RegistryError::Store(error) => error,
            other => PortError::Rejected {
                target: "dataset-registry",
                message: other.to_string(),
            },
        }
    }
}

/// A small probe useful to wiring and health checks.
pub async fn probe(store: &dyn ObjectStore, prefix: &str) -> PortResult<()> {
    store.list(prefix).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aiwatcher_prompts::adapters::memory::MemoryObjectStore;

    fn registry() -> Registry {
        Registry::new(Arc::new(MemoryObjectStore::new()), "datasets")
    }

    fn recipe(pipeline: &str) -> SaveRecipeRequest {
        SaveRecipeRequest {
            name: "production/failed-runs".to_owned(),
            description: "Regression candidates".to_owned(),
            pipeline: pipeline.to_owned(),
            engine: QueryEngine::Flow,
        }
    }

    fn dataset(pipeline: &str) -> PublishDatasetRequest {
        PublishDatasetRequest {
            name: "support/conversations".to_owned(),
            description: "Production conversations".to_owned(),
            recipe: Some("production/failed-runs".to_owned()),
            pipeline: pipeline.to_owned(),
            engine: QueryEngine::Flow,
            columns: vec!["run_id".to_owned()],
            items: vec![BTreeMap::from([(
                "run_id".to_owned(),
                Value::String("run-1".to_owned()),
            )])],
            source: "http://api.test".to_owned(),
            window_seconds: Some(900),
            produced_by: None,
            execution_id: None,
        }
    }

    #[tokio::test]
    async fn a_recipe_saved_before_the_engine_field_keeps_its_revision() {
        // The bytes the request serialised to before AW-3, written out rather
        // than rebuilt, so the test cannot agree with a change by construction.
        let before = digest(
            br#"{"name":"production/failed-runs","description":"Regression candidates","pipeline":"data_frame()->read(default)"}"#,
        );
        let registry = registry();

        let saved = registry
            .save_recipe(recipe("data_frame()->read(default)"))
            .await
            .unwrap();
        assert_eq!(saved.recipe.revision, before);

        // The same text for another engine is another recipe, not a rename.
        let other = registry
            .save_recipe(SaveRecipeRequest {
                engine: QueryEngine::DataFusion,
                ..recipe("data_frame()->read(default)")
            })
            .await
            .unwrap();
        assert_ne!(other.recipe.revision, before);
        assert_eq!(other.recipe.engine, QueryEngine::DataFusion);
    }

    #[tokio::test]
    async fn a_version_published_before_the_engine_field_keeps_its_id() {
        let request = dataset("data_frame()->read(default)");
        let before = digest(
            &serde_json::to_vec(&(
                &request.pipeline,
                &request.columns,
                &request.items,
                &request.source,
                request.window_seconds,
            ))
            .unwrap(),
        );
        let registry = registry();

        let flow = registry.publish(request.clone()).await.unwrap();
        assert_eq!(flow.dataset.latest.version, before);

        // The same text and the same rows from another engine are a different
        // execution, and the version says which one it was.
        let datafusion = registry
            .publish(PublishDatasetRequest {
                engine: QueryEngine::DataFusion,
                ..request
            })
            .await
            .unwrap();
        assert_ne!(datafusion.dataset.latest.version, before);
    }

    #[tokio::test]
    async fn saving_the_same_recipe_is_idempotent_and_an_edit_is_a_revision() {
        let registry = registry();
        let first = registry
            .save_recipe(recipe("data_frame()->read(default)"))
            .await
            .unwrap();
        let same = registry
            .save_recipe(recipe("data_frame()->read(default)"))
            .await
            .unwrap();
        let edited = registry
            .save_recipe(recipe("data_frame()->read(default)->limit(5)"))
            .await
            .unwrap();

        assert!(first.created);
        assert!(!same.created);
        assert_ne!(first.recipe.revision, edited.recipe.revision);
        let page = registry.recipes().await.unwrap();
        assert_eq!(page.recipes.len(), 1);
        assert_eq!(page.recipes[0].revision, edited.recipe.revision);
    }

    #[tokio::test]
    async fn every_distinct_execution_becomes_a_dataset_version() {
        let registry = registry();
        let first = registry
            .publish(dataset("data_frame()->read(default)"))
            .await
            .unwrap();
        let same = registry
            .publish(dataset("data_frame()->read(default)"))
            .await
            .unwrap();
        let mut redescribed = dataset("data_frame()->read(default)");
        redescribed.description = "A clearer catalogue description".to_owned();
        let redescribed = registry.publish(redescribed).await.unwrap();
        let changed = registry
            .publish(dataset("data_frame()->read(default)->limit(1)"))
            .await
            .unwrap();

        assert!(first.created);
        assert!(!same.created);
        assert!(
            !redescribed.created,
            "catalogue metadata is not dataset content"
        );
        assert_eq!(
            redescribed.dataset.description,
            "A clearer catalogue description"
        );
        assert!(changed.created);
        assert_eq!(changed.dataset.versions.len(), 2);
        assert_ne!(first.dataset.latest.version, changed.dataset.latest.version);
        assert_eq!(registry.datasets().await.unwrap().datasets.len(), 1);
    }

    #[tokio::test]
    async fn where_a_version_came_from_is_provenance_rather_than_identity() {
        // A block dragged across the canvas is a new pipeline revision and the
        // same rows. A dataset version per canvas tidy-up would be a version
        // history about layout.
        let registry = registry();
        let mut first = dataset("data_frame()->read(default)");
        first.produced_by = Some("curation/pii-detection@aaaa".to_owned());
        let mut moved = dataset("data_frame()->read(default)");
        moved.produced_by = Some("curation/pii-detection@bbbb".to_owned());

        let first = registry.publish(first).await.unwrap();
        let moved = registry.publish(moved).await.unwrap();

        assert!(first.created);
        assert!(!moved.created);
        assert_eq!(
            first.dataset.latest.produced_by.as_deref(),
            Some("curation/pii-detection@aaaa"),
            "the version keeps the chain it was first published from"
        );
    }

    #[tokio::test]
    async fn names_support_folders_without_accepting_paths() {
        let registry = registry();
        assert!(
            registry
                .save_recipe(recipe("data_frame()->read(default)"))
                .await
                .is_ok()
        );
        for invalid in ["../secret", "/root", "folder//name", "name with space"] {
            let mut request = recipe("data_frame()->read(default)");
            request.name = invalid.to_owned();
            assert!(registry.save_recipe(request).await.is_err(), "{invalid}");
        }
    }

    #[tokio::test]
    async fn dataset_rows_are_sliced_and_searched_without_returning_the_whole_artifact() {
        let registry = registry();
        let mut request = dataset("data_frame()->read(default)");
        request.columns = vec!["case_id".to_owned(), "input".to_owned()];
        request.items = vec![
            BTreeMap::from([
                ("case_id".to_owned(), Value::String("case-1".to_owned())),
                ("input".to_owned(), Value::String("Alpha prompt".to_owned())),
            ]),
            BTreeMap::from([
                ("case_id".to_owned(), Value::String("case-2".to_owned())),
                ("input".to_owned(), Value::String("Beta prompt".to_owned())),
            ]),
            BTreeMap::from([
                ("case_id".to_owned(), Value::String("case-3".to_owned())),
                (
                    "input".to_owned(),
                    serde_json::json!({ "nested": "beta follow-up" }),
                ),
            ]),
        ];
        registry.publish(request).await.unwrap();

        let first = registry
            .rows("support/conversations", None, 0, 2, None)
            .await
            .unwrap();
        assert_eq!(first.total_rows, 3);
        assert_eq!(first.matching_rows, 3);
        assert_eq!(first.rows.len(), 2);
        assert_eq!(first.rows[0].row_index, 0);
        assert_eq!(first.next_offset, Some(2));

        let searched = registry
            .rows("support/conversations", None, 0, 100, Some("BETA"))
            .await
            .unwrap();
        assert_eq!(searched.total_rows, 3);
        assert_eq!(searched.matching_rows, 2);
        assert_eq!(
            searched
                .rows
                .iter()
                .map(|row| row.row_index)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(searched.next_offset, None);
    }

    #[tokio::test]
    async fn an_unknown_dataset_or_version_is_not_found() {
        let registry = registry();
        assert!(matches!(
            registry.rows("missing", None, 0, 50, None).await,
            Err(RegistryError::NotFound(_))
        ));

        registry
            .publish(dataset("data_frame()->read(default)"))
            .await
            .unwrap();
        assert!(matches!(
            registry
                .rows("support/conversations", Some("missing"), 0, 50, None)
                .await,
            Err(RegistryError::NotFound(_))
        ));
    }
}
