//! A curation as a chain of blocks, and the rules that make one runnable.
//!
//! The other half of [`CurationRecipe`](crate::CurationRecipe): a recipe is one
//! Flow PHP script, a pipeline is the same job assembled out of blocks. Nothing
//! here executes anything — each block belongs to a different engine, and two
//! of the three are optional services this binary does not know exist. What
//! this crate owns is the definition: an immutable, content-addressed revision.
//!
//! **The shape is a chain**: one head, one tail, everything reachable. A block
//! with two parents would need to be told how to combine them, and one with two
//! children would run twice. What is refused is a shape that could not run, not
//! one that is untidy.
//!
//! **A transform may not follow a notebook.** A Flow block's input is a
//! `read()` from the query service's catalog; there is no way to hand it rows a
//! notebook produced, so such a chain would run the transform against the
//! *source* again and quietly produce something else. Refused by name.
//!
//! ADR_0024.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::{
    MAX_DESCRIPTION_BYTES, MAX_PIPELINE_BYTES, Registry, RegistryError, Result, digest,
    validate_name,
};

/// A canvas holds a chain somebody can read at a glance, not a program.
const MAX_BLOCKS: usize = 24;
const MAX_TITLE_BYTES: usize = 120;
const MAX_SETTINGS_BYTES: usize = 32 * 1024;
const MAX_ID_LENGTH: usize = 40;

/// What one block is, and everything that block kind needs.
///
/// Internally tagged rather than flattened: `spec.kind` is one word a reader
/// and a `switch` both understand, and the generated TypeScript is a
/// discriminated union rather than an intersection of optional fields.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BlockSpec {
    /// Where the rows come from: one dataset in the Flow service's catalog,
    /// with the arguments that catalog declares for it. `hub_rows` is a
    /// Hugging Face corpus; `runs` and `spans` are this installation's own log.
    Source {
        dataset: String,
        /// `read()` arguments, by name. Values are sent as written — the Flow
        /// service refuses one the dataset never declared, which is where that
        /// check belongs.
        #[serde(default)]
        arguments: BTreeMap<String, String>,
    },
    /// Flow PHP steps, appended to the source's `read()`. The tail of a
    /// pipeline, without `data_frame()`, without the `read()` and without the
    /// `write()`: those three are what the chain contributes.
    Transform {
        #[serde(default)]
        steps: String,
    },
    /// A marimo notebook, run over the rows by the marimo service.
    ///
    /// `revision` is the digest of the notebook source as it was when this
    /// pipeline was saved. The source itself lives in the notebook directory,
    /// because that is the file marimo serves, a run executes and a test
    /// imports — so this is a pin rather than a copy, and the panel says so
    /// when the two disagree.
    Notebook {
        notebook: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        revision: Option<String>,
        /// What the notebook's `mo.ui` elements start at: the block's settings,
        /// which a headless run has instead of somebody moving a slider.
        #[serde(default)]
        #[schema(value_type = Object)]
        params: BTreeMap<String, Value>,
    },
    /// The end of the chain: the rows, and the dataset a version of them is
    /// published to.
    View {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dataset: Option<String>,
    },
}

impl BlockSpec {
    /// The word this kind is called by, for a message a person reads.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Source { .. } => "source",
            Self::Transform { .. } => "transform",
            Self::Notebook { .. } => "notebook",
            Self::View { .. } => "view",
        }
    }
}

/// Where a block sits on the canvas. Presentation, and nothing reads it but the
/// canvas — the chain is the edges.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct BlockPosition {
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct PipelineBlock {
    /// Stable within one pipeline. What the edges name.
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub position: BlockPosition,
    pub spec: BlockSpec,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct PipelineEdge {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct CurationPipeline {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub blocks: Vec<PipelineBlock>,
    pub edges: Vec<PipelineEdge>,
    /// SHA-256 of the authored fields. The stable identity of this revision.
    pub revision: String,
    #[serde(with = "time::serde::rfc3339")]
    pub saved_at: OffsetDateTime,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct SavePipelineRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub blocks: Vec<PipelineBlock>,
    #[serde(default)]
    pub edges: Vec<PipelineEdge>,
}

impl SavePipelineRequest {
    /// The same validation for an authored save and an import preflight.
    pub fn validate(&self) -> Result<()> {
        validate_name(&self.name, "pipeline")?;
        if self.description.len() > MAX_DESCRIPTION_BYTES {
            return Err(RegistryError::TooLarge {
                what: "the description",
                size: self.description.len(),
                limit: MAX_DESCRIPTION_BYTES,
            });
        }
        check(&self.blocks, &self.edges)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct SavedPipeline {
    pub pipeline: CurationPipeline,
    /// False when this exact immutable revision was already present.
    pub created: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct PipelinePage {
    pub pipelines: Vec<CurationPipeline>,
}

impl Registry {
    /// Save an immutable pipeline revision and move its head document to it.
    ///
    /// Same two orderings as everything else in this crate: the version object
    /// is written before the head that indexes it, and the head is derived.
    pub async fn save_pipeline(&self, request: SavePipelineRequest) -> Result<SavedPipeline> {
        request.validate()?;

        let revision = digest(
            &serde_json::to_vec(&request)
                .map_err(|error| RegistryError::Invalid(error.to_string()))?,
        );
        let version_key = self.pipeline_version_key(&request.name, &revision);
        let existing: Option<CurationPipeline> = self.read_json(&version_key).await?;
        let created = existing.is_none();
        let pipeline = match existing {
            Some(pipeline) => pipeline,
            None => CurationPipeline {
                name: request.name,
                description: request.description,
                blocks: request.blocks,
                edges: request.edges,
                revision,
                saved_at: OffsetDateTime::now_utc(),
            },
        };

        if created {
            self.write_json(&version_key, &pipeline).await?;
        }
        self.write_json(&self.pipeline_head_key(&pipeline.name), &pipeline)
            .await?;

        Ok(SavedPipeline { pipeline, created })
    }

    /// One saved pipeline: a pinned revision, or the head when none is named.
    ///
    /// The pinned read is what makes a managed execution repeatable — a run
    /// compiles the revision it was asked for, and editing the definition
    /// while it is going creates a new revision and changes nothing about what
    /// is already running (ADR_0025).
    pub async fn pipeline(
        &self,
        name: &str,
        revision: Option<&str>,
    ) -> Result<Option<CurationPipeline>> {
        validate_name(name, "pipeline")?;
        let key = match revision.filter(|revision| !revision.is_empty()) {
            Some(revision) => self.pipeline_version_key(name, revision),
            None => self.pipeline_head_key(name),
        };
        self.read_json(&key).await
    }

    /// Every saved pipeline, newest save first.
    pub async fn pipelines(&self) -> Result<PipelinePage> {
        let marker = "/head.json";
        let mut pipelines = Vec::new();
        for entry in self
            .store
            .list(&format!("{}/pipelines/", self.prefix))
            .await?
        {
            if !entry.key.ends_with(marker) {
                continue;
            }
            if let Some(pipeline) = self.read_json::<CurationPipeline>(&entry.key).await? {
                pipelines.push(pipeline);
            }
        }
        pipelines.sort_by_key(|pipeline| std::cmp::Reverse(pipeline.saved_at));
        Ok(PipelinePage { pipelines })
    }

    fn pipeline_head_key(&self, name: &str) -> String {
        format!("{}/pipelines/{}/head.json", self.prefix, Self::id(name))
    }

    fn pipeline_version_key(&self, name: &str, revision: &str) -> String {
        format!(
            "{}/pipelines/{}/versions/{revision}.json",
            self.prefix,
            Self::id(name)
        )
    }
}

/// The chain, head first, or every reason there is not one.
///
/// Every problem at once rather than the first: somebody wiring a canvas fixes
/// what they can see, and a validator that reports one thing per round trip
/// teaches people to press the button again instead of reading it. Same reason
/// the annotation registry reports a drawing's problems together.
pub fn order_of<'a>(
    blocks: &'a [PipelineBlock],
    edges: &[PipelineEdge],
) -> std::result::Result<Vec<&'a PipelineBlock>, Vec<String>> {
    let mut problems = Vec::new();

    if blocks.is_empty() {
        return Err(vec!["a pipeline needs at least one block".to_owned()]);
    }
    if blocks.len() > MAX_BLOCKS {
        problems.push(format!(
            "a pipeline holds at most {MAX_BLOCKS} blocks; this one has {}",
            blocks.len()
        ));
    }

    let mut known: BTreeMap<&str, &PipelineBlock> = BTreeMap::new();
    for block in blocks {
        if !is_block_id(&block.id) {
            problems.push(format!(
                "'{}' is not a block id: lower-case letters, digits and dashes, at most {MAX_ID_LENGTH}",
                block.id
            ));
            continue;
        }
        if known.insert(block.id.as_str(), block).is_some() {
            problems.push(format!("two blocks share the id '{}'", block.id));
        }
        problems.extend(block_problems(block));
    }

    let mut next: BTreeMap<&str, &str> = BTreeMap::new();
    let mut has_parent: BTreeSet<&str> = BTreeSet::new();
    for edge in edges {
        let (from, to) = (edge.from.as_str(), edge.to.as_str());
        if !known.contains_key(from) || !known.contains_key(to) {
            problems.push(format!(
                "the connection {from} → {to} names a block that is not on the canvas"
            ));
            continue;
        }
        if from == to {
            problems.push(format!("{from} is connected to itself"));
            continue;
        }
        if next.insert(from, to).is_some() {
            problems.push(format!(
                "{from} feeds more than one block; a curation is a chain, so each block has one next"
            ));
        }
        if !has_parent.insert(to) {
            problems.push(format!(
                "{to} is fed by more than one block; a curation is a chain, so each block has one input"
            ));
        }
    }

    let heads: Vec<&&PipelineBlock> = known
        .values()
        .filter(|block| !has_parent.contains(block.id.as_str()))
        .collect();
    if heads.len() != 1 && problems.is_empty() {
        problems.push(if heads.is_empty() {
            "the blocks form a loop, so nothing starts the chain".to_owned()
        } else {
            format!(
                "{} blocks start a chain of their own ({}); connect them into one",
                heads.len(),
                heads
                    .iter()
                    .map(|block| block.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        });
    }

    if !problems.is_empty() {
        return Err(problems);
    }

    let mut chain: Vec<&PipelineBlock> = Vec::with_capacity(known.len());
    let mut walked: BTreeSet<&str> = BTreeSet::new();
    let mut cursor = Some(heads[0].id.as_str());
    while let Some(id) = cursor {
        if !walked.insert(id) {
            break;
        }
        chain.push(known[id]);
        cursor = next.get(id).copied();
    }
    if chain.len() != known.len() {
        let stranded: Vec<&str> = known
            .keys()
            .filter(|id| !walked.contains(*id))
            .copied()
            .collect();
        return Err(vec![format!(
            "{} is not connected to the chain",
            stranded.join(", ")
        )]);
    }

    problems.extend(chain_problems(&chain));
    if problems.is_empty() {
        Ok(chain)
    } else {
        Err(problems)
    }
}

/// What is wrong with the order these blocks are in.
fn chain_problems(chain: &[&PipelineBlock]) -> Vec<String> {
    let mut problems = Vec::new();

    if !matches!(chain[0].spec, BlockSpec::Source { .. }) {
        problems.push(format!(
            "the chain starts with {} ({}); it starts where the rows come from, which is a source block",
            chain[0].id,
            chain[0].spec.kind()
        ));
    }
    for block in &chain[1..] {
        if matches!(block.spec, BlockSpec::Source { .. }) {
            problems.push(format!(
                "{} is a second source; the rows come from one place and every later block reads what the one before it produced",
                block.id
            ));
        }
    }

    let mut seen_notebook: Option<&str> = None;
    let mut seen_view: Option<&str> = None;
    for block in chain {
        match block.spec {
            BlockSpec::Notebook { .. } => seen_notebook = Some(&block.id),
            BlockSpec::Transform { .. } => {
                if let Some(notebook) = seen_notebook {
                    problems.push(format!(
                        "{} is a Flow PHP block after the notebook {notebook}. A Flow step reads its rows from the query service's catalog, so it cannot be handed what a notebook produced — put every transform before the first notebook",
                        block.id
                    ));
                }
            }
            _ => {}
        }
        if let Some(view) = seen_view {
            problems.push(format!(
                "{} comes after the view {view}, which is the end of the chain",
                block.id
            ));
        }
        if matches!(block.spec, BlockSpec::View { .. }) {
            seen_view = Some(&block.id);
        }
    }

    problems
}

/// What is wrong with one block, whatever it is connected to.
pub(crate) fn block_problems(block: &PipelineBlock) -> Vec<String> {
    let mut problems = Vec::new();
    if block.title.len() > MAX_TITLE_BYTES {
        problems.push(format!(
            "{}'s title is {} bytes; the limit is {MAX_TITLE_BYTES}",
            block.id,
            block.title.len()
        ));
    }
    match &block.spec {
        BlockSpec::Source { dataset, arguments } => {
            if dataset.trim().is_empty() {
                problems.push(format!("{} does not say which dataset it reads", block.id));
            }
            let size = arguments.values().map(String::len).sum::<usize>();
            if size > MAX_SETTINGS_BYTES {
                problems.push(format!(
                    "{}'s read() arguments are {size} bytes; the limit is {MAX_SETTINGS_BYTES}",
                    block.id
                ));
            }
        }
        BlockSpec::Transform { steps } => {
            if steps.len() > MAX_PIPELINE_BYTES {
                problems.push(format!(
                    "{}'s Flow PHP steps are {} bytes; the limit is {MAX_PIPELINE_BYTES}",
                    block.id,
                    steps.len()
                ));
            }
        }
        BlockSpec::Notebook {
            notebook,
            revision,
            params,
        } => {
            if !is_notebook_name(notebook) {
                problems.push(format!(
                    "'{notebook}' is not a notebook name: lower-case letters, digits and underscores, starting with a letter"
                ));
            }
            if let Some(revision) = revision
                && !is_digest(revision)
            {
                problems.push(format!(
                    "{}'s pinned notebook revision is not a sha256 digest",
                    block.id
                ));
            }
            let size = serde_json::to_vec(params).map_or(0, |encoded| encoded.len());
            if size > MAX_SETTINGS_BYTES {
                problems.push(format!(
                    "{}'s settings are {size} bytes; the limit is {MAX_SETTINGS_BYTES}",
                    block.id
                ));
            }
        }
        BlockSpec::View { dataset } => {
            if let Some(dataset) = dataset
                && validate_name(dataset, "dataset").is_err()
            {
                problems.push(format!(
                    "{} publishes to '{dataset}', which is not a dataset name",
                    block.id
                ));
            }
        }
    }
    problems
}

fn check(blocks: &[PipelineBlock], edges: &[PipelineEdge]) -> Result<()> {
    order_of(blocks, edges)
        .map(|_| ())
        .map_err(RegistryError::Rejected)
}

fn is_block_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LENGTH
        && id
            .chars()
            .all(|value| value.is_ascii_lowercase() || value.is_ascii_digit() || value == '-')
}

fn is_notebook_name(name: &str) -> bool {
    let mut characters = name.chars();
    name.len() <= 64
        && characters
            .next()
            .is_some_and(|value| value.is_ascii_lowercase())
        && characters
            .all(|value| value.is_ascii_lowercase() || value.is_ascii_digit() || value == '_')
}

fn is_digest(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|character| character.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aiwatcher_prompts::adapters::memory::MemoryObjectStore;
    use std::sync::Arc;

    fn block(id: &str, spec: BlockSpec) -> PipelineBlock {
        PipelineBlock {
            id: id.to_owned(),
            title: id.to_owned(),
            position: BlockPosition::default(),
            spec,
        }
    }

    fn source() -> PipelineBlock {
        block(
            "corpus",
            BlockSpec::Source {
                dataset: "hub_rows".to_owned(),
                arguments: BTreeMap::from([(
                    "dataset".to_owned(),
                    "ai4privacy/pii-masking-200k".to_owned(),
                )]),
            },
        )
    }

    fn transform() -> PipelineBlock {
        block(
            "shape",
            BlockSpec::Transform {
                steps: "->limit(50)".to_owned(),
            },
        )
    }

    fn notebook() -> PipelineBlock {
        block(
            "detect",
            BlockSpec::Notebook {
                notebook: "pii_detection".to_owned(),
                revision: None,
                params: BTreeMap::new(),
            },
        )
    }

    fn view() -> PipelineBlock {
        block("result", BlockSpec::View { dataset: None })
    }

    fn edge(from: &str, to: &str) -> PipelineEdge {
        PipelineEdge {
            from: from.to_owned(),
            to: to.to_owned(),
        }
    }

    fn demo() -> SavePipelineRequest {
        SavePipelineRequest {
            name: "curation/pii-detection".to_owned(),
            description: "The end-to-end example".to_owned(),
            blocks: vec![source(), transform(), notebook(), view()],
            edges: vec![
                edge("corpus", "shape"),
                edge("shape", "detect"),
                edge("detect", "result"),
            ],
        }
    }

    fn problems(request: &SavePipelineRequest) -> Vec<String> {
        order_of(&request.blocks, &request.edges).expect_err("expected a refusal")
    }

    #[test]
    fn a_chain_is_ordered_from_its_head_whatever_order_the_blocks_arrived_in() {
        let mut request = demo();
        request.blocks.reverse();

        let chain = order_of(&request.blocks, &request.edges).unwrap();

        assert_eq!(
            chain
                .iter()
                .map(|block| block.id.as_str())
                .collect::<Vec<_>>(),
            ["corpus", "shape", "detect", "result"]
        );
    }

    #[test]
    fn a_block_that_feeds_two_is_refused_because_the_second_would_run_twice() {
        let mut request = demo();
        request
            .blocks
            .push(block("second", BlockSpec::View { dataset: None }));
        request.edges.push(edge("shape", "second"));

        let refusals = problems(&request);

        assert!(
            refusals
                .iter()
                .any(|problem| problem.contains("shape feeds more than one block")),
            "{refusals:?}"
        );
    }

    #[test]
    fn two_unconnected_chains_are_refused_naming_both_starts() {
        let mut request = demo();
        request.blocks.push(block(
            "orphan",
            BlockSpec::Transform {
                steps: "->limit(1)".to_owned(),
            },
        ));

        let refusals = problems(&request);

        assert!(
            refusals
                .iter()
                .any(|problem| problem.contains("corpus, orphan")),
            "{refusals:?}"
        );
    }

    #[test]
    fn a_flow_step_after_a_notebook_is_refused_with_the_reason_rather_than_reordered() {
        let request = SavePipelineRequest {
            blocks: vec![source(), notebook(), transform(), view()],
            edges: vec![
                edge("corpus", "detect"),
                edge("detect", "shape"),
                edge("shape", "result"),
            ],
            ..demo()
        };

        let refusals = problems(&request);

        assert!(
            refusals
                .iter()
                .any(|problem| problem.contains("cannot be handed what a notebook produced")),
            "{refusals:?}"
        );
    }

    #[test]
    fn a_chain_that_does_not_start_where_the_rows_come_from_is_refused() {
        let request = SavePipelineRequest {
            blocks: vec![transform(), notebook()],
            edges: vec![edge("shape", "detect")],
            ..demo()
        };

        let refusals = problems(&request);

        assert!(
            refusals
                .iter()
                .any(|problem| problem.contains("which is a source block")),
            "{refusals:?}"
        );
    }

    #[test]
    fn nothing_may_follow_the_view_that_ends_the_chain() {
        let request = SavePipelineRequest {
            blocks: vec![source(), view(), notebook()],
            edges: vec![edge("corpus", "result"), edge("result", "detect")],
            ..demo()
        };

        let refusals = problems(&request);

        assert!(
            refusals
                .iter()
                .any(|problem| problem.contains("comes after the view result")),
            "{refusals:?}"
        );
    }

    #[test]
    fn a_loop_is_refused_as_a_chain_that_never_starts() {
        let request = SavePipelineRequest {
            blocks: vec![source(), transform()],
            edges: vec![edge("corpus", "shape"), edge("shape", "corpus")],
            ..demo()
        };

        let refusals = problems(&request);

        assert!(
            refusals
                .iter()
                .any(|problem| problem.contains("form a loop")),
            "{refusals:?}"
        );
    }

    #[test]
    fn every_problem_is_reported_at_once_rather_than_one_per_attempt() {
        let request = SavePipelineRequest {
            blocks: vec![
                block("Corpus", BlockSpec::View { dataset: None }),
                block(
                    "detect",
                    BlockSpec::Notebook {
                        notebook: "Not A Name".to_owned(),
                        revision: Some("nope".to_owned()),
                        params: BTreeMap::new(),
                    },
                ),
            ],
            edges: vec![edge("missing", "detect")],
            ..demo()
        };

        let refusals = problems(&request);

        assert!(refusals.len() >= 4, "{refusals:?}");
    }

    #[tokio::test]
    async fn saving_the_same_pipeline_is_idempotent_and_an_edit_is_a_revision() {
        let registry = Registry::new(Arc::new(MemoryObjectStore::new()), "datasets");

        let first = registry.save_pipeline(demo()).await.unwrap();
        let same = registry.save_pipeline(demo()).await.unwrap();
        let mut moved = demo();
        moved.blocks[0].position = BlockPosition { x: 40.0, y: 10.0 };
        let moved = registry.save_pipeline(moved).await.unwrap();

        assert!(first.created);
        assert!(!same.created);
        assert_ne!(
            first.pipeline.revision, moved.pipeline.revision,
            "where a block sits is part of what was saved"
        );
        let page = registry.pipelines().await.unwrap();
        assert_eq!(page.pipelines.len(), 1);
        assert_eq!(page.pipelines[0].revision, moved.pipeline.revision);
    }

    #[tokio::test]
    async fn a_refused_pipeline_is_not_stored_at_all() {
        let registry = Registry::new(Arc::new(MemoryObjectStore::new()), "datasets");
        let mut request = demo();
        request.edges.clear();

        let refusal = registry.save_pipeline(request).await;

        assert!(matches!(refusal, Err(RegistryError::Rejected(_))));
        assert!(registry.pipelines().await.unwrap().pipelines.is_empty());
    }
}
