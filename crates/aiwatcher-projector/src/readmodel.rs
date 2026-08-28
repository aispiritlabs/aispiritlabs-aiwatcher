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
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
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
            started_at: event.metadata.occurred_at,
            ended_at: None,
            duration_ms: None,
            event_count: 0,
            llm_calls: 0,
            tool_calls: 0,
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
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

        let subject = event.event_type.subject();
        let phase = event.event_type.phase();

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
        }

        if subject == Subject::Run {
            match phase {
                Some(Phase::End { ok: true }) => {
                    self.status = RunStatus::Succeeded;
                    self.finish(event.metadata.occurred_at);
                }
                Some(Phase::End { ok: false }) => {
                    self.status = RunStatus::Failed;
                    self.error = event
                        .data_str("error")
                        .or_else(|| event.data_str("message"))
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
    pub conversation_id: Option<String>,
    pub agent_id: Option<String>,
    /// Runs produced by this service. See `RunSummary::runtimes`.
    pub runtime: Option<String>,
    pub workflow: Option<String>,
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
    pub status: Option<RunStatus>,
    /// Cursor: return runs older than this one. Keyset pagination, because an
    /// offset shifts under a list that is actively growing.
    pub before: Option<String>,
    pub limit: Option<usize>,
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
}

impl Default for ReadModelConfig {
    fn default() -> Self {
        Self {
            max_runs: 5_000,
            max_spans_per_run: 500,
            max_spans_total: 60_000,
            evaluations: EvaluationConfig::default(),
        }
    }
}

/// Whether any of a run's spans carries `key = wanted`.
///
/// A run with no retained spans does not match: the alternative — treating
/// "unknown" as "matches" — would put runs into a model or tool bucket they may
/// have nothing to do with.
fn span_attribute_matches(spans: Option<&Vec<CompletedSpan>>, key: &str, wanted: &str) -> bool {
    spans.is_some_and(|spans| {
        spans.iter().any(|span| {
            span.attributes.iter().any(|(name, value)| {
                name == key
                    && matches!(
                        value,
                        aiwatcher_core::ports::AttrValue::Str(inner) if inner == wanted
                    )
            })
        })
    })
}

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
            // agents did, so it gets its own projection.
            state.evaluations.apply(event, &self.config.evaluations);
            return;
        }
        let run_id = event.metadata.run_id.clone();
        if !state.runs.contains_key(&run_id) {
            state.order.push(run_id.clone());
            state.runs.insert(run_id.clone(), RunSummary::new(event));
        }
        if let Some(summary) = state.runs.get_mut(&run_id) {
            summary.apply(event);
        }
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

    pub async fn list(&self, filter: &RunFilter) -> RunPage {
        let state = self.state.read().await;
        let limit = filter.limit.unwrap_or(50).clamp(1, 500);

        // Newest first.
        let mut matching: Vec<&RunSummary> = state
            .order
            .iter()
            .rev()
            .filter_map(|run_id| state.runs.get(run_id))
            .filter(|run| {
                filter
                    .conversation_id
                    .as_ref()
                    .is_none_or(|wanted| run.conversation_id.as_ref() == Some(wanted))
            })
            .filter(|run| {
                filter
                    .agent_id
                    .as_ref()
                    .is_none_or(|wanted| run.agents.iter().any(|agent| agent == wanted))
            })
            .filter(|run| {
                filter
                    .runtime
                    .as_ref()
                    .is_none_or(|wanted| run.runtimes.iter().any(|runtime| runtime == wanted))
            })
            .filter(|run| {
                filter
                    .workflow
                    .as_ref()
                    .is_none_or(|wanted| run.workflow.as_ref() == Some(wanted))
            })
            .filter(|run| {
                filter
                    .trace_id
                    .as_ref()
                    .is_none_or(|wanted| &run.trace_id.to_hex() == wanted)
            })
            .filter(|run| filter.status.is_none_or(|wanted| run.status == wanted))
            .filter(|run| {
                filter.model.as_ref().is_none_or(|wanted| {
                    span_attribute_matches(
                        state.spans.get(&run.run_id),
                        aiwatcher_core::attrs::genai::REQUEST_MODEL,
                        wanted,
                    )
                })
            })
            .filter(|run| {
                filter.tool.as_ref().is_none_or(|wanted| {
                    span_attribute_matches(
                        state.spans.get(&run.run_id),
                        aiwatcher_core::attrs::genai::TOOL_NAME,
                        wanted,
                    )
                })
            })
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
        crate::conversations::compute(&runs, &state.spans, filter)
    }

    /// One dimension's rows: the explorer's top level, whatever it is rooted on.
    pub async fn dimensions(
        &self,
        kind: crate::dimensions::DimensionKind,
        filter: &crate::dimensions::DimensionFilter,
    ) -> crate::dimensions::DimensionPage {
        let state = self.state.read().await;
        let runs: Vec<RunSummary> = state.runs.values().cloned().collect();
        crate::dimensions::compute(&runs, &state.spans, kind, filter)
    }

    /// Every retained span, flat and filterable. See [`crate::spans`].
    pub async fn spans(&self, filter: &crate::spans::SpanFilter) -> crate::spans::SpanPage {
        let state = self.state.read().await;
        crate::spans::compute(&state.spans, filter)
    }

    /// Evaluation reports, newest first. See [`crate::evaluations`].
    pub async fn evaluations(&self, filter: &EvaluationFilter) -> EvaluationPage {
        self.state.read().await.evaluations.page(filter)
    }

    /// One evaluation, with its cases, its report and its baseline.
    pub async fn evaluation(&self, evaluation_id: &str) -> Option<EvaluationDetail> {
        self.state.read().await.evaluations.detail(evaluation_id)
    }

    /// Suites: the level above an evaluation report.
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
            started_at: datetime!(2026-08-27 18:20:00 UTC),
            ended_at: None,
            duration_ms: Some(1),
            event_count: 1,
            llm_calls: 0,
            tool_calls: 0,
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
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
}
