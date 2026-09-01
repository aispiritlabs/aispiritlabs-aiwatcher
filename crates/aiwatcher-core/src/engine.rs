//! The orchestration engine: what can be started, and starting one.
//!
//! Everything else in this crate describes what *happened*. This module
//! describes what somebody could *ask to happen*, which is a different kind of
//! fact and belongs to a different system — Flyte in this deployment, and the
//! whole point of a port is that it need not be.
//!
//! ## Why this is not the workflow catalog
//!
//! [`crate::catalog`] and the projector's `/workflows` answer "which graphs
//! has this instance seen?", folded from `workflow.declared` on the log. This
//! port answers "which graphs could I start right now?", and the two sets
//! overlap only by coincidence: a workflow declared last week may have been
//! deleted from the orchestrator, and a launch plan registered this morning
//! has never published an event. Merging them would produce a picker that
//! offers things nothing can run and hides things nobody has run yet.
//!
//! ADR_0012 rejected reading Flyte's API *for the shape of a graph*, and this
//! does not reverse it: the shape still comes from the declaration on the log,
//! because that is the only source that is right when the orchestrator is
//! bypassed. What comes from the engine is the launchable inventory and its
//! input interface, which the log cannot know — nothing publishes an event
//! about a workflow nobody has run.
//!
//! ## What aiwatcher supplies and what it never supplies
//!
//! A launch carries names and values a caller chose: which registered entity,
//! and what to bind its declared inputs to. It never carries an endpoint, a
//! container image, a command, or anything else describing *how* to run
//! something — see [`WorkflowEngine`] and ADR_0016. The engine's address is
//! configuration, exactly as the rerun target is.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::ports::{PortError, PortResult};

/// The kind of registered thing a launch names.
///
/// Flyte's launchable unit is a launch plan; a task and a workflow are
/// registered entities that a launch plan points at. Other engines divide this
/// differently, so the kind travels with the reference rather than being
/// assumed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    #[default]
    LaunchPlan,
    Task,
    Workflow,
}

impl EntityKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LaunchPlan => "lp",
            Self::Task => "task",
            Self::Workflow => "wf",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "lp" | "launch_plan" => Some(Self::LaunchPlan),
            "task" => Some(Self::Task),
            "wf" | "workflow" => Some(Self::Workflow),
            _ => None,
        }
    }
}

/// Where a launchable entity lives, as one string that survives a URL path.
///
/// `lp:planner:production:house.import:v3`. Colon rather than slash on
/// purpose: these five parts have to travel as a single path segment in
/// `/api/v1/engine/workflows/{id}`, and an id containing a slash turns one
/// route into an ambiguous prefix match. Flyte validates project, domain,
/// name and version against `[a-zA-Z0-9_\-.]`, so a colon cannot occur inside
/// a part and the split is unambiguous.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct EngineRef {
    pub kind: EntityKind,
    pub project: String,
    pub domain: String,
    pub name: String,
    /// Absent means "whatever the engine considers current". A launch that
    /// pins no version is a launch whose meaning changes under it, which is
    /// occasionally what an operator wants and never what a comparison wants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// A reference that could not be read.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is not an engine reference; expected kind:project:domain:name[:version]")]
pub struct BadEngineRef(pub String);

impl EngineRef {
    /// Rejects a part that would make the rendering ambiguous or the request
    /// forgeable. Nothing downstream re-checks this: the reference is
    /// interpolated into the engine's own URLs, so a part carrying `/` or `..`
    /// is a path-traversal primitive pointed at the orchestrator.
    fn valid_part(part: &str) -> bool {
        !part.is_empty()
            && part.len() <= 255
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
            && part != "."
            && part != ".."
    }

    /// Whether every part is safe to interpolate into an engine URL.
    #[must_use]
    pub fn is_well_formed(&self) -> bool {
        Self::valid_part(&self.project)
            && Self::valid_part(&self.domain)
            && Self::valid_part(&self.name)
            && self.version.as_deref().is_none_or(Self::valid_part)
    }

    /// The single-segment rendering used in URLs and in `EngineWorkflow::id`.
    #[must_use]
    pub fn render(&self) -> String {
        let mut rendered = format!(
            "{}:{}:{}:{}",
            self.kind.as_str(),
            self.project,
            self.domain,
            self.name
        );
        if let Some(version) = &self.version {
            rendered.push(':');
            rendered.push_str(version);
        }
        rendered
    }
}

impl std::str::FromStr for EngineRef {
    type Err = BadEngineRef;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let bad = || BadEngineRef(value.to_owned());
        let mut parts = value.split(':');
        let kind = parts.next().and_then(EntityKind::parse).ok_or_else(bad)?;
        let project = parts.next().ok_or_else(bad)?.to_owned();
        let domain = parts.next().ok_or_else(bad)?.to_owned();
        let name = parts.next().ok_or_else(bad)?.to_owned();
        let version = parts.next().map(ToOwned::to_owned);
        if parts.next().is_some() {
            return Err(bad());
        }
        let reference = Self {
            kind,
            project,
            domain,
            name,
            version,
        };
        if !reference.is_well_formed() {
            return Err(bad());
        }
        Ok(reference)
    }
}

impl std::fmt::Display for EngineRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.render())
    }
}

/// Which part of the feature/training/inference cycle a workflow belongs to.
///
/// A **hint**, and named as one everywhere it is rendered. It is derived from
/// the entity's own name and description, which is a guess: an orchestrator
/// has no field that says "this one produces datasets". The value is that a
/// picker in Data Curation can default to curation workflows instead of
/// listing every launch plan in the cluster; the cost of being wrong is a
/// filter somebody switches off, which is why nothing but presentation may
/// depend on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PipelineStage {
    /// Producing or refreshing a dataset: curation, feature extraction, ingest.
    Curation,
    /// Changing the model: training, fine-tuning, distillation.
    Training,
    /// Measuring one: evaluation, benchmarking, scoring.
    Evaluation,
    /// Serving or batch-scoring with one: inference, prediction, embedding.
    Inference,
}

impl PipelineStage {
    /// Every stage, in the order the cycle runs.
    pub const ALL: [Self; 4] = [
        Self::Curation,
        Self::Training,
        Self::Evaluation,
        Self::Inference,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Curation => "curation",
            Self::Training => "training",
            Self::Evaluation => "evaluation",
            Self::Inference => "inference",
        }
    }

    /// The keyword lists behind the hint, in one place so a reader can see
    /// exactly how weak the inference is.
    #[must_use]
    pub fn guess(text: &str) -> Option<Self> {
        const KEYWORDS: [(PipelineStage, &[&str]); 4] = [
            (
                PipelineStage::Curation,
                &[
                    "curat", "dataset", "ingest", "feature", "etl", "extract", "prepar", "clean",
                    "label", "annotat",
                ],
            ),
            (
                PipelineStage::Training,
                &[
                    "train",
                    "finetune",
                    "fine_tune",
                    "fine-tune",
                    "sft",
                    "lora",
                    "distill",
                    "pretrain",
                ],
            ),
            (
                PipelineStage::Evaluation,
                &["eval", "benchmark", "score", "judge", "validat"],
            ),
            (
                PipelineStage::Inference,
                &[
                    "infer",
                    "predict",
                    "serve",
                    "score_batch",
                    "embed",
                    "generat",
                    "batch_score",
                ],
            ),
        ];
        let text = text.to_ascii_lowercase();
        // First match in cycle order, so `train_and_eval` reads as training —
        // the earlier stage is the one a name usually leads with.
        KEYWORDS
            .iter()
            .find(|(_, keywords)| keywords.iter().any(|keyword| text.contains(keyword)))
            .map(|(stage, _)| *stage)
    }
}

impl std::str::FromStr for PipelineStage {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|stage| stage.as_str() == value.to_ascii_lowercase())
            .ok_or_else(|| format!("{value:?} is not a pipeline stage"))
    }
}

/// The shape of one declared input, for a form to render.
///
/// Display only. Whatever a caller sends is bound to the engine's *own*
/// declared type at launch time, read from the engine at that moment — see
/// [`WorkflowEngine::launch`]. A panel that has been open since before a
/// redeploy is therefore rendering a stale form against a fresh interface, and
/// the launch fails with the engine's own message rather than binding a value
/// to a type nobody checked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct EngineParameter {
    pub name: String,
    pub kind: ParameterKind,
    /// Required *and* without a default. A parameter with a default is
    /// optional however the engine phrases it.
    pub required: bool,
    /// The default, rendered as JSON. `None` means there is none, which for a
    /// required parameter is the normal case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Object)]
    pub default: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// The engine's own name for the type, for the cases [`ParameterKind`]
    /// flattens: a blob, a structured dataset, a union. Shown beside the
    /// field so a `Json` box is not a mystery.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub type_name: String,
    /// The permitted values, when the engine declared a closed set. Turns a
    /// text box into a select, which is the difference between a filter
    /// somebody types wrong and one they pick.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enum_values: Vec<String>,
}

/// How a form should render one input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ParameterKind {
    String,
    Integer,
    Float,
    Boolean,
    /// An instant. Rendered as a date-time control and sent as RFC 3339, which
    /// is what makes "a range" a first-class thing a picker can fill in.
    Datetime,
    /// A span. Sent as seconds; the adapter renders the engine's own spelling.
    Duration,
    /// A closed set — see [`EngineParameter::enum_values`].
    Enum,
    /// A list of something. Sent as a JSON array.
    Collection,
    /// A string-keyed map. Sent as a JSON object.
    Map,
    /// Anything else the engine declared: a struct, a blob, a dataframe. Sent
    /// as JSON and bound by the engine's own rules.
    Json,
}

/// One thing a caller could start.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct EngineWorkflow {
    /// [`EngineRef::render`] — what `POST /launches` and the detail route take.
    pub id: String,
    /// The registered name, without project or domain.
    pub name: String,
    pub project: String,
    pub domain: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
    pub kind: EntityKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// A guess, never a fact — see [`PipelineStage`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_hint: Option<PipelineStage>,
    pub parameters: Vec<EngineParameter>,
    /// Whether the engine considers this version launchable. An inactive
    /// launch plan is listed rather than hidden, because "it is there and it
    /// is switched off" is the answer somebody is looking for when they cannot
    /// find it.
    pub active: bool,
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub updated_at: Option<OffsetDateTime>,
    /// Where to see it in the engine's own console, when one is configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// What a caller is asking the catalog for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CatalogQuery {
    /// Case-insensitive substring over name and description.
    pub search: Option<String>,
    /// Overrides the configured project and domain. A deployment that watches
    /// one namespace still has a staging domain worth launching into.
    pub project: Option<String>,
    pub domain: Option<String>,
    pub stage: Option<PipelineStage>,
    pub limit: usize,
    /// The engine's own continuation token, passed back verbatim.
    pub token: Option<String>,
}

/// A page of launchable things.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct EngineCatalog {
    pub workflows: Vec<EngineWorkflow>,
    /// Absent on the last page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_token: Option<String>,
}

/// What is being asked of the engine.
///
/// Note what is absent, and it is the same absence as [`crate::ports::RerunRequest`]:
/// no endpoint, no image, no command. `workflow` names something the engine
/// already holds, and `inputs` binds the parameters that entity itself
/// declared. A request that could describe *how* to run something would make
/// aiwatcher a way to execute arbitrary work inside the cluster on the word of
/// whoever reached the API.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct LaunchRequest {
    /// [`EngineRef::render`].
    pub workflow: String,
    /// Parameter name to value. Values are JSON and are bound to the types the
    /// engine declares, at launch time.
    #[serde(default)]
    #[schema(value_type = Object)]
    pub inputs: BTreeMap<String, serde_json::Value>,
    /// The id the events this execution publishes are expected to carry, so
    /// the panel can follow a launch it just made without waiting for a
    /// producer to be discovered. Generated by the API when the caller does
    /// not supply one — see `aiwatcher_api::engine`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run_id: Option<String>,
    /// Who asked, for the engine's own audit trail. Never a permission: the
    /// engine's credentials are aiwatcher's, and the role check happened in
    /// the handler.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub requested_by: String,
}

/// What came back. Not a result — nothing has finished.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LaunchAccepted {
    /// The engine's name for the execution it just created.
    pub reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Echoed back so the caller can subscribe to
    /// `/api/v1/workflow-executions/{id}/stream` immediately — before the
    /// producer has published anything, which is the interesting part of a
    /// launch's first thirty seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run_id: Option<String>,
}

/// Where an execution has got to, as the engine sees it.
///
/// aiwatcher's own view of a run comes from the log and says something
/// different: `RunStatus` is what the *producer* reported. When they disagree
/// the disagreement is the finding — an execution the orchestrator calls
/// `Failed` whose events stop mid-run is a pod that was killed, and one it
/// calls `Succeeded` with no events at all is a producer that is not
/// instrumented.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct EngineExecution {
    pub reference: String,
    pub phase: EnginePhase,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub started_at: Option<OffsetDateTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Read back from the execution's labels, when aiwatcher set one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnginePhase {
    Queued,
    Running,
    Succeeded,
    Failed,
    Aborted,
    /// The engine said something this adapter does not model. Kept distinct
    /// from `Running` so a panel does not draw a spinner for a state nobody
    /// understood.
    Unknown,
}

/// How this instance is wired, for a client deciding what to render.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct EngineDescription {
    /// `flyte` today. A name, not a version: the panel branches on nothing.
    pub kind: String,
    pub project: String,
    pub domain: String,
    /// The engine's console, when one is configured, so the panel can link out
    /// rather than pretending to be it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub console_url: Option<String>,
}

/// Reading an orchestrator's inventory, and starting one of its entries.
///
/// The second port here that makes something happen rather than recording
/// that it did — [`crate::ports::WorkflowRunner`] is the first — and it
/// carries the same two rules. The engine's address is **configuration**: it
/// never comes from an event, from a request body, or from anything a
/// producer can write, because aiwatcher runs inside the cluster and a
/// caller-supplied URL is a request to reach that cluster's network. And its
/// absence is a **501**: a null engine that acknowledged launches nobody ran
/// would be worse than no engine at all.
#[async_trait]
pub trait WorkflowEngine: Send + Sync + std::fmt::Debug {
    /// Which engine, and the project and domain it defaults to.
    fn describe(&self) -> EngineDescription;

    /// One page of launchable entities.
    async fn catalog(&self, query: &CatalogQuery) -> PortResult<EngineCatalog>;

    /// One entity and its input interface, or `None` when the engine has no
    /// such thing. `None` rather than an error: asking about a launch plan
    /// that was deleted is a 404, not a broken orchestrator.
    async fn workflow(&self, reference: &EngineRef) -> PortResult<Option<EngineWorkflow>>;

    /// Start one.
    ///
    /// Implementations read the entity's declared interface *now* rather than
    /// trusting the shape a caller was rendering, and bind each input to the
    /// declared type. An input the entity does not declare is a rejection, not
    /// a passthrough: an engine that ignores unknown fields turns a typo in a
    /// filter into a run over everything.
    async fn launch(&self, request: LaunchRequest) -> PortResult<LaunchAccepted>;

    /// Where one execution has got to, or `None` when the engine has no record
    /// of it.
    async fn execution(&self, reference: &str) -> PortResult<Option<EngineExecution>>;
}

/// A launch the adapter refused before anything left the process.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LaunchError {
    #[error("{0}")]
    Invalid(String),
    #[error("{workflow} declares no input named {parameter:?}")]
    UnknownInput { workflow: String, parameter: String },
    #[error("{parameter} is required and was not supplied")]
    MissingInput { parameter: String },
    #[error("{parameter} expects {expected} and got {got}")]
    WrongType {
        parameter: String,
        expected: String,
        got: String,
    },
}

impl From<LaunchError> for PortError {
    /// Always `Rejected`, and that classification is the point: a launch the
    /// adapter refused will be refused identically next time, so the API
    /// answers 4xx rather than inviting a retry that cannot work.
    fn from(error: LaunchError) -> Self {
        Self::Rejected {
            target: "workflow-engine",
            message: error.to_string(),
        }
    }
}

impl From<BadEngineRef> for PortError {
    fn from(error: BadEngineRef) -> Self {
        Self::Rejected {
            target: "workflow-engine",
            message: error.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn a_reference_survives_a_round_trip_through_one_path_segment() {
        let reference = EngineRef {
            kind: EntityKind::LaunchPlan,
            project: "planner".to_owned(),
            domain: "production".to_owned(),
            name: "house.import".to_owned(),
            version: Some("v3".to_owned()),
        };
        let rendered = reference.render();
        assert_eq!(rendered, "lp:planner:production:house.import:v3");
        assert!(!rendered.contains('/'), "it has to be one path segment");
        assert_eq!(EngineRef::from_str(&rendered), Ok(reference));
    }

    #[test]
    fn a_reference_without_a_version_means_whatever_is_current() {
        let reference = EngineRef::from_str("lp:planner:production:house.import").expect("reads");
        assert_eq!(reference.version, None);
        assert_eq!(reference.render(), "lp:planner:production:house.import");
    }

    #[test]
    fn a_reference_carrying_a_path_is_refused() {
        // Every part is interpolated into the orchestrator's own URLs. A name
        // holding `../` would be a path traversal aimed at a system aiwatcher
        // authenticates to, which is a worse failure than a 400.
        for hostile in [
            "lp:planner:production:../../admin",
            "lp:planner:production:name/with/slash",
            "lp:planner:production:..",
            "lp::production:name",
            "lp:planner:production:name:v1:extra",
            "sh:planner:production:name",
            "planner:production:name",
        ] {
            assert_eq!(
                EngineRef::from_str(hostile),
                Err(BadEngineRef(hostile.to_owned())),
                "{hostile} should not parse"
            );
        }
    }

    #[test]
    fn the_stage_hint_reads_a_name_and_admits_when_it_cannot() {
        assert_eq!(
            PipelineStage::guess("house_dataset_curation"),
            Some(PipelineStage::Curation)
        );
        assert_eq!(
            PipelineStage::guess("llama_finetune_v2"),
            Some(PipelineStage::Training)
        );
        assert_eq!(
            PipelineStage::guess("nightly_eval_suite"),
            Some(PipelineStage::Evaluation)
        );
        assert_eq!(
            PipelineStage::guess("batch_inference"),
            Some(PipelineStage::Inference)
        );
        // The honest answer for a name that says nothing. A default of
        // `Curation` here would fill the Data Curation picker with every
        // unnamed launch plan in the cluster.
        assert_eq!(PipelineStage::guess("wf_7"), None);
    }

    #[test]
    fn a_refused_launch_is_never_retryable() {
        let error: PortError = LaunchError::MissingInput {
            parameter: "since".to_owned(),
        }
        .into();
        assert!(
            !error.is_retryable(),
            "a launch missing an input will still be missing it next time"
        );
    }
}
