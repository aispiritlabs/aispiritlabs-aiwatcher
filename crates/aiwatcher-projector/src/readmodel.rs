//! What the panel lists, without querying a trace store.
//!
//! A trace store answers "show me this trace". A panel also needs "which runs
//! failed in the last hour", "what did this conversation cost", "is this run
//! still going" — questions a waterfall view cannot answer and that would be a
//! full scan in VictoriaTraces.
//!
//! So the projector keeps a small, bounded, in-memory projection alongside the
//! trace writes. It is a cache of the log, not a source of truth: a restart
//! rebuilds it by replaying, and anything evicted is still in Laser and in
//! VictoriaTraces.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tokio::sync::RwLock;

use aiwatcher_core::ports::CompletedSpan;
use aiwatcher_core::{Checkpoint, EventType, Phase, RecordedEvent, Subject, TraceId};

use crate::evaluations::{
    EvaluationConfig, EvaluationDetail, EvaluationFilter, EvaluationPage, EvaluationState,
    SuitePage,
};
use crate::measured::{MeasuredRuns, MeasuredState};
use crate::workflows::{
    ExecutionDetail, ExecutionFilter, ExecutionPage, WorkflowConfig, WorkflowDefinition,
    WorkflowFilter, WorkflowPage, WorkflowState,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Started and not yet finished. Also what a run looks like while its
    /// producer is mid-flight.
    #[default]
    Running,
    Succeeded,
    Failed,
}

/// How many workflow nodes a run keeps the names of. A traversal with more
/// is a graph the workflow fold draws; a run keeps enough to say whether it
/// stepped outside the one a variant pins.
pub const MAX_NODES_RUN: usize = 64;

/// How many step starts and ends of workflow nodes a run keeps, in log order —
/// what the order of its traversal is read from.
pub const MAX_NODE_STEPS: usize = 256;

/// One step of a workflow node, as the run's log holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", content = "node", rename_all = "snake_case")]
pub enum NodeStep {
    Started(String),
    Completed(String),
    Failed(String),
}

/// One row in the runs table.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RunSummary {
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    pub trace_id: TraceId,
    pub status: RunStatus,
    pub agents: Vec<String>,
    /// Every producing service seen on this run, in first-seen order.
    ///
    /// A run is normally one process, but a handoff between two services keeps
    /// the same `run_id`, and that is exactly the case worth being able to see.
    #[serde(default)]
    pub runtimes: Vec<String>,
    /// The orchestration this run executes, when the producer names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
    /// The declared variant that answered in this run, when the producer names
    /// one — the first an event of it carried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant_id: Option<String>,
    /// The published result this run answered a case for, from `run.started`'s
    /// `evaluation_id`: a run made for a measurement rather than for somebody
    /// using the application, which is what keeps a benchmark out of what a
    /// variant was observed doing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluation_id: Option<String>,
    /// The credential the ingest route authenticated the run's start under — an
    /// ingest token's name, a person's subject. Absent where a broker delivered
    /// it, and nothing here authenticated who did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_by: Option<String>,
    /// Which project this run belongs to (ADR_0033), from the **first** event
    /// folded into it — including when that first event carried none, which is
    /// the global side and every run this build has held.
    ///
    /// Set once and never moved. A run id is a producer's text, so two
    /// credentials can publish into one; first-seen-wins means a second cannot
    /// take a run into its project, which is the rule ADR_0033 states for an
    /// execution's owner in the form a fold can keep. Nothing here refuses the
    /// second — a fold has nobody to refuse to — so what it does is leave the
    /// run where it was.
    ///
    /// This row is a **fact**, not a decision: it says whose a run is, and
    /// says nothing about who may read it. That is a grant, asked of IAM per
    /// request, and it is IAM-02/E3's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<aiwatcher_core::ProjectScope>,
    /// The run whose model call this run served, from `run.started`'s
    /// `caller_run_id`: a serving host saying which request it answered, so a
    /// call can be seen from the side that served it as well as the side that
    /// made it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_run_id: Option<String>,
    /// The shape of the workflow this run declared, as
    /// [`aiwatcher_core::topology::Topology::digest`] reads its own
    /// `workflow.declared` — node IDs and edges, never the producer's version
    /// string. The last declaration naming a node wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_topology: Option<String>,
    /// The workflow nodes this run started a step of, in first-seen order, at
    /// most [`MAX_NODES_RUN`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes_run: Vec<String>,
    /// Each node step's start and end, in log order, at most
    /// [`MAX_NODE_STEPS`]: the order a run went through its workflow. Kept for
    /// the traces step and never listed — a runs page does not need it.
    #[serde(skip)]
    pub node_steps: Vec<NodeStep>,
    /// Whether it took more node steps than it keeps, so the order of the rest
    /// was never read.
    #[serde(skip)]
    pub node_steps_dropped: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    /// The newest event folded into this row, ended or not.
    ///
    /// The one number that separates a run still working from a run whose
    /// producer stopped talking. Nothing here promotes the second case to a
    /// status — a projector that decided a run had died would be guessing
    /// about a process it cannot see, and the guess would be wrong for every
    /// agent that legitimately thinks for an hour. It reports when the run was
    /// last heard from and lets the reader draw the line.
    #[serde(with = "time::serde::rfc3339")]
    pub last_event_at: OffsetDateTime,
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub ended_at: Option<OffsetDateTime>,
    pub duration_ms: Option<i64>,
    pub event_count: u64,
    pub llm_calls: u64,
    pub tool_calls: u64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_tokens: i64,
    /// What the producers said this run's model calls cost, in US dollars —
    /// the sum of `llm.completed`'s `cost_usd`.
    ///
    /// A provider that reports a cost is reporting what it charged, which a
    /// price table can only estimate from tokens. Absent where no call
    /// reported one, never nought: a run whose cost nobody stated has an
    /// unknown cost, not a free one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// How many of this run's model calls reported that cost. Below
    /// `llm_calls`, the figure above is part of the bill rather than the bill.
    #[serde(default)]
    pub costed_calls: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The newest checkpoint folded into this row. A client can resume the
    /// live stream from here without re-reading the run.
    pub last_checkpoint: Checkpoint,
}

impl RunSummary {
    fn new(event: &RecordedEvent) -> Self {
        Self {
            run_id: event.metadata.run_id.clone(),
            conversation_id: event.metadata.conversation_id.clone(),
            trace_id: event.metadata.trace_id,
            status: RunStatus::Running,
            agents: Vec::new(),
            runtimes: Vec::new(),
            workflow: None,
            variant_id: None,
            evaluation_id: None,
            published_by: None,
            // Taken here rather than in `apply`, which is what makes it the
            // first event's and immune to a second credential.
            project: event.metadata.project,
            caller_run_id: None,
            workflow_topology: None,
            nodes_run: Vec::new(),
            node_steps: Vec::new(),
            node_steps_dropped: false,
            started_at: event.metadata.occurred_at,
            last_event_at: event.metadata.occurred_at,
            ended_at: None,
            duration_ms: None,
            event_count: 0,
            llm_calls: 0,
            tool_calls: 0,
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
            cost_usd: None,
            costed_calls: 0,
            error: None,
            last_checkpoint: Checkpoint::beginning(),
        }
    }

    fn apply(&mut self, event: &RecordedEvent) {
        self.event_count += 1;
        self.last_checkpoint = event.metadata.checkpoint.clone();
        if event.metadata.occurred_at < self.started_at {
            // Events can arrive slightly out of order across producers; keep
            // the earliest observation as the start.
            self.started_at = event.metadata.occurred_at;
        }
        // …and the latest as the last we heard from it. Same reason, other
        // end: a late-arriving event must not move the row backwards in time.
        self.last_event_at = self.last_event_at.max(event.metadata.occurred_at);
        if self.conversation_id.is_none() {
            self.conversation_id = event.metadata.conversation_id.clone();
        }
        if let Some(agent) = &event.metadata.agent_id
            && !self.agents.iter().any(|known| known == agent)
        {
            self.agents.push(agent.clone());
        }
        let runtime = &event.metadata.source.service;
        if !runtime.is_empty() && !self.runtimes.iter().any(|known| known == runtime) {
            self.runtimes.push(runtime.clone());
        }
        if self.workflow.is_none() {
            self.workflow = event.metadata.workflow_id.clone();
        }
        if self.variant_id.is_none() {
            self.variant_id = event.metadata.variant_id.clone();
        }

        let subject = event.event_type.subject();
        let phase = event.event_type.phase();

        if subject == Subject::Run && phase == Some(Phase::Start) {
            if self.evaluation_id.is_none() {
                self.evaluation_id = event.data_str("evaluation_id").map(ToOwned::to_owned);
            }
            if self.published_by.is_none() {
                self.published_by = event.metadata.published_by.clone();
            }
            if self.caller_run_id.is_none() {
                self.caller_run_id = event.data_str("caller_run_id").map(ToOwned::to_owned);
            }
        }
        if event.event_type == EventType::WorkflowDeclared
            && let Some(shape) = aiwatcher_core::topology::Topology::read(&event.data)
        {
            self.workflow_topology = Some(shape.digest());
        }
        if subject == Subject::Step
            && phase == Some(Phase::Start)
            && self.nodes_run.len() < MAX_NODES_RUN
            && let Some(node) = event.data_str("node")
            && !self.nodes_run.iter().any(|known| known == node)
        {
            self.nodes_run.push(node.to_owned());
        }
        if subject == Subject::Step
            && let Some(node) = event.data_str("node")
        {
            let step = match phase {
                Some(Phase::Start) => Some(NodeStep::Started(node.to_owned())),
                Some(Phase::End { ok: true }) => Some(NodeStep::Completed(node.to_owned())),
                Some(Phase::End { ok: false }) => Some(NodeStep::Failed(node.to_owned())),
                _ => None,
            };
            if let Some(step) = step {
                if self.node_steps.len() < MAX_NODE_STEPS {
                    self.node_steps.push(step);
                } else {
                    self.node_steps_dropped = true;
                }
            }
        }

        if subject == Subject::Llm && phase == Some(Phase::Start) {
            self.llm_calls += 1;
        }
        if subject == Subject::Tool && phase == Some(Phase::Start) {
            self.tool_calls += 1;
        }

        if subject == Subject::Llm && matches!(phase, Some(Phase::End { ok: true })) {
            self.input_tokens += event
                .data_i64("prompt_tokens")
                .or_else(|| event.data_i64("input_tokens"))
                .unwrap_or(0);
            self.output_tokens += event
                .data_i64("completion_tokens")
                .or_else(|| event.data_i64("output_tokens"))
                .unwrap_or(0);
            self.cached_tokens += event.data_i64("cached_tokens").unwrap_or(0);
            // Summed only where it was reported. A call that said nothing about
            // what it cost leaves the total naming fewer calls than the run
            // made, which is the honest reading of a partial bill.
            if let Some(cost) = event.data_f64("cost_usd").filter(|cost| cost.is_finite()) {
                self.cost_usd = Some(self.cost_usd.unwrap_or(0.0) + cost);
                self.costed_calls += 1;
            }
        }

        // A managed execution never emits `run.completed`: the engine owns the
        // run and says so with `execution.*` (ADR_0026), whose end phases are
        // already in the catalog. Reading only `Subject::Run` left every such
        // run `Running` forever while the workflow fold beside it said
        // `succeeded` — two views of one run disagreeing. The guardrail this
        // sits under refuses to *infer* an ending from silence; this is not an
        // inference, it is the producer saying it finished in the vocabulary
        // its own ADR gave it.
        if matches!(subject, Subject::Run | Subject::Execution) {
            match phase {
                Some(Phase::End { ok: true }) => {
                    self.status = RunStatus::Succeeded;
                    self.finish(event.metadata.occurred_at);
                }
                Some(Phase::End { ok: false }) => {
                    self.status = RunStatus::Failed;
                    // `reason` is what `execution.failed` calls it — the
                    // engine's own word, and the only one it writes.
                    self.error = event
                        .data_str("error")
                        .or_else(|| event.data_str("message"))
                        .or_else(|| event.data_str("reason"))
                        .map(ToOwned::to_owned);
                    self.finish(event.metadata.occurred_at);
                }
                _ => {}
            }
        }

        // A failure anywhere marks the run failed even if `run.failed` never
        // arrives — a crashed producer is exactly the case where it will not.
        if matches!(
            event.event_type,
            EventType::AgentFailed | EventType::LlmFailed
        ) && self.status == RunStatus::Running
            && self.error.is_none()
        {
            self.error = event.data_str("error").map(ToOwned::to_owned);
        }
    }

    fn finish(&mut self, at: OffsetDateTime) {
        self.ended_at = Some(at);
        self.duration_ms = Some(((at - self.started_at).whole_milliseconds()).max(0) as i64);
    }
}

/// A run plus what is needed to draw it.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RunDetail {
    pub summary: RunSummary,
    /// Spans finished so far, in completion order. A running run has fewer
    /// spans than it eventually will — that is the point of a live view.
    #[schema(value_type = Vec<Object>)]
    pub spans: Vec<CompletedSpan>,
}

/// Filters for the runs list.
#[derive(Clone, Debug, Default, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct RunFilter {
    /// Only runs with activity in the last this-many seconds. See
    /// [`crate::window`] — zero and absent both mean everything.
    pub window_seconds: Option<i64>,
    /// The instant the window ends at, in seconds since the epoch.
    ///
    /// `None` is now, which is every ordinary read and every link somebody
    /// pastes. A managed step pins one so that a retry reads the same rows —
    /// see [`crate::window::bounds`].
    pub as_of: Option<i64>,
    pub conversation_id: Option<String>,
    pub agent_id: Option<String>,
    /// Runs produced by this service. See `RunSummary::runtimes`.
    pub runtime: Option<String>,
    pub workflow: Option<String>,
    /// Runs in which this declared variant answered.
    pub variant_id: Option<String>,
    /// Runs sharing one trace. Normally one run, but a producer that supplies
    /// its own `trace_id` can span several — the only view that shows it.
    pub trace_id: Option<String>,
    /// Runs that made at least one call to this model.
    ///
    /// Matched against the run's spans rather than its summary, because a run
    /// does not carry the models it used — the LLM spans do. Same for `tool`.
    pub model: Option<String>,
    /// Runs that invoked this tool.
    pub tool: Option<String>,
    /// Runs in which a call named this registered prompt.
    ///
    /// The prompt's *name*, never its text and never its version id: the text
    /// is off the log on purpose (ADR_0011) and a version is what the prompt's
    /// own page lists. Matched against the run's spans, as `model` and `tool`
    /// are.
    pub prompt: Option<String>,
    pub status: Option<RunStatus>,
    /// Cursor: return runs older than this one. Keyset pagination, because an
    /// offset shifts under a list that is actively growing.
    pub before: Option<String>,
    pub limit: Option<usize>,
}

impl RunFilter {
    /// The axes this read narrows by, as the one predicate every list shares.
    #[must_use]
    pub fn selection(&self) -> crate::selection::RunSelection<'_> {
        crate::selection::RunSelection {
            conversation_id: self.conversation_id.as_deref(),
            agent_id: self.agent_id.as_deref(),
            runtime: self.runtime.as_deref(),
            workflow: self.workflow.as_deref(),
            variant_id: self.variant_id.as_deref(),
            trace_id: self.trace_id.as_deref(),
            model: self.model.as_deref(),
            tool: self.tool.as_deref(),
            prompt: self.prompt.as_deref(),
            status: self.status,
        }
    }
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct RunPage {
    pub runs: Vec<RunSummary>,
    /// Pass as `before` to fetch the next page. Absent on the last page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub total_known: usize,
}

/// The read model is the only unbounded-by-nature thing in the process, so its
/// caps are what decide whether aiwatcher fits in a small container.
///
/// The defaults are sized for a **512 MB** limit with room for spikes. Measured
/// on a debug build at full retention with a realistic workload (two agents,
/// two LLM calls, two tool calls and 24 streamed chunks per run): ~150 MB
/// resident, of which the read model is the largest share. A release build is
/// smaller. `just load-test` reproduces the measurement.
///
/// **What a project costs** (IAM-02 E2, re-measured 2026-09-18 on the same
/// debug build and workload, 5 000 runs, one batch per run so the three are
/// comparable):
///
/// | projects | resident |
/// |---|---|
/// | none — the global side | 176 MB |
/// | one | 182 MB |
/// | fifty | 183 MB |
///
/// The cost is **per event**, not per project: a run's row gains two uuids and
/// each span two attributes, and the fifty-project deployment costs a megabyte
/// more than the one-project deployment because the fold is one fold with the
/// scope in the row rather than one fold per tenant. The 512 MB limit in
/// `deploy/` stands on this measurement with ~2.8× headroom, so nothing there
/// moves; what would move it is raising `max_spans_total`, as it always was.
/// (The figures are above the older ~150 MB because the workload now posts one
/// batch per run rather than batches of 600, which is the producer shape a
/// scoped deployment has — one credential per project.)
#[derive(Clone, Debug)]
pub struct ReadModelConfig {
    /// How many runs to keep. Past it, the oldest *finished* runs are evicted
    /// first — a running run is never dropped out from under a live viewer.
    pub max_runs: usize,
    /// Spans retained per run for the waterfall. A run with more spans than
    /// this is already past what a waterfall can show.
    pub max_spans_per_run: usize,
    /// Spans retained across **all** runs.
    ///
    /// The per-run cap alone is not a memory bound: `max_runs` multiplied by
    /// `max_spans_per_run` is the real exposure, and at the old defaults that
    /// was ten million spans — gigabytes. This is the cap that makes the
    /// footprint predictable, by evicting the oldest finished runs' spans when
    /// the total is exceeded.
    pub max_spans_total: usize,
    /// What the evaluation projection may hold. Separate caps because a report
    /// is a producer-supplied document and a run is not.
    pub evaluations: EvaluationConfig,
    /// What the workflow projection may hold. Separate again, because a graph
    /// is held per *execution* and an execution can outlive several runs.
    pub workflows: WorkflowConfig,
}

impl Default for ReadModelConfig {
    fn default() -> Self {
        Self {
            max_runs: 5_000,
            max_spans_per_run: 500,
            max_spans_total: 60_000,
            evaluations: EvaluationConfig::default(),
            workflows: WorkflowConfig::default(),
        }
    }
}

/// Whether any of a run's spans carries `key = wanted`.
///
/// A run with no retained spans does not match: the alternative — treating
/// "unknown" as "matches" — would put runs into a model or tool bucket they may
/// have nothing to do with.
#[derive(Debug, Default)]
struct State {
    runs: HashMap<String, RunSummary>,
    spans: HashMap<String, Vec<CompletedSpan>>,
    /// Run ids in first-seen order; the eviction candidate list.
    order: Vec<String>,
    /// Running total across `spans`, so the global cap is checked without
    /// walking every run on every write.
    span_count: usize,
    /// Evaluations. Folded apart from runs — see [`crate::evaluations`].
    evaluations: EvaluationState,
    /// Workflow graphs. Folded *alongside* runs rather than apart from them —
    /// see [`ReadModel::apply`].
    workflows: WorkflowState,
    /// What clients counted of the runs they opened for measurements — see
    /// [`crate::measured`].
    measured: MeasuredState,
}

/// The panel's projection of the log.
#[derive(Debug)]
pub struct ReadModel {
    state: RwLock<State>,
    config: ReadModelConfig,
}

impl Default for ReadModel {
    fn default() -> Self {
        Self::new(ReadModelConfig::default())
    }
}

impl ReadModel {
    #[must_use]
    pub fn new(config: ReadModelConfig) -> Self {
        Self {
            state: RwLock::new(State::default()),
            config,
        }
    }

    /// Fold one event in. Idempotent for everything except the counters, which
    /// the pipeline's deduplicator protects.
    pub async fn apply(&self, event: &RecordedEvent) {
        let mut state = self.state.write().await;
        if event.event_type.subject() == Subject::Eval {
            // An evaluation is an execution, but it is not an agent run: no
            // agents, no LLM calls, no tokens. Folding it into the runs list
            // would put an empty row in the view people scan for what their
            // agents did, so it gets its own projection. It is also not a
            // workflow node, and `workflow_id` on an `eval.*` event is the
            // suite's fallback name — folding it below would invent an
            // execution out of a report.
            state.evaluations.apply(event, &self.config.evaluations);
            return;
        }
        if event.event_type.subject() == Subject::Client {
            // A client's count of its runs: its `run_id` names the client, and
            // a row for it in the runs list would be a run nobody opened.
            state.measured.apply(event);
            return;
        }
        if event.event_type == EventType::RunStarted {
            state.measured.apply(event);
        }
        let run_id = event.metadata.run_id.clone();
        if !state.runs.contains_key(&run_id) {
            state.order.push(run_id.clone());
            state.runs.insert(run_id.clone(), RunSummary::new(event));
        }
        if let Some(summary) = state.runs.get_mut(&run_id) {
            summary.apply(event);
        }
        // Alongside the runs fold, not instead of it. A workflow event is
        // still an event on a real run — the graph is a second way of reading
        // the same stream, not a separate stream. Everything without a
        // `workflow_run_id` returns immediately inside `apply`.
        state.workflows.apply(event, &self.config.workflows);
        Self::evict(&mut state, self.config.max_runs);
    }

    /// Attach finished spans to their run.
    pub async fn record_spans(&self, spans: &[CompletedSpan]) {
        if spans.is_empty() {
            return;
        }
        let mut state = self.state.write().await;
        for span in spans {
            let Some(run_id) = span.attributes.iter().find_map(|(key, value)| {
                (key == aiwatcher_core::attrs::aiwatcher::run::ID)
                    .then(|| match value {
                        aiwatcher_core::ports::AttrValue::Str(inner) => Some(inner.clone()),
                        _ => None,
                    })
                    .flatten()
            }) else {
                continue;
            };
            // The bucket borrow and the running total cannot be held at once,
            // so the deltas are computed first and applied after.
            let (added, trimmed) = {
                let bucket = state.spans.entry(run_id).or_default();
                // A replay writes the same span id again; replace rather than
                // append, so the waterfall does not grow duplicates.
                let added = match bucket
                    .iter_mut()
                    .find(|candidate| candidate.span_id == span.span_id)
                {
                    Some(existing) => {
                        *existing = span.clone();
                        0
                    }
                    None => {
                        bucket.push(span.clone());
                        1
                    }
                };
                let trimmed = bucket.len().saturating_sub(self.config.max_spans_per_run);
                if trimmed > 0 {
                    bucket.drain(0..trimmed);
                }
                (added, trimmed)
            };
            state.span_count = state.span_count + added - trimmed;
        }
        Self::shed_spans(&mut state, self.config.max_spans_total);
    }

    pub async fn run(&self, run_id: &str) -> Option<RunDetail> {
        let state = self.state.read().await;
        let summary = state.runs.get(run_id)?.clone();
        let mut spans = state.spans.get(run_id).cloned().unwrap_or_default();
        // Waterfall order: by start time, then by span id for stability.
        spans.sort_by(|a, b| {
            a.start
                .cmp(&b.start)
                .then_with(|| a.span_id.to_hex().cmp(&b.span_id.to_hex()))
        });
        Some(RunDetail { summary, spans })
    }

    /// The runs that say they served a call one of `callers` made — a serving
    /// host's run naming the request it answered, by `caller_run_id` — keyed by
    /// the run they served. One pass over what is held.
    pub async fn serving(
        &self,
        callers: &std::collections::BTreeSet<&str>,
    ) -> std::collections::BTreeMap<String, Vec<RunDetail>> {
        let state = self.state.read().await;
        let mut found: std::collections::BTreeMap<String, Vec<RunDetail>> =
            std::collections::BTreeMap::new();
        for summary in state.runs.values() {
            let Some(caller) = summary
                .caller_run_id
                .as_deref()
                .filter(|caller| callers.contains(caller))
            else {
                continue;
            };
            found.entry(caller.to_owned()).or_default().push(RunDetail {
                summary: summary.clone(),
                spans: state
                    .spans
                    .get(&summary.run_id)
                    .cloned()
                    .unwrap_or_default(),
            });
        }
        found
    }

    /// The runs a witness relayed a model call in — a span carrying its
    /// digests of what the request asked — that started at or after `from`,
    /// whichever run each names as its caller, if any. One pass over what is
    /// held.
    pub async fn asked_since(&self, from: OffsetDateTime) -> Vec<RunDetail> {
        let state = self.state.read().await;
        state
            .runs
            .values()
            .filter(|summary| summary.started_at >= from)
            .filter_map(|summary| {
                let spans = state.spans.get(&summary.run_id)?;
                spans
                    .iter()
                    .any(|span| {
                        span.attributes.iter().any(|(name, _)| {
                            name == aiwatcher_core::attrs::aiwatcher::witness::ASKED
                        })
                    })
                    .then(|| RunDetail {
                        summary: summary.clone(),
                        spans: spans.clone(),
                    })
            })
            .collect()
    }

    /// What each client counted of the runs it opened for one measurement, at
    /// each attempt of generating its answers.
    ///
    /// `project` is the project whose measurement is being asked about —
    /// `None` for the global side, which is every one a production caller asks
    /// about today (ADR_0033: no production caller constructs a bound store).
    pub async fn measured_runs(
        &self,
        project: Option<aiwatcher_core::ProjectScope>,
        evaluation_id: &str,
    ) -> Vec<MeasuredRuns> {
        self.state.read().await.measured.of(project, evaluation_id)
    }

    pub async fn list(&self, filter: &RunFilter) -> RunPage {
        self.list_at(filter, OffsetDateTime::now_utc()).await
    }

    /// The runs list with the clock passed in, so the window is testable.
    pub async fn list_at(&self, filter: &RunFilter, now: OffsetDateTime) -> RunPage {
        let state = self.state.read().await;
        let limit = filter.limit.unwrap_or(50).clamp(1, 500);
        let window =
            crate::window::bounds(filter.window_seconds, crate::window::at(filter.as_of), now);

        // Newest first.
        let selection = filter.selection();
        let mut matching: Vec<&RunSummary> = state
            .order
            .iter()
            .rev()
            .filter_map(|run_id| state.runs.get(run_id))
            // Last activity, not start: a run that began before the window and
            // is still emitting is the one most worth seeing in it.
            .filter(|run| window.holds(run.last_event_at))
            .filter(|run| selection.matches(run, state.spans.get(&run.run_id)))
            .collect();

        if let Some(cursor) = &filter.before
            && let Some(index) = matching.iter().position(|run| &run.run_id == cursor)
        {
            matching.drain(0..=index);
        }

        let total_known = matching.len();
        let page: Vec<RunSummary> = matching.into_iter().take(limit).cloned().collect();
        let next_cursor = (total_known > page.len())
            .then(|| page.last().map(|run| run.run_id.clone()))
            .flatten();

        RunPage {
            runs: page,
            next_cursor,
            total_known,
        }
    }

    /// Conversations: the level above a run.
    pub async fn conversations(
        &self,
        filter: &crate::conversations::ConversationFilter,
    ) -> crate::conversations::ConversationPage {
        let state = self.state.read().await;
        let runs: Vec<RunSummary> = state.runs.values().cloned().collect();
        crate::conversations::compute(&runs, &state.spans, filter, OffsetDateTime::now_utc())
    }

    /// One dimension's rows: the explorer's top level, whatever it is rooted on.
    pub async fn dimensions(
        &self,
        kind: crate::dimensions::DimensionKind,
        filter: &crate::dimensions::DimensionFilter,
    ) -> crate::dimensions::DimensionPage {
        let state = self.state.read().await;
        let runs: Vec<RunSummary> = state.runs.values().cloned().collect();
        crate::dimensions::compute(&runs, &state.spans, kind, filter, OffsetDateTime::now_utc())
    }

    /// What each of these variants was observed doing, from the runs held
    /// that name it. See [`crate::observations`].
    pub async fn variant_observations(
        &self,
        variant_ids: &[&str],
        window_seconds: Option<i64>,
        prices: Option<&aiwatcher_core::prices::ModelPrices>,
    ) -> Vec<crate::observations::VariantObservations> {
        let state = self.state.read().await;
        crate::observations::compute(
            state.runs.values(),
            &state.spans,
            variant_ids,
            window_seconds,
            OffsetDateTime::now_utc(),
            prices,
        )
    }

    /// Every retained span, flat and filterable. See [`crate::spans`].
    pub async fn spans(&self, filter: &crate::spans::SpanFilter) -> crate::spans::SpanPage {
        let state = self.state.read().await;
        crate::spans::compute(&state.spans, filter, OffsetDateTime::now_utc())
    }

    /// Evaluation reports, newest first. See [`crate::evaluations`].
    pub async fn evaluations(&self, filter: &EvaluationFilter) -> EvaluationPage {
        self.state
            .read()
            .await
            .evaluations
            .page(filter, OffsetDateTime::now_utc())
    }

    /// One evaluation, with its cases, its report and its baseline.
    pub async fn evaluation(&self, evaluation_id: &str) -> Option<EvaluationDetail> {
        self.state.read().await.evaluations.detail(evaluation_id)
    }

    /// Resolve both reports under the same read lock.
    pub async fn evaluation_with_baseline(
        &self,
        evaluation_id: &str,
        baseline_id: Option<&str>,
    ) -> Option<EvaluationDetail> {
        self.state
            .read()
            .await
            .evaluations
            .detail_with_baseline(evaluation_id, baseline_id)
    }

    /// Suites: the level above an evaluation report.
    /// The workflow catalog: every declared graph, and the ones only observed.
    pub async fn workflows(&self, filter: &WorkflowFilter) -> WorkflowPage {
        self.state
            .read()
            .await
            .workflows
            .workflows(filter, OffsetDateTime::now_utc())
    }

    pub async fn workflow(&self, workflow_id: &str) -> Option<WorkflowDefinition> {
        self.state.read().await.workflows.workflow(workflow_id)
    }

    pub async fn workflow_executions(&self, filter: &ExecutionFilter) -> ExecutionPage {
        self.state
            .read()
            .await
            .workflows
            .executions(filter, OffsetDateTime::now_utc())
    }

    pub async fn workflow_execution(&self, workflow_run_id: &str) -> Option<ExecutionDetail> {
        self.state.read().await.workflows.execution(workflow_run_id)
    }

    pub async fn legacy_evaluations(
        &self,
        filter: &EvaluationFilter,
        excluded: &std::collections::BTreeSet<String>,
    ) -> EvaluationPage {
        self.state.read().await.evaluations.page_excluding(
            filter,
            OffsetDateTime::now_utc(),
            excluded,
        )
    }
    pub async fn legacy_evaluation(
        &self,
        id: &str,
        baseline: Option<&str>,
        excluded: &std::collections::BTreeSet<String>,
    ) -> Option<EvaluationDetail> {
        self.state
            .read()
            .await
            .evaluations
            .detail_excluding(id, baseline, excluded)
    }
    pub async fn legacy_evaluation_suites(
        &self,
        excluded: &std::collections::BTreeSet<String>,
    ) -> SuitePage {
        self.state
            .read()
            .await
            .evaluations
            .suites_excluding(excluded)
    }

    pub async fn evaluation_suites(&self) -> SuitePage {
        self.state.read().await.evaluations.suites()
    }

    /// The metrics view: a fold over everything currently retained.
    ///
    /// Held under the read lock for the duration, which is fine because it is a
    /// pass over in-memory data with no I/O in it.
    pub async fn metrics(
        &self,
        filter: &crate::metrics::MetricsFilter,
    ) -> crate::metrics::MetricsSummary {
        let state = self.state.read().await;
        let runs: Vec<RunSummary> = state.runs.values().cloned().collect();
        crate::metrics::compute(
            &runs,
            &state.spans,
            filter,
            self.config.max_runs,
            OffsetDateTime::now_utc(),
        )
    }

    /// How many runs are currently held.
    #[must_use]
    pub async fn len(&self) -> usize {
        self.state.read().await.runs.len()
    }

    #[must_use]
    pub async fn is_empty(&self) -> bool {
        self.state.read().await.runs.is_empty()
    }

    /// Drop whole runs' spans, oldest first, until the global budget is met.
    ///
    /// Spans go before the runs themselves: a run without its spans still shows
    /// its summary, its token counts and its status, so the list and the
    /// metrics stay complete while the waterfall for an old run is what is
    /// given up. A running run keeps its spans — that is the one someone is
    /// most likely to be looking at.
    fn shed_spans(state: &mut State, max_spans_total: usize) {
        if state.span_count <= max_spans_total {
            return;
        }
        for run_id in state.order.clone() {
            if state.span_count <= max_spans_total {
                break;
            }
            let running = state
                .runs
                .get(&run_id)
                .is_some_and(|run| run.status == RunStatus::Running);
            if running {
                continue;
            }
            if let Some(dropped) = state.spans.remove(&run_id) {
                state.span_count = state.span_count.saturating_sub(dropped.len());
            }
        }
    }

    /// Drop the oldest finished runs once over the cap. Running runs are kept:
    /// evicting one would blank the page someone is watching.
    fn evict(state: &mut State, max_runs: usize) {
        if state.runs.len() <= max_runs {
            return;
        }
        let mut excess = state.runs.len() - max_runs;
        let mut keep = Vec::with_capacity(state.order.len());
        for run_id in std::mem::take(&mut state.order) {
            let finished = state
                .runs
                .get(&run_id)
                .is_some_and(|run| run.status != RunStatus::Running);
            if excess > 0 && finished {
                state.runs.remove(&run_id);
                if let Some(dropped) = state.spans.remove(&run_id) {
                    state.span_count = state.span_count.saturating_sub(dropped.len());
                }
                excess -= 1;
            } else {
                keep.push(run_id);
            }
        }
        state.order = keep;
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use aiwatcher_core::attrs::aiwatcher as own;
    use aiwatcher_core::ports::{SpanKind, SpanStatus, attr};
    use aiwatcher_core::{Checkpoint, SpanId, TraceId};

    use super::*;

    fn summary(run_id: &str, status: RunStatus) -> RunSummary {
        RunSummary {
            run_id: run_id.to_owned(),
            conversation_id: None,
            trace_id: TraceId::derive(run_id),
            status,
            agents: Vec::new(),
            runtimes: Vec::new(),
            workflow: None,
            variant_id: None,
            evaluation_id: None,
            published_by: None,
            project: None,
            caller_run_id: None,
            workflow_topology: None,
            nodes_run: Vec::new(),
            node_steps: Vec::new(),
            node_steps_dropped: false,
            started_at: datetime!(2026-08-27 18:20:00 UTC),
            last_event_at: datetime!(2026-08-27 18:20:00 UTC),
            ended_at: None,
            duration_ms: Some(1),
            event_count: 1,
            llm_calls: 0,
            tool_calls: 0,
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
            cost_usd: None,
            costed_calls: 0,
            error: None,
            last_checkpoint: Checkpoint::from_global_position(1),
        }
    }

    fn span(run_id: &str, index: usize) -> CompletedSpan {
        let trace_id = TraceId::derive(run_id);
        let start = datetime!(2026-08-27 18:20:00 UTC);
        CompletedSpan {
            trace_id,
            span_id: SpanId::derive(trace_id, &format!("s{index}")),
            parent_span_id: None,
            name: format!("span-{index}"),
            kind: SpanKind::Internal,
            start,
            end: start + time::Duration::milliseconds(1),
            status: SpanStatus::Ok,
            attributes: vec![attr(own::run::ID, run_id)],
            events: Vec::new(),
            links: Vec::new(),
        }
    }

    async fn seed(model: &ReadModel, run_id: &str, status: RunStatus, spans: usize) {
        let mut state = model.state.write().await;
        state.order.push(run_id.to_owned());
        state
            .runs
            .insert(run_id.to_owned(), summary(run_id, status));
        drop(state);
        let batch: Vec<CompletedSpan> = (0..spans).map(|index| span(run_id, index)).collect();
        model.record_spans(&batch).await;
    }

    /// A run is in the window when it was last *heard from* in it.
    ///
    /// Start would have been the easy predicate and the wrong one: the run
    /// worth seeing in a fifteen-minute view is the one that began an hour ago
    /// and is still talking, and windowing on start is exactly what hides it.
    #[tokio::test]
    async fn the_window_keeps_an_old_run_that_is_still_emitting_and_drops_a_quiet_one() {
        let model = ReadModel::default();
        let now = datetime!(2026-08-29 12:00:00 UTC);

        let mut talkative = summary("talkative", RunStatus::Running);
        talkative.started_at = now - time::Duration::hours(3);
        talkative.last_event_at = now - time::Duration::minutes(2);
        let mut quiet = summary("quiet", RunStatus::Succeeded);
        quiet.started_at = now - time::Duration::hours(3);
        quiet.last_event_at = now - time::Duration::hours(2);

        {
            let mut state = model.state.write().await;
            for run in [talkative, quiet] {
                state.order.push(run.run_id.clone());
                state.runs.insert(run.run_id.clone(), run);
            }
        }

        let page = model
            .list_at(
                &RunFilter {
                    window_seconds: Some(900),
                    ..RunFilter::default()
                },
                now,
            )
            .await;

        assert_eq!(page.total_known, 1);
        assert_eq!(page.runs[0].run_id, "talkative");
    }

    #[tokio::test]
    async fn a_window_of_zero_lists_everything_rather_than_nothing() {
        let model = ReadModel::default();
        seed(&model, "run-1", RunStatus::Succeeded, 0).await;

        let page = model
            .list_at(
                &RunFilter {
                    window_seconds: Some(0),
                    ..RunFilter::default()
                },
                OffsetDateTime::now_utc(),
            )
            .await;

        assert_eq!(page.total_known, 1);
    }

    /// The cap that makes the footprint predictable.
    ///
    /// `max_runs * max_spans_per_run` is the real exposure and it is enormous;
    /// without a global budget a handful of pathological runs can hold more
    /// memory than the whole process is allowed.
    #[tokio::test]
    async fn spans_are_shed_once_the_global_budget_is_exceeded() {
        let model = ReadModel::new(ReadModelConfig {
            max_runs: 100,
            max_spans_per_run: 100,
            max_spans_total: 20,
            ..ReadModelConfig::default()
        });

        for index in 0..4 {
            seed(&model, &format!("run-{index}"), RunStatus::Succeeded, 10).await;
        }

        let held = model.state.read().await.span_count;
        assert!(held <= 20, "span_count {held} should be within the budget");
        assert!(
            model
                .run("run-3")
                .await
                .is_some_and(|d| !d.spans.is_empty()),
            "the newest run keeps its spans"
        );
        assert!(
            model.run("run-0").await.is_some_and(|d| d.spans.is_empty()),
            "the oldest run gives up its spans first"
        );
        assert!(
            model.run("run-0").await.is_some(),
            "but the run itself survives, so the list and the metrics stay complete"
        );
    }

    #[tokio::test]
    async fn a_running_run_keeps_its_spans_even_under_pressure() {
        let model = ReadModel::new(ReadModelConfig {
            max_runs: 100,
            max_spans_per_run: 100,
            max_spans_total: 15,
            ..ReadModelConfig::default()
        });

        seed(&model, "live", RunStatus::Running, 10).await;
        for index in 0..3 {
            seed(&model, &format!("done-{index}"), RunStatus::Succeeded, 10).await;
        }

        assert!(
            model.run("live").await.is_some_and(|d| d.spans.len() == 10),
            "the run someone is most likely watching is not the one to strip"
        );
    }

    #[tokio::test]
    async fn the_per_run_cap_trims_and_keeps_the_total_honest() {
        let model = ReadModel::new(ReadModelConfig {
            max_runs: 10,
            max_spans_per_run: 5,
            max_spans_total: 1_000,
            ..ReadModelConfig::default()
        });
        seed(&model, "chatty", RunStatus::Succeeded, 40).await;

        let detail = model.run("chatty").await.expect("the run");
        assert_eq!(detail.spans.len(), 5, "trimmed to the per-run cap");
        assert_eq!(
            model.state.read().await.span_count,
            5,
            "and the running total matches what is actually held"
        );
    }

    /// A report a managed step recorded carries that run's `workflow_run_id`,
    /// and the workflow fold keys executions by exactly that field — which is
    /// why `apply` routes every `eval.*` event away before it.
    #[tokio::test]
    async fn a_report_a_step_recorded_starts_no_workflow_execution() {
        use aiwatcher_core::{EventEnvelope, Sdk, Source};

        let model = ReadModel::new(ReadModelConfig::default());
        let at = datetime!(2026-09-11 09:00:00 UTC);
        for event_type in [EventType::EvalStarted, EventType::EvalCompleted] {
            let mut envelope =
                EventEnvelope::new(event_type, "eval-1", at, Source::new("worker", Sdk::Python))
                    .with_data(
                        serde_json::json!({ "suite": "held-out", "step_id": "evaluate_baseline" }),
                    );
            // Both, as a step's report carries them: exactly what the workflow
            // fold would key a new execution by, were the report to reach it.
            envelope.workflow_id = Some("optimise-prompt".to_owned());
            envelope.workflow_run_id = Some("nobody-declared-this".to_owned());
            model.apply(&envelope.record(1, 1, at, None)).await;
        }

        let page = model.workflow_executions(&ExecutionFilter::default()).await;
        assert!(
            page.executions.is_empty(),
            "a report is not a node, and the run it names is no execution of its own"
        );
        assert!(
            model.run("eval-1").await.is_none(),
            "nor is it an agent run"
        );
    }

    fn scope(last: u8) -> aiwatcher_core::ProjectScope {
        aiwatcher_core::ProjectScope::new(
            uuid::Uuid::parse_str("0198c0de-0000-7000-8000-00000000000a").expect("uuid"),
            uuid::Uuid::parse_str(&format!("0198c0de-0000-7000-8000-0000000000{last:02x}"))
                .expect("uuid"),
        )
    }

    /// A run's project is the first event's, and a second credential
    /// publishing into the same run id does not take it into another project.
    #[tokio::test]
    async fn a_run_belongs_to_the_project_its_first_event_carried_and_is_never_moved() {
        use aiwatcher_core::{EventEnvelope, Sdk, Source};

        let model = ReadModel::new(ReadModelConfig::default());
        let at = datetime!(2026-09-18 09:00:00 UTC);
        let source = Source::new("agent", Sdk::Python);
        for (position, project) in [(1, Some(scope(0xaa))), (2, Some(scope(0xbb))), (3, None)] {
            let mut event = EventEnvelope::new(EventType::RunStarted, "run-1", at, source.clone());
            event.project = project;
            model
                .apply(&event.record(position, position, at, None))
                .await;
        }

        // A run id is a producer's text, so two credentials can publish into
        // one. First-seen-wins is what stops the second taking the run — the
        // fold's form of the rule ADR_0033 states for an execution's owner.
        let run = model.run("run-1").await.expect("the run").summary;
        assert_eq!(run.project, Some(scope(0xaa)));
        assert_eq!(run.event_count, 3, "and every event still counts in it");
    }

    /// A run published under no project reads exactly as every run this build
    /// has ever held, and says nothing about a project in its JSON.
    #[tokio::test]
    async fn a_run_with_no_project_is_the_global_side_and_gains_no_field() {
        use aiwatcher_core::{EventEnvelope, Sdk, Source};

        let model = ReadModel::new(ReadModelConfig::default());
        let at = datetime!(2026-09-18 09:00:00 UTC);
        let event = EventEnvelope::new(
            EventType::RunStarted,
            "run-global",
            at,
            Source::new("agent", Sdk::Python),
        );
        model.apply(&event.record(1, 1, at, None)).await;

        let run = model.run("run-global").await.expect("the run").summary;
        assert_eq!(run.project, None);
        assert!(
            serde_json::to_value(&run)
                .expect("encodes")
                .get("project")
                .is_none(),
            "so no stored or served shape moves for a deployment with no projects"
        );
    }

    /// A client's count of its runs is no run of its own, and the measurement
    /// it counts for reads it beside the starts that arrived.
    #[tokio::test]
    async fn a_client_s_count_lists_no_run_and_is_read_beside_a_measurement_s_starts() {
        use aiwatcher_core::{EventEnvelope, Sdk, Source};

        let model = ReadModel::new(ReadModelConfig::default());
        let at = datetime!(2026-09-14 09:00:00 UTC);
        let mut source = Source::new("worker", Sdk::Python);
        source.client = Some("worker-1".to_owned());
        let mut start = EventEnvelope::new(EventType::RunStarted, "generate-1", at, source.clone())
            .with_data(serde_json::json!({ "evaluation_id": "e1", "generation_attempt": 1 }));
        start.run_sequence = Some(1);
        start.variant_id = Some("v1".to_owned());
        model.apply(&start.record(1, 1, at, None)).await;
        let mut counted =
            EventEnvelope::new(EventType::ClientCounted, "client-worker-1", at, source).with_data(
                serde_json::json!({ "evaluation_id": "e1", "generation_attempt": 1, "runs": 2 }),
            );
        counted.variant_id = Some("v1".to_owned());
        model.apply(&counted.record(2, 2, at, None)).await;

        assert!(model.run("client-worker-1").await.is_none());
        assert_eq!(model.len().await, 1);
        assert_eq!(
            model.measured_runs(None, "e1").await,
            [crate::MeasuredRuns {
                client: "worker-1".to_owned(),
                attempt: Some(1),
                opened: 2,
                arrived: 1,
            }]
        );
    }

    /// A run names its variant on its events and, when a measurement made it,
    /// the result it answered for on its start — and a list narrowed to the
    /// variant finds it, while what the variant was observed doing leaves the
    /// measurement's run out.
    #[tokio::test]
    async fn a_run_s_variant_and_the_measurement_that_made_it_are_folded_from_its_events() {
        use aiwatcher_core::{EventEnvelope, Sdk, Source};

        let model = ReadModel::new(ReadModelConfig::default());
        let at = datetime!(2026-09-13 09:00:00 UTC);
        for (run_id, measured) in [("served", None), ("benchmark", Some("answers-v1"))] {
            for (position, event_type) in [(1, EventType::RunStarted), (2, EventType::RunCompleted)]
            {
                let data = match (&event_type, measured) {
                    (EventType::RunStarted, Some(evaluation)) => {
                        serde_json::json!({ "evaluation_id": evaluation })
                    }
                    _ => serde_json::json!({}),
                };
                let mut envelope =
                    EventEnvelope::new(event_type, run_id, at, Source::new("bot", Sdk::Python))
                        .with_data(data);
                envelope.variant_id = Some("v1".to_owned());
                model
                    .apply(&envelope.record(position, position, at, None))
                    .await;
            }
        }

        let page = model
            .list_at(
                &RunFilter {
                    variant_id: Some("v1".to_owned()),
                    ..RunFilter::default()
                },
                at,
            )
            .await;
        assert_eq!(page.total_known, 2);
        let benchmark = model.run("benchmark").await.expect("folded").summary;
        assert_eq!(benchmark.variant_id.as_deref(), Some("v1"));
        assert_eq!(benchmark.evaluation_id.as_deref(), Some("answers-v1"));

        let observed = model.variant_observations(&["v1"], None, None).await;
        assert_eq!((observed[0].runs, observed[0].measured_runs), (1, 1));
    }

    /// A bill is the provider's word, and a partial one has to read as partial.
    #[tokio::test]
    async fn a_run_sums_the_costs_its_calls_reported_and_counts_how_many_did() {
        use aiwatcher_core::{EventEnvelope, Sdk, Source};

        let model = ReadModel::new(ReadModelConfig::default());
        let at = datetime!(2026-09-14 12:00:00 UTC);
        let calls = [
            serde_json::json!({ "call_id": "c1", "prompt_tokens": 1_240, "cost_usd": 0.000_2 }),
            // The second call said nothing about what it cost, so it is counted
            // as a call and not as a cost.
            serde_json::json!({ "call_id": "c2", "prompt_tokens": 300 }),
            serde_json::json!({ "call_id": "c3", "cost_usd": 0.000_3 }),
        ];
        for (at_position, data) in calls.into_iter().enumerate() {
            let position = at_position as u64 + 1;
            let envelope = EventEnvelope::new(
                EventType::LlmCompleted,
                "run-cost",
                at,
                Source::new("api", Sdk::Other("go".to_owned())),
            )
            .with_data(data);
            model
                .apply(&envelope.record(position, position, at, None))
                .await;
        }

        let summary = model.run("run-cost").await.expect("folded").summary;
        assert_eq!(summary.costed_calls, 2);
        let cost = summary.cost_usd.expect("two calls reported one");
        assert!((cost - 0.000_5).abs() < 1e-9, "got {cost}");
    }

    /// A run whose calls all stayed quiet about money has an unknown cost,
    /// which is not the same claim as a free one.
    #[tokio::test]
    async fn a_run_no_call_priced_reports_no_cost_rather_than_nought() {
        use aiwatcher_core::{EventEnvelope, Sdk, Source};

        let model = ReadModel::new(ReadModelConfig::default());
        let at = datetime!(2026-09-14 12:00:00 UTC);
        let envelope = EventEnvelope::new(
            EventType::LlmCompleted,
            "run-quiet",
            at,
            Source::new("api", Sdk::Other("go".to_owned())),
        )
        .with_data(serde_json::json!({ "call_id": "c1", "prompt_tokens": 10 }));
        model.apply(&envelope.record(1, 1, at, None)).await;

        let summary = model.run("run-quiet").await.expect("folded").summary;
        assert_eq!(summary.cost_usd, None);
        assert_eq!(summary.costed_calls, 0);
    }

    /// What a traces step reads off a run beyond its calls: who published its
    /// start, the shape it declared, the nodes it stepped through — and, from a
    /// serving host's own run, which run's call it served.
    #[tokio::test]
    async fn a_run_s_publisher_shape_and_steps_are_folded_and_a_serving_run_is_found_by_its_caller()
    {
        use aiwatcher_core::{EventEnvelope, Sdk, Source};

        let model = ReadModel::new(ReadModelConfig::default());
        let at = datetime!(2026-09-13 09:00:00 UTC);
        let events = [
            (
                "answer",
                EventType::RunStarted,
                "worker",
                serde_json::json!({}),
            ),
            (
                "answer",
                EventType::WorkflowDeclared,
                "worker",
                serde_json::json!({ "nodes": ["retrieve", "answer"], "edges": [["retrieve", "answer"]] }),
            ),
            (
                "answer",
                EventType::StepStarted,
                "worker",
                serde_json::json!({ "node": "retrieve" }),
            ),
            (
                "answer",
                EventType::StepStarted,
                "worker",
                serde_json::json!({ "node": "answer" }),
            ),
            (
                "answer",
                EventType::StepStarted,
                "worker",
                serde_json::json!({ "node": "answer" }),
            ),
            (
                "served",
                EventType::RunStarted,
                "serving",
                serde_json::json!({ "caller_run_id": "answer" }),
            ),
            (
                "elsewhere",
                EventType::RunStarted,
                "serving",
                serde_json::json!({}),
            ),
        ];
        for (position, (run_id, event_type, publisher, data)) in events.into_iter().enumerate() {
            let mut envelope =
                EventEnvelope::new(event_type, run_id, at, Source::new("bot", Sdk::Python))
                    .with_data(data);
            envelope.workflow_id = Some("capitals-app".to_owned());
            envelope.published_by = Some(publisher.to_owned());
            let position = position as u64 + 1;
            model
                .apply(&envelope.record(position, position, at, None))
                .await;
        }

        let answer = model.run("answer").await.expect("folded").summary;
        assert_eq!(answer.published_by.as_deref(), Some("worker"));
        assert_eq!(
            answer.workflow_topology,
            aiwatcher_core::topology::Topology::read(&serde_json::json!({
                "nodes": ["answer", "retrieve"],
                "edges": [{"from": "retrieve", "to": "answer"}]
            }))
            .map(|shape| shape.digest())
        );
        assert_eq!(answer.nodes_run, ["retrieve", "answer"]);
        assert_eq!(
            answer.node_steps,
            [
                NodeStep::Started("retrieve".to_owned()),
                NodeStep::Started("answer".to_owned()),
                NodeStep::Started("answer".to_owned()),
            ]
        );

        let serving = model
            .serving(&std::collections::BTreeSet::from(["answer"]))
            .await;
        let found: Vec<(&str, Option<&str>)> = serving["answer"]
            .iter()
            .map(|run| {
                (
                    run.summary.run_id.as_str(),
                    run.summary.published_by.as_deref(),
                )
            })
            .collect();
        assert_eq!(found, [("served", Some("serving"))]);
    }
}
