//! Workflow graphs: the shape an orchestration declared, and what a traversal
//! of it actually did.
//!
//! Everything else in this crate folds *what happened*. This module folds one
//! thing that did not: the topology. That is the whole reason it exists.
//!
//! ```text
//! workflow.declared   → the catalog: nodes and edges, before anything runs
//! step.*              → a node's execution, and its span (see `crate::spans`)
//! artifact.produced   → what a node handed on, by reference
//! agent.message       → one agent addressing another
//! run.*               → which runs a traversal is made of
//! ```
//!
//! Three rules carry the meaning.
//!
//! * **A node that never ran is `Pending`, and that is only expressible
//!   because something declared it.** A projection over observed events alone
//!   can answer "what has this done" and can never answer "what has it not
//!   done yet" — which is the question somebody watching a workflow is
//!   actually asking. The declaration is what turns a list of finished stages
//!   into a graph with a front edge.
//!
//! * **An execution is not a run.** A stage-per-pod orchestrator gives every
//!   stage its own process, so one traversal is four runs joined by
//!   `workflow_run_id` (see [`aiwatcher_core::EventEnvelope::workflow_run`]).
//!   A workflow that runs start to finish in one process is its own execution
//!   and nobody has to know the difference.
//!
//! * **A node the declaration never mentioned is kept, and flagged.** Dropping
//!   it would hide the one case worth seeing: a graph that has drifted from
//!   the code that runs it. [`NodeState::declared`] is how the panel tells the
//!   two apart.
//!
//! Bounded like every other projection here, and by the same two-tier rule:
//! detail is shed before whole executions are dropped, and a running execution
//! is never the one evicted — it is the one somebody is watching.

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use aiwatcher_core::{Checkpoint, EventType, Phase, RecordedEvent, SpanId, Subject};

// ── What a producer declared ─────────────────────────────────────────────────

/// One node of a declared graph.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct WorkflowNode {
    pub id: String,
    pub name: String,
    /// What kind of work it is — `chain`, `retriever`, `agent`, whatever the
    /// producer calls it. Free text on purpose: the same reason
    /// `data.step_type` is free text, so a new kind is not a backend release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The agent expected to run it, when the graph names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

/// One declared edge. Direction is `from → to`.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, utoipa::ToSchema,
)]
pub struct WorkflowEdge {
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// A workflow as the catalog holds it.
#[derive(Clone, Debug, PartialEq, Serialize, utoipa::ToSchema)]
pub struct WorkflowDefinition {
    pub workflow_id: String,
    pub name: String,
    /// Content hash of the declared topology, chosen by the producer.
    ///
    /// Not interpreted here beyond equality: a changed version replaces the
    /// stored shape. It exists so re-declaring on every execution — which is
    /// what keeps the catalog alive across retention eviction — costs nothing
    /// when the shape has not moved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub nodes: Vec<WorkflowNode>,
    pub edges: Vec<WorkflowEdge>,
    #[serde(with = "time::serde::rfc3339")]
    pub declared_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub last_activity_at: OffsetDateTime,
    pub executions: u64,
    pub running: u64,
    pub succeeded: u64,
    pub failed: u64,
}

// ── What a traversal did ─────────────────────────────────────────────────────

/// Where one node of one execution got to.
///
/// Four states rather than three, and the extra one is the point: `Pending` is
/// a node the graph declares and nothing has started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    #[default]
    Pending,
    Running,
    Succeeded,
    Failed,
}

/// Where a whole traversal got to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    #[default]
    Running,
    Succeeded,
    Failed,
}

/// Something a node produced, by reference.
///
/// The bytes stay wherever the producer put them. aiwatcher keeps the pointer
/// because a pointer is bounded and a floor-plan PDF is not — see the
/// artifact guardrail in `CLAUDE.md`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Artifact {
    pub name: String,
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<i64>,
    /// Content hash, when the producer computed one. What makes "the same
    /// artifact as last time" a checkable claim rather than a guess.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub produced_at: OffsetDateTime,
}

/// One agent addressing another.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AgentMessage {
    pub from: String,
    pub to: String,
    /// `handoff`, `request`, `response`, `broadcast` — or whatever else a
    /// producer coins. Free text for the same reason `step_type` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The channel it went over, when there is one worth naming: a queue, a
    /// topic, a shared file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// The node it was sent from, when the sender was inside one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

/// One node of one execution.
#[derive(Clone, Debug, PartialEq, Serialize, utoipa::ToSchema)]
pub struct NodeState {
    pub node_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub status: NodeStatus,
    /// Whether the declared graph mentions this node.
    ///
    /// `false` means something ran that the declaration does not know about,
    /// which is a stale declaration and worth seeing rather than hiding.
    pub declared: bool,
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub started_at: Option<OffsetDateTime>,
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub ended_at: Option<OffsetDateTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    /// How many times this node was started.
    ///
    /// Counted by distinct span key, not by event, so a redelivery does not
    /// invent a retry that never happened.
    pub attempts: u32,
    pub agents: Vec<String>,
    pub artifacts: Vec<Artifact>,
    /// The run the node executed in. A stage-per-pod orchestrator gives every
    /// node a different one; this is what links a node back to its trace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span_id: Option<SpanId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl NodeState {
    fn declared_from(node: &WorkflowNode) -> Self {
        Self {
            node_id: node.id.clone(),
            name: node.name.clone(),
            kind: node.kind.clone(),
            status: NodeStatus::Pending,
            declared: true,
            started_at: None,
            ended_at: None,
            duration_ms: None,
            attempts: 0,
            agents: Vec::new(),
            artifacts: Vec::new(),
            run_id: None,
            span_id: None,
            error: None,
        }
    }

    fn observed(node_id: String) -> Self {
        Self {
            name: node_id.clone(),
            node_id,
            kind: None,
            status: NodeStatus::Pending,
            declared: false,
            started_at: None,
            ended_at: None,
            duration_ms: None,
            attempts: 0,
            agents: Vec::new(),
            artifacts: Vec::new(),
            run_id: None,
            span_id: None,
            error: None,
        }
    }
}

/// One row in the executions list.
#[derive(Clone, Debug, PartialEq, Serialize, utoipa::ToSchema)]
pub struct ExecutionSummary {
    pub workflow_run_id: String,
    pub workflow_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub status: ExecutionStatus,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    /// The newest event folded into this execution, ended or not.
    ///
    /// An execution outlives its runs, and a stage-per-pod one is idle between
    /// them by design, so "started three hours ago" says nothing about whether
    /// anything is still moving. This does. Same fact as `RunSummary`'s field
    /// of the same shape, and it is what the time window matches on.
    #[serde(with = "time::serde::rfc3339")]
    pub last_activity_at: OffsetDateTime,
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub ended_at: Option<OffsetDateTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    pub nodes_total: u64,
    pub nodes_pending: u64,
    pub nodes_running: u64,
    pub nodes_succeeded: u64,
    pub nodes_failed: u64,
    /// Every agent seen anywhere in the traversal, in first-seen order.
    pub agents: Vec<String>,
    pub artifacts: u64,
    pub messages: u64,
    /// The runs this traversal is made of. One for a single-process workflow,
    /// one per stage for a stage-per-pod one.
    pub runs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The newest checkpoint folded into this row, so a client can resume the
    /// live stream from here without re-reading the execution.
    pub last_checkpoint: Checkpoint,
}

/// One execution, with everything needed to draw it.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct ExecutionDetail {
    pub summary: ExecutionSummary,
    /// In declaration order where declared, then first-seen order for the rest.
    pub nodes: Vec<NodeState>,
    /// The declared edges of the workflow this executes.
    pub edges: Vec<WorkflowEdge>,
    /// Messages actually observed. Never merged with `edges`: sequence is not
    /// communication, and a view that conflated them would claim a handoff
    /// that nothing recorded.
    pub messages: Vec<AgentMessage>,
    /// Older messages were shed under memory pressure. The count in
    /// `summary.messages` is still complete.
    pub messages_truncated: bool,
}

// ── Filters and pages ────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct WorkflowFilter {
    /// Only graphs with activity in the last this-many seconds. See
    /// [`crate::window`]. A declaration is not evicted by it — the catalog
    /// keeps the shape, the window decides what is shown.
    pub window_seconds: Option<i64>,
    /// Substring over the id and the name.
    pub search: Option<String>,
    /// Cursor: the last workflow id on the previous page. Exclusive.
    pub after: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct WorkflowPage {
    pub workflows: Vec<WorkflowDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub total_known: usize,
}

#[derive(Clone, Debug, Default, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct ExecutionFilter {
    /// Only executions with activity in the last this-many seconds. See
    /// [`crate::window`].
    pub window_seconds: Option<i64>,
    pub workflow_id: Option<String>,
    pub status: Option<ExecutionStatus>,
    /// Substring over the execution id, the workflow id and the agents.
    pub search: Option<String>,
    /// Cursor: the last execution id on the previous page. Exclusive.
    pub after: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct ExecutionPage {
    pub executions: Vec<ExecutionSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub total_known: usize,
}

// ── Caps ─────────────────────────────────────────────────────────────────────

/// What the workflow projection may hold.
///
/// Per-item caps and global totals, because a per-item cap alone is an
/// exposure rather than a bound: `max_executions × max_nodes_per_execution` is
/// the real number, and the totals are what make the footprint predictable.
/// Same reasoning as [`crate::evaluations::EvaluationConfig`], which says it at
/// length.
#[derive(Clone, Copy, Debug)]
pub struct WorkflowConfig {
    /// Distinct workflows in the catalog. A deployment with more graphs than
    /// this has a naming problem, not a capacity one.
    pub max_definitions: usize,
    pub max_executions: usize,
    /// A graph wider than this is past what anyone can read on a canvas.
    pub max_nodes_per_execution: usize,
    pub max_artifacts_total: usize,
    pub max_messages_per_execution: usize,
    pub max_messages_total: usize,
}

impl Default for WorkflowConfig {
    fn default() -> Self {
        Self {
            max_definitions: 200,
            max_executions: 1_000,
            max_nodes_per_execution: 200,
            max_artifacts_total: 20_000,
            max_messages_per_execution: 500,
            max_messages_total: 50_000,
        }
    }
}

// ── The fold ─────────────────────────────────────────────────────────────────

/// One execution as it is held in memory.
#[derive(Clone, Debug)]
struct Held {
    summary: ExecutionSummary,
    /// Node order, so the graph draws the same way twice. Declared nodes in
    /// declaration order, then observed ones in first-seen order.
    node_order: Vec<String>,
    nodes: HashMap<String, NodeState>,
    edges: Vec<WorkflowEdge>,
    messages: Vec<AgentMessage>,
    messages_truncated: bool,
    /// Span keys already counted as an attempt, per node. Distinct keys are
    /// distinct attempts; a redelivery reuses one.
    attempts: HashMap<String, BTreeSet<String>>,
    /// Artifact uris already recorded, so a redelivery does not double-list.
    artifact_uris: BTreeSet<String>,
    /// Message ids already recorded, for the same reason.
    message_ids: BTreeSet<String>,
    /// Every run any event of this execution arrived on.
    ///
    /// Kept apart from the two below because a producer that emits `step.*`
    /// without a run lifecycle still ran in a process worth naming — the node
    /// links back to its trace through it.
    seen_runs: BTreeSet<String>,
    /// Runs started and not yet ended. An execution with one of these open is
    /// still running, whatever its nodes say.
    open_runs: BTreeSet<String>,
    finished_runs: BTreeSet<String>,
}

impl Held {
    fn new(event: &RecordedEvent, workflow_run_id: String, workflow_id: String) -> Self {
        Self {
            summary: ExecutionSummary {
                workflow_run_id,
                workflow_id,
                version: None,
                status: ExecutionStatus::Running,
                started_at: event.metadata.occurred_at,
                last_activity_at: event.metadata.occurred_at,
                ended_at: None,
                duration_ms: None,
                nodes_total: 0,
                nodes_pending: 0,
                nodes_running: 0,
                nodes_succeeded: 0,
                nodes_failed: 0,
                agents: Vec::new(),
                artifacts: 0,
                messages: 0,
                runs: Vec::new(),
                error: None,
                last_checkpoint: Checkpoint::beginning(),
            },
            node_order: Vec::new(),
            nodes: HashMap::new(),
            edges: Vec::new(),
            messages: Vec::new(),
            messages_truncated: false,
            attempts: HashMap::new(),
            artifact_uris: BTreeSet::new(),
            message_ids: BTreeSet::new(),
            seen_runs: BTreeSet::new(),
            open_runs: BTreeSet::new(),
            finished_runs: BTreeSet::new(),
        }
    }

    fn node_mut(&mut self, node_id: &str, max_nodes: usize) -> Option<&mut NodeState> {
        if !self.nodes.contains_key(node_id) {
            if self.nodes.len() >= max_nodes {
                return None;
            }
            self.node_order.push(node_id.to_owned());
            self.nodes
                .insert(node_id.to_owned(), NodeState::observed(node_id.to_owned()));
        }
        self.nodes.get_mut(node_id)
    }

    /// Apply a declared shape: seed the nodes nothing has started yet, and
    /// promote any that already ran to `declared`.
    fn adopt(&mut self, definition: &WorkflowDefinition, max_nodes: usize) {
        self.summary.version = definition.version.clone();
        self.edges = definition.edges.clone();
        for node in &definition.nodes {
            if let Some(existing) = self.nodes.get_mut(&node.id) {
                existing.declared = true;
                existing.name = node.name.clone();
                if existing.kind.is_none() {
                    existing.kind = node.kind.clone();
                }
                continue;
            }
            if self.nodes.len() >= max_nodes {
                break;
            }
            self.node_order.push(node.id.clone());
            self.nodes
                .insert(node.id.clone(), NodeState::declared_from(node));
        }
        // Declared nodes lead, in declaration order; whatever ran without being
        // declared follows. Ordering the canvas by when a producer happened to
        // mention something would move the graph under the reader.
        let declared: Vec<String> = definition
            .nodes
            .iter()
            .map(|node| node.id.clone())
            .collect();
        let mut ordered = declared.clone();
        for id in &self.node_order {
            if !declared.contains(id) {
                ordered.push(id.clone());
            }
        }
        ordered.retain(|id| self.nodes.contains_key(id));
        self.node_order = ordered;
    }

    fn note_agent(&mut self, agent: &str) {
        if !agent.is_empty() && !self.summary.agents.iter().any(|known| known == agent) {
            self.summary.agents.push(agent.to_owned());
        }
    }

    /// Recompute everything derived from the nodes and the runs.
    ///
    /// Cheap — a graph is capped at [`WorkflowConfig::max_nodes_per_execution`]
    /// — and always right, which incremental counters kept across four event
    /// types would not be.
    fn recount(&mut self) {
        let mut pending = 0;
        let mut running = 0;
        let mut succeeded = 0;
        let mut failed = 0;
        for node in self.nodes.values() {
            match node.status {
                NodeStatus::Pending => pending += 1,
                NodeStatus::Running => running += 1,
                NodeStatus::Succeeded => succeeded += 1,
                NodeStatus::Failed => failed += 1,
            }
        }
        self.summary.nodes_total = self.nodes.len() as u64;
        self.summary.nodes_pending = pending;
        self.summary.nodes_running = running;
        self.summary.nodes_succeeded = succeeded;
        self.summary.nodes_failed = failed;
        self.summary.artifacts = self.artifact_uris.len() as u64;
        self.summary.messages = self.message_ids.len() as u64;
        self.summary.runs = self.seen_runs.iter().cloned().collect();

        // Three clauses, each checkable on its own:
        //
        //   failed    — anything in it failed;
        //   running   — a run is open, or a node is running;
        //   succeeded — nothing failed, nothing is open, and something finished.
        //
        // A node the traversal skipped stays `Pending` and does not hold the
        // execution open. A branch not taken is not a stall, and treating it as
        // one would leave every conditional workflow running forever. The
        // pending count is reported instead, so "4 of 5 ran" is visible rather
        // than implied.
        let anything_failed = failed > 0 || self.summary.error.is_some();
        let anything_open = !self.open_runs.is_empty() || running > 0;
        let anything_finished = !self.finished_runs.is_empty() || succeeded > 0;

        self.summary.status = if anything_failed {
            ExecutionStatus::Failed
        } else if anything_open || !anything_finished {
            ExecutionStatus::Running
        } else {
            ExecutionStatus::Succeeded
        };

        if self.summary.status == ExecutionStatus::Running {
            self.summary.ended_at = None;
            self.summary.duration_ms = None;
        } else {
            let ended = self
                .nodes
                .values()
                .filter_map(|node| node.ended_at)
                .max()
                .unwrap_or(self.summary.started_at)
                .max(self.summary.ended_at.unwrap_or(self.summary.started_at));
            self.summary.ended_at = Some(ended);
            self.summary.duration_ms =
                Some(((ended - self.summary.started_at).whole_milliseconds()).max(0) as i64);
        }
    }

    fn detail(&self) -> ExecutionDetail {
        ExecutionDetail {
            summary: self.summary.clone(),
            nodes: self
                .node_order
                .iter()
                .filter_map(|id| self.nodes.get(id))
                .cloned()
                .collect(),
            edges: self.edges.clone(),
            messages: self.messages.clone(),
            messages_truncated: self.messages_truncated,
        }
    }
}

/// The projection. Folded from the log like everything else here, and rebuilt
/// by a replay on restart.
#[derive(Debug, Default)]
pub struct WorkflowState {
    definitions: HashMap<String, WorkflowDefinition>,
    /// Workflow ids in first-seen order; the eviction candidate list.
    definition_order: Vec<String>,
    held: HashMap<String, Held>,
    /// Execution ids in first-seen order; the eviction candidate list.
    order: Vec<String>,
    /// Running totals, so the global caps are checked without walking every
    /// execution on every write.
    artifact_count: usize,
    message_count: usize,
}

impl WorkflowState {
    /// Fold one event in.
    ///
    /// Called for **every** event, not only workflow-flavoured ones: a
    /// traversal is made of runs, agents and steps that carry no marker beyond
    /// their `workflow_run_id`. Events without one are not part of any graph
    /// and return immediately.
    pub fn apply(&mut self, event: &RecordedEvent, config: &WorkflowConfig) {
        let (Some(workflow_id), Some(execution_id)) = (
            event.metadata.workflow_id.clone(),
            event.metadata.workflow_run_id.clone(),
        ) else {
            return;
        };

        if event.event_type == EventType::WorkflowDeclared {
            self.declare(event, &workflow_id, config);
        }

        if !self.held.contains_key(&execution_id) {
            self.order.push(execution_id.clone());
            let mut held = Held::new(event, execution_id.clone(), workflow_id.clone());
            // Seed the shape the catalog already knows, so a graph is drawable
            // from the first event of an execution rather than from whenever
            // its declaration happens to arrive.
            if let Some(definition) = self.definitions.get(&workflow_id) {
                held.adopt(definition, config.max_nodes_per_execution);
            }
            self.held.insert(execution_id.clone(), held);
            self.bump_definition(&workflow_id, event.metadata.occurred_at);
        }

        let mut artifacts_added = 0usize;
        let mut messages_added = 0usize;
        {
            let Some(held) = self.held.get_mut(&execution_id) else {
                return;
            };
            held.summary.last_checkpoint = event.metadata.checkpoint.clone();
            held.summary.last_activity_at = held
                .summary
                .last_activity_at
                .max(event.metadata.occurred_at);
            if event.metadata.occurred_at < held.summary.started_at {
                // Producers across four pods do not agree on a clock to the
                // millisecond; keep the earliest observation as the start.
                held.summary.started_at = event.metadata.occurred_at;
            }
            if let Some(agent) = &event.metadata.agent_id {
                held.note_agent(agent);
            }
            held.seen_runs.insert(event.metadata.run_id.clone());

            match event.event_type.subject() {
                Subject::Run => apply_run(held, event),
                Subject::Step => apply_step(held, event, config),
                Subject::Workflow if event.event_type == EventType::ArtifactProduced => {
                    artifacts_added = usize::from(apply_artifact(held, event, config));
                }
                Subject::Agent if event.event_type == EventType::AgentMessage => {
                    messages_added = usize::from(apply_message(held, event, config));
                }
                _ => {}
            }

            held.recount();
        }

        self.artifact_count += artifacts_added;
        self.message_count += messages_added;

        // Evict first, then recount: the counts are a fold over what is still
        // held, so refreshing before eviction would leave a definition claiming
        // executions that had just been dropped — and if the dropped one was
        // its last, claiming them for good.
        self.evict(config);
        self.shed_detail(config);
        self.refresh_definition(&workflow_id, event.metadata.occurred_at);
        // Also here, not only in `declare`: a workflow that nobody ever
        // declares still gets a catalog row from `bump_definition`, and a cap
        // enforced on only one of the two paths is not a cap.
        self.evict_definitions(config.max_definitions);
    }

    /// Upsert the catalog entry from a declaration.
    fn declare(&mut self, event: &RecordedEvent, workflow_id: &str, config: &WorkflowConfig) {
        let nodes = declared_nodes(&event.data);
        let edges = declared_edges(&event.data);
        let version = event
            .data_str("version")
            .or_else(|| event.data_str("workflow_version"))
            .map(ToOwned::to_owned);
        let name = event
            .data_str("name")
            .or_else(|| event.data_str("workflow_name"))
            .unwrap_or(workflow_id)
            .to_owned();

        if let Some(existing) = self.definitions.get_mut(workflow_id) {
            existing.name = name;
            existing.last_activity_at = existing.last_activity_at.max(event.metadata.occurred_at);
            // A declaration that carries no nodes is a producer saying the
            // workflow exists, not one saying it is empty. Keeping the stored
            // shape is what stops a heartbeat from wiping the graph.
            if !nodes.is_empty() {
                existing.version = version;
                existing.nodes = nodes;
                existing.edges = edges;
            }
        } else {
            self.definition_order.push(workflow_id.to_owned());
            self.definitions.insert(
                workflow_id.to_owned(),
                WorkflowDefinition {
                    workflow_id: workflow_id.to_owned(),
                    name,
                    version,
                    nodes,
                    edges,
                    declared_at: event.metadata.occurred_at,
                    last_activity_at: event.metadata.occurred_at,
                    executions: 0,
                    running: 0,
                    succeeded: 0,
                    failed: 0,
                },
            );
        }

        // Every execution of this workflow that is still held picks the shape
        // up, including ones that started before the declaration arrived.
        let Some(definition) = self.definitions.get(workflow_id).cloned() else {
            return;
        };
        for held in self.held.values_mut() {
            if held.summary.workflow_id == workflow_id {
                held.adopt(&definition, config.max_nodes_per_execution);
                held.recount();
            }
        }
        self.evict_definitions(config.max_definitions);
    }

    /// A workflow with no declaration still gets a catalog row, so the picker
    /// lists it. Its graph is whatever ran.
    fn bump_definition(&mut self, workflow_id: &str, at: OffsetDateTime) {
        if !self.definitions.contains_key(workflow_id) {
            self.definition_order.push(workflow_id.to_owned());
            self.definitions.insert(
                workflow_id.to_owned(),
                WorkflowDefinition {
                    workflow_id: workflow_id.to_owned(),
                    name: workflow_id.to_owned(),
                    version: None,
                    nodes: Vec::new(),
                    edges: Vec::new(),
                    declared_at: at,
                    last_activity_at: at,
                    executions: 0,
                    running: 0,
                    succeeded: 0,
                    failed: 0,
                },
            );
        }
    }

    /// Recount a workflow's executions. Cheap enough to do on every event: the
    /// alternative is four counters kept in step across creation, transition
    /// and eviction, and eviction is where that always goes wrong.
    fn refresh_definition(&mut self, workflow_id: &str, at: OffsetDateTime) {
        let mut executions = 0;
        let mut running = 0;
        let mut succeeded = 0;
        let mut failed = 0;
        for held in self.held.values() {
            if held.summary.workflow_id != workflow_id {
                continue;
            }
            executions += 1;
            match held.summary.status {
                ExecutionStatus::Running => running += 1,
                ExecutionStatus::Succeeded => succeeded += 1,
                ExecutionStatus::Failed => failed += 1,
            }
        }
        if let Some(definition) = self.definitions.get_mut(workflow_id) {
            definition.executions = executions;
            definition.running = running;
            definition.succeeded = succeeded;
            definition.failed = failed;
            definition.last_activity_at = definition.last_activity_at.max(at);
        }
    }

    // ── Reads ────────────────────────────────────────────────────────────────

    /// The catalog, most recently active first.
    #[must_use]
    pub fn workflows(&self, filter: &WorkflowFilter, now: OffsetDateTime) -> WorkflowPage {
        let limit = filter.limit.unwrap_or(50).clamp(1, 500);
        let needle = filter.search.as_ref().map(|text| text.to_lowercase());
        let since = crate::window::cutoff(filter.window_seconds, now);

        let mut matching: Vec<&WorkflowDefinition> = self
            .definitions
            .values()
            .filter(|row| since.is_none_or(|start| row.last_activity_at >= start))
            .filter(|row| {
                needle.as_ref().is_none_or(|needle| {
                    row.workflow_id.to_lowercase().contains(needle)
                        || row.name.to_lowercase().contains(needle)
                })
            })
            .collect();
        // Most recently active first, with the id breaking ties so the order is
        // total — two workflows touched in the same millisecond must not swap
        // places between two pages of one listing.
        matching.sort_by(|left, right| {
            right
                .last_activity_at
                .cmp(&left.last_activity_at)
                .then_with(|| left.workflow_id.cmp(&right.workflow_id))
        });

        if let Some(cursor) = &filter.after
            && let Some(index) = matching.iter().position(|row| &row.workflow_id == cursor)
        {
            matching.drain(0..=index);
        }

        let total_known = matching.len();
        let workflows: Vec<WorkflowDefinition> =
            matching.into_iter().take(limit).cloned().collect();
        let next_cursor = (total_known > workflows.len())
            .then(|| workflows.last().map(|row| row.workflow_id.clone()))
            .flatten();

        WorkflowPage {
            workflows,
            next_cursor,
            total_known,
        }
    }

    #[must_use]
    pub fn workflow(&self, workflow_id: &str) -> Option<WorkflowDefinition> {
        self.definitions.get(workflow_id).cloned()
    }

    /// Newest first, filtered, one page.
    #[must_use]
    pub fn executions(&self, filter: &ExecutionFilter, now: OffsetDateTime) -> ExecutionPage {
        let limit = filter.limit.unwrap_or(50).clamp(1, 500);
        let needle = filter.search.as_ref().map(|text| text.to_lowercase());
        let since = crate::window::cutoff(filter.window_seconds, now);

        let mut matching: Vec<&ExecutionSummary> = self
            .order
            .iter()
            .rev()
            .filter_map(|id| self.held.get(id))
            .map(|held| &held.summary)
            .filter(|row| since.is_none_or(|start| row.last_activity_at >= start))
            .filter(|row| {
                filter
                    .workflow_id
                    .as_ref()
                    .is_none_or(|want| &row.workflow_id == want)
            })
            .filter(|row| filter.status.is_none_or(|want| row.status == want))
            .filter(|row| {
                needle.as_ref().is_none_or(|needle| {
                    row.workflow_run_id.to_lowercase().contains(needle)
                        || row.workflow_id.to_lowercase().contains(needle)
                        || row
                            .agents
                            .iter()
                            .any(|agent| agent.to_lowercase().contains(needle))
                })
            })
            .collect();

        if let Some(cursor) = &filter.after
            && let Some(index) = matching
                .iter()
                .position(|row| &row.workflow_run_id == cursor)
        {
            matching.drain(0..=index);
        }

        let total_known = matching.len();
        let executions: Vec<ExecutionSummary> = matching.into_iter().take(limit).cloned().collect();
        let next_cursor = (total_known > executions.len())
            .then(|| executions.last().map(|row| row.workflow_run_id.clone()))
            .flatten();

        ExecutionPage {
            executions,
            next_cursor,
            total_known,
        }
    }

    #[must_use]
    pub fn execution(&self, workflow_run_id: &str) -> Option<ExecutionDetail> {
        self.held.get(workflow_run_id).map(Held::detail)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.held.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    // ── Caps ─────────────────────────────────────────────────────────────────

    /// Give up detail before giving up executions.
    ///
    /// An execution that has lost its messages still has its graph, its node
    /// statuses and its artifact counts — everything the canvas draws. What is
    /// given up is the per-message list of an old traversal, the same trade
    /// `EvaluationState::shed_detail` makes for an old report's cases. A
    /// running execution is skipped: it is the one someone is watching.
    fn shed_detail(&mut self, config: &WorkflowConfig) {
        if self.message_count <= config.max_messages_total
            && self.artifact_count <= config.max_artifacts_total
        {
            return;
        }
        for id in self.order.clone() {
            if self.message_count <= config.max_messages_total
                && self.artifact_count <= config.max_artifacts_total
            {
                break;
            }
            let Some(held) = self.held.get_mut(&id) else {
                continue;
            };
            if held.summary.status == ExecutionStatus::Running {
                continue;
            }
            if self.message_count > config.max_messages_total && !held.messages.is_empty() {
                self.message_count = self.message_count.saturating_sub(held.messages.len());
                held.messages.clear();
                held.messages_truncated = true;
            }
            if self.artifact_count > config.max_artifacts_total {
                let mut dropped = 0;
                for node in held.nodes.values_mut() {
                    dropped += node.artifacts.len();
                    node.artifacts.clear();
                }
                self.artifact_count = self.artifact_count.saturating_sub(dropped);
            }
        }
    }

    /// Drop the oldest finished executions once over the cap.
    fn evict(&mut self, config: &WorkflowConfig) {
        if self.held.len() <= config.max_executions {
            return;
        }
        let mut excess = self.held.len() - config.max_executions;
        let mut keep = Vec::with_capacity(self.order.len());
        for id in std::mem::take(&mut self.order) {
            let finished = self
                .held
                .get(&id)
                .is_some_and(|held| held.summary.status != ExecutionStatus::Running);
            if excess > 0 && finished {
                if let Some(dropped) = self.held.remove(&id) {
                    self.message_count = self.message_count.saturating_sub(dropped.messages.len());
                    let artifacts: usize = dropped
                        .nodes
                        .values()
                        .map(|node| node.artifacts.len())
                        .sum();
                    self.artifact_count = self.artifact_count.saturating_sub(artifacts);
                }
                excess -= 1;
            } else {
                keep.push(id);
            }
        }
        self.order = keep;
    }

    /// Drop the least recently active workflows once over the cap.
    ///
    /// Unlike an execution, a definition is never "running" — but one with a
    /// live execution under it must not go, or that execution's canvas loses
    /// its edges.
    fn evict_definitions(&mut self, max_definitions: usize) {
        if self.definitions.len() <= max_definitions {
            return;
        }
        let mut excess = self.definitions.len() - max_definitions;
        let mut keep = Vec::with_capacity(self.definition_order.len());
        for id in std::mem::take(&mut self.definition_order) {
            let held_by_an_execution = self
                .held
                .values()
                .any(|held| held.summary.workflow_id == id);
            if excess > 0 && !held_by_an_execution {
                self.definitions.remove(&id);
                excess -= 1;
            } else {
                keep.push(id);
            }
        }
        self.definition_order = keep;
    }
}

// ── Per-subject folds ────────────────────────────────────────────────────────

fn apply_run(held: &mut Held, event: &RecordedEvent) {
    let run_id = event.metadata.run_id.clone();
    match event.event_type.phase() {
        Some(Phase::Start) => {
            if !held.finished_runs.contains(&run_id) {
                held.open_runs.insert(run_id);
            }
        }
        Some(Phase::End { ok }) => {
            held.open_runs.remove(&run_id);
            held.finished_runs.insert(run_id);
            held.summary.ended_at = Some(
                held.summary
                    .ended_at
                    .map_or(event.metadata.occurred_at, |seen| {
                        seen.max(event.metadata.occurred_at)
                    }),
            );
            if !ok && held.summary.error.is_none() {
                held.summary.error = event
                    .data_str("error")
                    .or_else(|| event.data_str("message"))
                    .map_or_else(|| "the run failed".to_owned(), ToOwned::to_owned)
                    .into();
            }
        }
        _ => {}
    }
}

fn apply_step(held: &mut Held, event: &RecordedEvent, config: &WorkflowConfig) {
    let Some(node_id) = node_key(event) else {
        return;
    };
    let phase = event.event_type.phase();
    let at = event.metadata.occurred_at;
    let agent = event.metadata.agent_id.clone();
    let run_id = event.metadata.run_id.clone();
    let span_id = event.metadata.span_id;
    let span_key = event.metadata.span_key.clone();
    let kind = event
        .event_type
        .step_type(&event.data)
        .map(ToOwned::to_owned);
    let error = event
        .data_str("error")
        .or_else(|| event.data_str("message"))
        .map(ToOwned::to_owned);

    let counted = {
        let seen = held.attempts.entry(node_id.clone()).or_default();
        matches!(phase, Some(Phase::Start)) && seen.insert(span_key)
    };
    let attempts = held
        .attempts
        .get(&node_id)
        .map_or(0, |seen| seen.len().min(u32::MAX as usize) as u32);

    let Some(node) = held.node_mut(&node_id, config.max_nodes_per_execution) else {
        return;
    };
    if node.kind.is_none() {
        node.kind = kind;
    }
    node.run_id = Some(run_id);
    node.span_id = Some(span_id);
    if let Some(agent) = agent
        && !node.agents.iter().any(|known| known == &agent)
    {
        node.agents.push(agent);
    }

    match phase {
        Some(Phase::Start) => {
            if counted || node.started_at.is_none() {
                node.started_at = Some(node.started_at.map_or(at, |seen| seen.min(at)));
            }
            // A retry reopens a node that had already finished. Clearing the
            // end is what stops the second attempt of a flaky stage from
            // rendering as instantaneous.
            if counted && attempts > 1 {
                node.ended_at = None;
                node.duration_ms = None;
                node.error = None;
            }
            node.status = NodeStatus::Running;
        }
        Some(Phase::End { ok }) => {
            node.status = if ok {
                NodeStatus::Succeeded
            } else {
                NodeStatus::Failed
            };
            node.ended_at = Some(at);
            node.duration_ms = node
                .started_at
                .map(|started| ((at - started).whole_milliseconds()).max(0) as i64);
            if !ok {
                node.error = error;
            }
        }
        _ => {}
    }
    node.attempts = attempts.max(node.attempts);
}

/// Returns whether a new artifact was recorded.
fn apply_artifact(held: &mut Held, event: &RecordedEvent, config: &WorkflowConfig) -> bool {
    let Some(uri) = event
        .data_str("uri")
        .or_else(|| event.data_str("url"))
        .or_else(|| event.data_str("path"))
        .filter(|uri| !uri.is_empty())
    else {
        // An artifact with no reference is not an artifact. Recording the name
        // alone would list a row nobody can open.
        return false;
    };
    let uri = uri.to_owned();
    let node_id = node_key(event).unwrap_or_else(|| "unassigned".to_owned());
    let artifact = Artifact {
        name: event
            .data_str("name")
            .or_else(|| event.data_str("artifact"))
            .unwrap_or_else(|| file_name_of(&uri))
            .to_owned(),
        uri: uri.clone(),
        media_type: event
            .data_str("media_type")
            .or_else(|| event.data_str("content_type"))
            .map(ToOwned::to_owned),
        size_bytes: event
            .data_i64("size_bytes")
            .or_else(|| event.data_i64("size")),
        digest: event
            .data_str("digest")
            .or_else(|| event.data_str("sha256"))
            .map(ToOwned::to_owned),
        produced_at: event.metadata.occurred_at,
    };

    if !held.artifact_uris.insert(format!("{node_id}\u{1f}{uri}")) {
        return false;
    }
    let Some(node) = held.node_mut(&node_id, config.max_nodes_per_execution) else {
        return false;
    };
    node.artifacts.push(artifact);
    true
}

/// Returns whether a new message was recorded.
fn apply_message(held: &mut Held, event: &RecordedEvent, config: &WorkflowConfig) -> bool {
    let from = event
        .data_str("from")
        .or_else(|| event.data_str("sender"))
        .map(ToOwned::to_owned)
        .or_else(|| event.metadata.agent_id.clone());
    let to = event
        .data_str("to")
        .or_else(|| event.data_str("recipient"))
        .map(ToOwned::to_owned);
    let (Some(from), Some(to)) = (from, to) else {
        // An edge needs both ends. One with a missing side would be drawn
        // pointing at nothing, which is worse than not drawing it.
        return false;
    };

    if !held
        .message_ids
        .insert(event.metadata.message_id.to_string())
    {
        return false;
    }
    held.note_agent(&from);
    held.note_agent(&to);

    if held.messages.len() < config.max_messages_per_execution {
        held.messages.push(AgentMessage {
            from,
            to,
            kind: event
                .data_str("kind")
                .or_else(|| event.data_str("message_type"))
                .map(ToOwned::to_owned),
            channel: event.data_str("channel").map(ToOwned::to_owned),
            node: node_key(event),
            at: event.metadata.occurred_at,
        });
    } else {
        // The list is capped and the count is not: a chatty execution still
        // reports how much it said.
        held.messages_truncated = true;
    }
    true
}

// ── Payload readers ──────────────────────────────────────────────────────────

/// Which node of the graph an event belongs to.
///
/// `node` is the field to set; the rest are spellings a producer already uses.
/// `call_id` last, because it is what the span key is built from and is
/// therefore always present on a step — a useful fallback and a poor first
/// choice, since two attempts of one node carry different call ids.
fn node_key(event: &RecordedEvent) -> Option<String> {
    ["node", "node_id", "stage", "task", "name", "call_id"]
        .iter()
        .find_map(|key| event.data_str(key))
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn declared_nodes(data: &serde_json::Value) -> Vec<WorkflowNode> {
    let Some(items) = data.get("nodes").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    items
        .iter()
        .filter_map(|item| {
            // A bare string is a node with no metadata: `nodes: ["a", "b"]` is
            // what somebody writes first, and refusing it would make the
            // simplest declaration the one that silently does nothing.
            let (id, object) = match item {
                serde_json::Value::String(id) => (id.clone(), None),
                serde_json::Value::Object(fields) => {
                    let id = fields
                        .get("id")
                        .or_else(|| fields.get("node"))
                        .or_else(|| fields.get("name"))
                        .and_then(serde_json::Value::as_str)?
                        .to_owned();
                    (id, Some(fields))
                }
                _ => return None,
            };
            if id.is_empty() || !seen.insert(id.clone()) {
                return None;
            }
            let text = |key: &str| {
                object
                    .and_then(|fields| fields.get(key))
                    .and_then(serde_json::Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
            };
            Some(WorkflowNode {
                name: text("name").unwrap_or_else(|| id.clone()),
                id,
                kind: text("kind")
                    .or_else(|| text("type"))
                    .or_else(|| text("step_type")),
                agent: text("agent").or_else(|| text("agent_id")),
            })
        })
        .collect()
}

fn declared_edges(data: &serde_json::Value) -> Vec<WorkflowEdge> {
    let Some(items) = data.get("edges").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    items
        .iter()
        .filter_map(|item| {
            let edge = match item {
                // `[["a", "b"], ["b", "c"]]` — the shortest thing that can
                // express a chain.
                serde_json::Value::Array(pair) if pair.len() >= 2 => WorkflowEdge {
                    from: pair.first()?.as_str()?.to_owned(),
                    to: pair.get(1)?.as_str()?.to_owned(),
                    label: None,
                },
                serde_json::Value::Object(fields) => {
                    let text = |key: &str| {
                        fields
                            .get(key)
                            .and_then(serde_json::Value::as_str)
                            .filter(|value| !value.is_empty())
                            .map(ToOwned::to_owned)
                    };
                    WorkflowEdge {
                        from: text("from").or_else(|| text("source"))?,
                        to: text("to").or_else(|| text("target"))?,
                        label: text("label"),
                    }
                }
                _ => return None,
            };
            if edge.from.is_empty() || edge.to.is_empty() {
                return None;
            }
            seen.insert(edge.clone()).then_some(edge)
        })
        .collect()
}

/// The last path segment of a uri, for an artifact whose producer named no
/// name. `s3://bucket/house/acquire.json` → `acquire.json`.
fn file_name_of(uri: &str) -> &str {
    uri.rsplit('/')
        .next()
        .filter(|last| !last.is_empty())
        .unwrap_or(uri)
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use time::macros::datetime;

    use aiwatcher_core::{EventEnvelope, Sdk, Source};

    use super::*;

    fn now() -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }

    /// Builds the recorded stream for one execution, the way a log would.
    struct Traversal {
        workflow_id: String,
        execution_id: String,
        position: u64,
        at: OffsetDateTime,
    }

    impl Traversal {
        fn new(workflow_id: &str, execution_id: &str) -> Self {
            Self {
                workflow_id: workflow_id.to_owned(),
                execution_id: execution_id.to_owned(),
                position: 0,
                at: datetime!(2026-08-28 09:00:00 UTC),
            }
        }

        fn after(&mut self, millis: i64) -> &mut Self {
            self.at += time::Duration::milliseconds(millis);
            self
        }

        fn emit(
            &mut self,
            event_type: EventType,
            run_id: &str,
            agent_id: Option<&str>,
            data: serde_json::Value,
        ) -> RecordedEvent {
            self.position += 1;
            let mut envelope = EventEnvelope::new(
                event_type,
                run_id,
                self.at,
                Source::new("planner-import-service", Sdk::Python),
            )
            .with_data(data);
            envelope.event_id = Some(aiwatcher_core::MessageId::new(format!(
                "{}-{}",
                self.execution_id, self.position
            )));
            envelope.workflow_id = Some(self.workflow_id.clone());
            envelope.workflow_run_id = Some(self.execution_id.clone());
            envelope.agent_id = agent_id.map(ToOwned::to_owned);
            envelope.record(self.position, self.position, self.at, None)
        }
    }

    fn fold(events: &[RecordedEvent]) -> WorkflowState {
        let mut state = WorkflowState::default();
        let config = WorkflowConfig::default();
        for event in events {
            state.apply(event, &config);
        }
        state
    }

    fn declaration() -> serde_json::Value {
        json!({
            "workflow_id": "house-import",
            "name": "House import",
            "version": "sha256:f00d",
            "nodes": [
                { "id": "acquire", "name": "Acquire assets", "kind": "chain" },
                { "id": "normalize", "name": "Normalize", "kind": "chain" },
                { "id": "analyze", "name": "Analyze", "kind": "agent", "agent": "floor-plan" },
                { "id": "persist", "name": "Persist review", "kind": "chain" },
            ],
            "edges": [
                { "from": "acquire", "to": "normalize" },
                { "from": "normalize", "to": "analyze" },
                { "from": "analyze", "to": "persist" },
            ],
        })
    }

    #[test]
    fn a_declared_node_that_never_ran_is_pending_rather_than_absent() {
        // The whole reason a declaration exists: "what has this not done yet"
        // is unanswerable from observed events alone.
        let mut run = Traversal::new("house-import", "exec-1");
        let events = vec![
            run.emit(EventType::WorkflowDeclared, "run-a", None, declaration()),
            run.emit(EventType::RunStarted, "run-a", None, json!({})),
            run.after(10).emit(
                EventType::StepStarted,
                "run-a",
                None,
                json!({ "node": "acquire" }),
            ),
            run.after(200).emit(
                EventType::StepCompleted,
                "run-a",
                None,
                json!({ "node": "acquire" }),
            ),
        ];

        let state = fold(&events);
        let detail = state.execution("exec-1").expect("the execution is held");

        assert_eq!(
            detail
                .nodes
                .iter()
                .map(|node| node.node_id.as_str())
                .collect::<Vec<_>>(),
            vec!["acquire", "normalize", "analyze", "persist"],
            "the graph is the declared one, in declaration order"
        );
        assert_eq!(detail.nodes[0].status, NodeStatus::Succeeded);
        for pending in &detail.nodes[1..] {
            assert_eq!(pending.status, NodeStatus::Pending, "{}", pending.node_id);
        }
        assert_eq!(detail.summary.nodes_pending, 3);
        assert_eq!(detail.edges.len(), 3, "and it carries its declared edges");
    }

    #[test]
    fn four_stages_in_four_runs_are_one_execution() {
        // planner's shape: Flyte gives every stage its own pod and therefore
        // its own run. Without the join, this is four unrelated rows.
        let mut run = Traversal::new("house-import", "exec-2");
        // Declared by the first stage's pod, which is what makes the run count
        // below a statement about stages rather than about who declared.
        let mut events = vec![run.emit(
            EventType::WorkflowDeclared,
            "run-acquire",
            None,
            declaration(),
        )];
        for stage in ["acquire", "normalize", "analyze", "persist"] {
            let run_id = format!("run-{stage}");
            events.push(run.emit(EventType::RunStarted, &run_id, None, json!({})));
            events.push(run.after(5).emit(
                EventType::StepStarted,
                &run_id,
                Some("importer"),
                json!({ "node": stage }),
            ));
            events.push(run.after(100).emit(
                EventType::StepCompleted,
                &run_id,
                Some("importer"),
                json!({ "node": stage }),
            ));
            events.push(run.emit(
                EventType::RunCompleted,
                &run_id,
                None,
                json!({ "status": "succeeded" }),
            ));
        }

        let state = fold(&events);
        let detail = state.execution("exec-2").expect("the execution is held");

        assert_eq!(detail.summary.status, ExecutionStatus::Succeeded);
        assert_eq!(detail.summary.nodes_succeeded, 4);
        assert_eq!(detail.summary.nodes_pending, 0);
        assert_eq!(detail.summary.runs.len(), 4, "made of four runs");
        assert!(detail.summary.duration_ms.is_some_and(|ms| ms > 0));
    }

    #[test]
    fn a_failed_node_fails_the_execution_and_names_why() {
        let mut run = Traversal::new("house-import", "exec-3");
        let events = vec![
            run.emit(EventType::WorkflowDeclared, "run-a", None, declaration()),
            run.emit(EventType::RunStarted, "run-a", None, json!({})),
            run.after(10).emit(
                EventType::StepStarted,
                "run-a",
                None,
                json!({ "node": "analyze" }),
            ),
            run.after(50).emit(
                EventType::StepFailed,
                "run-a",
                None,
                json!({ "node": "analyze", "error": "OpenCV found no walls" }),
            ),
        ];

        let state = fold(&events);
        let detail = state.execution("exec-3").expect("the execution is held");

        assert_eq!(detail.summary.status, ExecutionStatus::Failed);
        let analyze = detail
            .nodes
            .iter()
            .find(|node| node.node_id == "analyze")
            .expect("the node is in the graph");
        assert_eq!(analyze.status, NodeStatus::Failed);
        assert_eq!(analyze.error.as_deref(), Some("OpenCV found no walls"));
    }

    #[test]
    fn a_skipped_branch_does_not_hold_the_execution_open_forever() {
        // A conditional graph reaches its end with nodes never started. Waiting
        // on them would leave every such workflow running for good; the pending
        // count says what happened instead.
        let mut run = Traversal::new("house-import", "exec-4");
        let events = vec![
            run.emit(EventType::WorkflowDeclared, "run-a", None, declaration()),
            run.emit(EventType::RunStarted, "run-a", None, json!({})),
            run.after(10).emit(
                EventType::StepStarted,
                "run-a",
                None,
                json!({ "node": "acquire" }),
            ),
            run.after(50).emit(
                EventType::StepCompleted,
                "run-a",
                None,
                json!({ "node": "acquire" }),
            ),
            run.after(10).emit(
                EventType::RunCompleted,
                "run-a",
                None,
                json!({ "status": "succeeded" }),
            ),
        ];

        let state = fold(&events);
        let summary = state.execution("exec-4").expect("held").summary;

        assert_eq!(summary.status, ExecutionStatus::Succeeded);
        assert_eq!(summary.nodes_pending, 3, "and says three never ran");
    }

    #[test]
    fn a_redelivered_step_does_not_invent_a_retry() {
        // Attempts are counted by span key, which is stable across a
        // redelivery. Counting events instead would report a flaky stage that
        // ran exactly once.
        let mut run = Traversal::new("house-import", "exec-5");
        let started = run.emit(
            EventType::StepStarted,
            "run-a",
            Some("importer"),
            json!({ "node": "acquire", "call_id": "attempt-1" }),
        );
        let events = vec![started.clone(), started];

        let state = fold(&events);
        let detail = state.execution("exec-5").expect("held");

        assert_eq!(detail.nodes[0].attempts, 1);
    }

    #[test]
    fn a_second_attempt_of_one_node_is_counted_and_reopens_it() {
        let mut run = Traversal::new("house-import", "exec-6");
        let events = vec![
            run.emit(
                EventType::StepStarted,
                "run-a",
                Some("importer"),
                json!({ "node": "acquire", "call_id": "attempt-1" }),
            ),
            run.after(50).emit(
                EventType::StepFailed,
                "run-a",
                Some("importer"),
                json!({ "node": "acquire", "call_id": "attempt-1", "error": "timeout" }),
            ),
            run.after(10).emit(
                EventType::StepStarted,
                "run-a",
                Some("importer"),
                json!({ "node": "acquire", "call_id": "attempt-2" }),
            ),
        ];

        let state = fold(&events);
        let node = &state.execution("exec-6").expect("held").nodes[0];

        assert_eq!(node.attempts, 2);
        assert_eq!(node.status, NodeStatus::Running, "the retry reopened it");
        assert!(
            node.ended_at.is_none(),
            "and cleared the first attempt's end"
        );
        assert!(node.error.is_none());
    }

    #[test]
    fn an_artifact_is_a_reference_attached_to_its_node() {
        let mut run = Traversal::new("house-import", "exec-7");
        let events = vec![
            run.emit(EventType::WorkflowDeclared, "run-a", None, declaration()),
            run.after(10).emit(
                EventType::StepStarted,
                "run-a",
                None,
                json!({ "node": "acquire" }),
            ),
            run.after(50).emit(
                EventType::ArtifactProduced,
                "run-a",
                None,
                json!({
                    "node": "acquire",
                    "uri": "s3://planner-flyte/house/acquisition.json",
                    "media_type": "application/json",
                    "size_bytes": 41_233,
                }),
            ),
        ];

        let state = fold(&events);
        let detail = state.execution("exec-7").expect("held");
        let acquire = &detail.nodes[0];

        assert_eq!(acquire.artifacts.len(), 1);
        assert_eq!(
            acquire.artifacts[0].name, "acquisition.json",
            "named from the uri"
        );
        assert_eq!(acquire.artifacts[0].size_bytes, Some(41_233));
        assert_eq!(detail.summary.artifacts, 1);
    }

    #[test]
    fn a_redelivered_artifact_is_listed_once() {
        let mut run = Traversal::new("house-import", "exec-8");
        let artifact = run.emit(
            EventType::ArtifactProduced,
            "run-a",
            None,
            json!({ "node": "acquire", "uri": "s3://b/one.json" }),
        );
        let state = fold(&[artifact.clone(), artifact]);

        assert_eq!(
            state.execution("exec-8").expect("held").summary.artifacts,
            1
        );
    }

    #[test]
    fn an_artifact_with_no_reference_is_not_recorded() {
        // A row nobody can open is worse than no row: it claims something was
        // produced and gives no way to check.
        let mut run = Traversal::new("house-import", "exec-9");
        let events = vec![run.emit(
            EventType::ArtifactProduced,
            "run-a",
            None,
            json!({ "node": "acquire", "name": "acquisition.json" }),
        )];

        let state = fold(&events);
        assert_eq!(
            state.execution("exec-9").expect("held").summary.artifacts,
            0
        );
    }

    #[test]
    fn two_agents_talking_become_messages_and_not_edges() {
        // Declared edges are the shape; messages are what was said. Merging
        // them would claim a handoff that nothing recorded.
        let mut run = Traversal::new("house-import", "exec-10");
        let events = vec![
            run.emit(EventType::WorkflowDeclared, "run-a", None, declaration()),
            run.emit(
                EventType::AgentMessage,
                "run-a",
                Some("planner"),
                json!({ "to": "floor-plan", "kind": "handoff", "channel": "queue" }),
            ),
            run.after(30).emit(
                EventType::AgentMessage,
                "run-a",
                Some("floor-plan"),
                json!({ "to": "planner", "kind": "response" }),
            ),
        ];

        let state = fold(&events);
        let detail = state.execution("exec-10").expect("held");

        assert_eq!(detail.messages.len(), 2);
        assert_eq!(detail.messages[0].from, "planner");
        assert_eq!(detail.messages[0].to, "floor-plan");
        assert_eq!(detail.messages[0].kind.as_deref(), Some("handoff"));
        assert_eq!(detail.edges.len(), 3, "the declared edges are untouched");
        assert_eq!(
            detail.summary.agents,
            vec!["planner", "floor-plan"],
            "both ends of a message count as agents of the execution"
        );
    }

    #[test]
    fn a_message_missing_one_end_is_dropped() {
        let mut run = Traversal::new("house-import", "exec-11");
        let events = vec![run.emit(
            EventType::AgentMessage,
            "run-a",
            Some("planner"),
            json!({ "kind": "broadcast" }),
        )];

        assert_eq!(
            fold(&events)
                .execution("exec-11")
                .expect("held")
                .messages
                .len(),
            0
        );
    }

    #[test]
    fn a_node_nobody_declared_is_kept_and_flagged() {
        // A graph that has drifted from the code running it is the case worth
        // seeing, so the undeclared node stays and says so.
        let mut run = Traversal::new("house-import", "exec-12");
        let events = vec![
            run.emit(EventType::WorkflowDeclared, "run-a", None, declaration()),
            run.after(10).emit(
                EventType::StepStarted,
                "run-a",
                None,
                json!({ "node": "watermark" }),
            ),
        ];

        let state = fold(&events);
        let detail = state.execution("exec-12").expect("held");
        let extra = detail
            .nodes
            .iter()
            .find(|node| node.node_id == "watermark")
            .expect("kept");

        assert!(!extra.declared);
        assert!(detail.nodes[..4].iter().all(|node| node.declared));
        assert_eq!(
            detail.nodes.last().map(|node| node.node_id.as_str()),
            Some("watermark"),
            "and it sorts after the declared ones so the canvas does not move"
        );
    }

    #[test]
    fn a_declaration_arriving_late_still_shapes_the_execution() {
        // Out-of-order delivery, or a stage-per-pod orchestrator whose second
        // pod is the one that declares.
        let mut run = Traversal::new("house-import", "exec-13");
        let events = vec![
            run.emit(
                EventType::StepStarted,
                "run-a",
                None,
                json!({ "node": "acquire" }),
            ),
            run.after(50).emit(
                EventType::StepCompleted,
                "run-a",
                None,
                json!({ "node": "acquire" }),
            ),
            run.emit(EventType::WorkflowDeclared, "run-a", None, declaration()),
        ];

        let state = fold(&events);
        let detail = state.execution("exec-13").expect("held");

        assert_eq!(detail.nodes.len(), 4);
        assert!(detail.nodes.iter().all(|node| node.declared));
        assert_eq!(detail.nodes[0].status, NodeStatus::Succeeded);
        assert_eq!(detail.edges.len(), 3);
    }

    #[test]
    fn a_workflow_with_no_declaration_still_appears_in_the_catalog() {
        // planner emits `workflow_id` today and declares nothing. The picker
        // has to list it, or the feature is invisible until every producer
        // ships an SDK update.
        let mut run = Traversal::new("nightly-summary", "exec-14");
        let events = vec![run.emit(EventType::RunStarted, "run-a", None, json!({}))];

        let state = fold(&events);
        let page = state.workflows(&WorkflowFilter::default(), now());

        assert_eq!(page.workflows.len(), 1);
        assert_eq!(page.workflows[0].workflow_id, "nightly-summary");
        assert!(page.workflows[0].nodes.is_empty());
        assert_eq!(page.workflows[0].executions, 1);
        assert_eq!(page.workflows[0].running, 1);
    }

    #[test]
    fn a_run_belonging_to_no_workflow_never_reaches_the_projection() {
        let envelope = EventEnvelope::new(
            EventType::RunStarted,
            "run-plain",
            datetime!(2026-08-28 09:00:00 UTC),
            Source::new("svc", Sdk::Python),
        );
        let event = envelope.record(1, 1, datetime!(2026-08-28 09:00:00 UTC), None);

        let state = fold(&[event]);

        assert!(state.is_empty());
        assert_eq!(
            state
                .workflows(&WorkflowFilter::default(), now())
                .total_known,
            0
        );
    }

    /// An execution is in the window when it last *moved* in it.
    ///
    /// A stage-per-pod traversal is idle between its stages by design, so how
    /// long ago it started says nothing about whether it is still going. Its
    /// last event does.
    #[test]
    fn the_window_keeps_an_execution_that_is_still_moving_and_drops_a_finished_one() {
        let mut state = WorkflowState::default();
        let config = WorkflowConfig::default();

        let mut old = Traversal::new("house-import", "exec-old");
        state.apply(
            &old.emit(EventType::RunStarted, "run-a", None, json!({})),
            &config,
        );

        let mut long = Traversal::new("house-import", "exec-long");
        state.apply(
            &long.emit(EventType::RunStarted, "run-b", None, json!({})),
            &config,
        );
        // Same start, an event four hours later: still working.
        state.apply(
            &long.after(4 * 60 * 60 * 1000).emit(
                EventType::StepStarted,
                "run-b",
                None,
                json!({"node": "load"}),
            ),
            &config,
        );

        let page = state.executions(
            &ExecutionFilter {
                window_seconds: Some(900),
                ..ExecutionFilter::default()
            },
            datetime!(2026-08-28 13:05:00 UTC),
        );

        assert_eq!(
            page.executions
                .iter()
                .map(|row| row.workflow_run_id.as_str())
                .collect::<Vec<_>>(),
            vec!["exec-long"],
        );
    }

    #[test]
    fn executions_page_newest_first_and_the_cursor_is_exclusive() {
        let mut state = WorkflowState::default();
        let config = WorkflowConfig::default();
        for index in 0..5 {
            let mut run = Traversal::new("house-import", &format!("exec-{index}"));
            let event = run.emit(EventType::RunStarted, "run-a", None, json!({}));
            state.apply(&event, &config);
        }

        let first = state.executions(
            &ExecutionFilter {
                limit: Some(2),
                ..ExecutionFilter::default()
            },
            now(),
        );
        assert_eq!(
            first
                .executions
                .iter()
                .map(|row| row.workflow_run_id.as_str())
                .collect::<Vec<_>>(),
            vec!["exec-4", "exec-3"]
        );
        assert_eq!(first.next_cursor.as_deref(), Some("exec-3"));

        let second = state.executions(
            &ExecutionFilter {
                limit: Some(2),
                after: first.next_cursor,
                ..ExecutionFilter::default()
            },
            now(),
        );
        assert_eq!(
            second
                .executions
                .iter()
                .map(|row| row.workflow_run_id.as_str())
                .collect::<Vec<_>>(),
            vec!["exec-2", "exec-1"]
        );
    }

    #[test]
    fn a_running_execution_is_never_the_one_evicted() {
        let config = WorkflowConfig {
            max_executions: 2,
            ..WorkflowConfig::default()
        };
        let mut state = WorkflowState::default();

        // One that stays open, then three that finish.
        let mut open = Traversal::new("house-import", "exec-open");
        let event = open.emit(EventType::RunStarted, "run-open", None, json!({}));
        state.apply(&event, &config);

        for index in 0..3 {
            let mut run = Traversal::new("house-import", &format!("exec-{index}"));
            for event in [
                run.emit(EventType::RunStarted, "run-a", None, json!({})),
                run.after(10).emit(
                    EventType::RunCompleted,
                    "run-a",
                    None,
                    json!({ "status": "succeeded" }),
                ),
            ] {
                state.apply(&event, &config);
            }
        }

        assert!(
            state.execution("exec-open").is_some(),
            "the running execution survives; it is the one being watched"
        );
        assert!(state.len() <= 3);
    }

    #[test]
    fn the_catalog_is_capped_even_for_workflows_nobody_declares() {
        // `bump_definition` is the path a producer that only sets `workflow_id`
        // takes, and a cap enforced on the declaration path alone is not a cap.
        let config = WorkflowConfig {
            max_definitions: 3,
            max_executions: 2,
            ..WorkflowConfig::default()
        };
        let mut state = WorkflowState::default();

        for index in 0..10 {
            let workflow = format!("workflow-{index}");
            let mut run = Traversal::new(&workflow, &format!("exec-{index}"));
            for event in [
                run.emit(EventType::RunStarted, "run-a", None, json!({})),
                run.after(10).emit(
                    EventType::RunCompleted,
                    "run-a",
                    None,
                    json!({ "status": "succeeded" }),
                ),
            ] {
                state.apply(&event, &config);
            }
        }

        let catalog = state.workflows(&WorkflowFilter::default(), now());
        assert!(
            catalog.total_known <= 3 + config.max_executions,
            "the catalog grew to {} rows",
            catalog.total_known
        );
        // And nothing with a live execution under it was dropped along the way.
        for row in &catalog.workflows {
            assert!(row.executions > 0 || row.nodes.is_empty());
        }
    }

    #[test]
    fn a_definition_stops_counting_an_execution_that_was_evicted() {
        // The counts are a fold over what is still held. Refreshing them before
        // eviction would leave a row claiming runs that had just been dropped.
        let config = WorkflowConfig {
            max_executions: 1,
            ..WorkflowConfig::default()
        };
        let mut state = WorkflowState::default();

        for index in 0..4 {
            let mut run = Traversal::new("house-import", &format!("exec-{index}"));
            for event in [
                run.emit(EventType::WorkflowDeclared, "run-a", None, declaration()),
                run.emit(EventType::RunStarted, "run-a", None, json!({})),
                run.after(10).emit(
                    EventType::RunCompleted,
                    "run-a",
                    None,
                    json!({ "status": "succeeded" }),
                ),
            ] {
                state.apply(&event, &config);
            }
        }

        let definition = state.workflow("house-import").expect("declared");
        assert_eq!(
            definition.executions as usize,
            state.len(),
            "the row must count what is held, not what was ever seen"
        );
        assert!(definition.executions <= 2, "{}", definition.executions);
    }

    #[test]
    fn nodes_only_count_once_however_many_events_touch_them() {
        let mut run = Traversal::new("house-import", "exec-15");
        let events = vec![
            run.emit(
                EventType::StepStarted,
                "run-a",
                Some("a"),
                json!({ "node": "one" }),
            ),
            run.emit(
                EventType::ArtifactProduced,
                "run-a",
                None,
                json!({ "node": "one", "uri": "s3://b/x" }),
            ),
            run.after(10).emit(
                EventType::StepCompleted,
                "run-a",
                Some("a"),
                json!({ "node": "one" }),
            ),
        ];

        let summary = fold(&events).execution("exec-15").expect("held").summary;

        assert_eq!(summary.nodes_total, 1);
        assert_eq!(summary.nodes_succeeded, 1);
    }

    #[test]
    fn a_declaration_of_bare_strings_and_pairs_is_understood() {
        // The shortest thing somebody writes first. Refusing it would make the
        // simplest declaration the one that silently does nothing.
        let mut run = Traversal::new("tiny", "exec-16");
        let events = vec![run.emit(
            EventType::WorkflowDeclared,
            "run-a",
            None,
            json!({ "nodes": ["a", "b"], "edges": [["a", "b"]] }),
        )];

        let state = fold(&events);
        let definition = state.workflow("tiny").expect("declared");

        assert_eq!(definition.nodes.len(), 2);
        assert_eq!(definition.nodes[0].id, "a");
        assert_eq!(definition.nodes[0].name, "a");
        assert_eq!(
            definition.edges,
            vec![WorkflowEdge {
                from: "a".to_owned(),
                to: "b".to_owned(),
                label: None,
            }]
        );
    }

    #[test]
    fn a_declaration_carrying_no_nodes_does_not_wipe_the_stored_shape() {
        let mut run = Traversal::new("house-import", "exec-17");
        let events = vec![
            run.emit(EventType::WorkflowDeclared, "run-a", None, declaration()),
            run.emit(
                EventType::WorkflowDeclared,
                "run-a",
                None,
                json!({ "workflow_id": "house-import" }),
            ),
        ];

        let state = fold(&events);
        assert_eq!(
            state
                .workflow("house-import")
                .expect("declared")
                .nodes
                .len(),
            4
        );
    }
}
