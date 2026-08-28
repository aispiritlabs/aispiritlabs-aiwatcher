//! Every way of slicing the retained runs, folded by one function.
//!
//! The explorer's tree is the same shape whatever its root is — a dimension,
//! then the runs under it, then spans, then events. What differs between
//! "group by session" and "group by workflow" is one line: which key a run
//! contributes. So that is the only thing parameterised here.
//!
//! ```text
//! session   run.conversation_id      0..1 per run
//! agent     run.agents               0..n
//! runtime   run.runtimes             0..n   (the producing service)
//! workflow  run.workflow             0..1
//! trace     run.trace_id             exactly 1
//! model     gen_ai.request.model     0..n   (from the run's spans)
//! tool      gen_ai.tool.name         0..n   (from the run's spans)
//! ```
//!
//! Folded from the read model, like [`crate::conversations`] and
//! [`crate::metrics`]: no extra store, and the same retention window. A run
//! that has no key for the chosen dimension is counted in `ungrouped_runs`
//! rather than dropped — a tree that silently holds fewer runs than the runs
//! list is a tree nobody trusts.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use aiwatcher_core::attrs::genai;
use aiwatcher_core::ports::CompletedSpan;

use crate::metrics::string_attr;
use crate::readmodel::{RunStatus, RunSummary};

/// What the tree is rooted on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DimensionKind {
    Session,
    Agent,
    Runtime,
    Workflow,
    Trace,
    Model,
    Tool,
}

impl DimensionKind {
    /// Whether answering this needs the run's spans.
    ///
    /// A run does not carry the models it used or the tools it called; the
    /// spans do. The caller can skip cloning the span map for the five
    /// dimensions that never look at it.
    #[must_use]
    pub const fn needs_spans(self) -> bool {
        matches!(self, Self::Model | Self::Tool)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Agent => "agent",
            Self::Runtime => "runtime",
            Self::Workflow => "workflow",
            Self::Trace => "trace",
            Self::Model => "model",
            Self::Tool => "tool",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct DimensionFilter {
    /// Narrow to runs that ran this agent, whatever the dimension is. Lets the
    /// tree stay scoped when someone arrives from an agent-filtered view.
    pub agent_id: Option<String>,
    /// Substring match on the key. The one control that turns a long list into
    /// the row someone is looking for.
    pub search: Option<String>,
    /// Cursor: return rows after this key in the current sort order. Keyed
    /// rather than offset, because the list reorders as runs arrive.
    pub after: Option<String>,
    pub limit: Option<usize>,
}

/// One row of the tree's top level, whatever the dimension is.
#[derive(Clone, Debug, PartialEq, Serialize, utoipa::ToSchema)]
pub struct DimensionSummary {
    pub key: String,
    pub runs: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub running: u64,
    /// Every agent seen across the row's runs, in first-seen order.
    pub agents: Vec<String>,
    pub llm_calls: u64,
    pub tool_calls: u64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_tokens: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    /// The newest run's start. What the list sorts by, so an active row stays
    /// at the top.
    #[serde(with = "time::serde::rfc3339")]
    pub last_activity_at: OffsetDateTime,
}

impl DimensionSummary {
    fn new(key: String, run: &RunSummary) -> Self {
        Self {
            key,
            runs: 0,
            succeeded: 0,
            failed: 0,
            running: 0,
            agents: Vec::new(),
            llm_calls: 0,
            tool_calls: 0,
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
            started_at: run.started_at,
            last_activity_at: run.started_at,
        }
    }

    fn absorb(&mut self, run: &RunSummary) {
        self.runs += 1;
        match run.status {
            RunStatus::Succeeded => self.succeeded += 1,
            RunStatus::Failed => self.failed += 1,
            RunStatus::Running => self.running += 1,
        }
        self.llm_calls += run.llm_calls;
        self.tool_calls += run.tool_calls;
        self.input_tokens += run.input_tokens;
        self.output_tokens += run.output_tokens;
        self.cached_tokens += run.cached_tokens;
        self.started_at = self.started_at.min(run.started_at);
        self.last_activity_at = self.last_activity_at.max(run.started_at);
        for agent in &run.agents {
            if !self.agents.iter().any(|known| known == agent) {
                self.agents.push(agent.clone());
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct DimensionPage {
    pub kind: DimensionKind,
    pub rows: Vec<DimensionSummary>,
    /// Rows matching the filter, before the page limit.
    pub total: usize,
    /// Runs that carry no key for this dimension. They are reachable from the
    /// runs list but have no row to sit under, and silently dropping them would
    /// make the two views disagree about how much exists.
    pub ungrouped_runs: u64,
    /// Pass as `after` to fetch the next page. Absent on the last page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// The keys one run contributes to a dimension.
///
/// Empty means the run is ungrouped *for this dimension*, which is a fact
/// worth reporting rather than a hole to hide.
fn keys_of(
    run: &RunSummary,
    kind: DimensionKind,
    spans: Option<&Vec<CompletedSpan>>,
) -> Vec<String> {
    match kind {
        DimensionKind::Session => run.conversation_id.clone().into_iter().collect(),
        DimensionKind::Agent => run.agents.clone(),
        DimensionKind::Runtime => run.runtimes.clone(),
        DimensionKind::Workflow => run.workflow.clone().into_iter().collect(),
        // Always exactly one: a trace id is derived from the run id when the
        // producer supplies none, so no run is ever untraced.
        DimensionKind::Trace => vec![run.trace_id.to_hex()],
        DimensionKind::Model => span_keys(spans, genai::REQUEST_MODEL),
        DimensionKind::Tool => span_keys(spans, genai::TOOL_NAME),
    }
}

/// Distinct values of one span attribute across a run, in first-seen order.
fn span_keys(spans: Option<&Vec<CompletedSpan>>, attribute: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for span in spans.into_iter().flatten() {
        if let Some(value) = string_attr(span, attribute)
            && !seen.iter().any(|known| known == value)
        {
            seen.push(value.to_owned());
        }
    }
    seen
}

/// Fold the retained runs into one dimension's rows.
pub fn compute(
    runs: &[RunSummary],
    spans: &HashMap<String, Vec<CompletedSpan>>,
    kind: DimensionKind,
    filter: &DimensionFilter,
) -> DimensionPage {
    let mut grouped: HashMap<String, DimensionSummary> = HashMap::new();
    let mut ungrouped_runs = 0u64;

    for run in runs {
        if filter
            .agent_id
            .as_ref()
            .is_some_and(|wanted| !run.agents.iter().any(|agent| agent == wanted))
        {
            continue;
        }

        let keys = keys_of(run, kind, spans.get(&run.run_id));
        if keys.is_empty() {
            ungrouped_runs += 1;
            continue;
        }

        for key in keys {
            // The search narrows keys, not runs: a run with two agents where
            // only one matches contributes to the matching row alone.
            if filter
                .search
                .as_ref()
                .is_some_and(|needle| !contains_ignoring_case(&key, needle))
            {
                continue;
            }
            grouped
                .entry(key.clone())
                .or_insert_with(|| DimensionSummary::new(key, run))
                .absorb(run);
        }
    }

    let total = grouped.len();
    let mut rows: Vec<DimensionSummary> = grouped.into_values().collect();
    // Most recent activity first, key as the tie-break so the order is total —
    // the cursor below is a position in this sequence and an unstable sort
    // would make a page boundary skip or repeat a row.
    rows.sort_by(|a, b| {
        b.last_activity_at
            .cmp(&a.last_activity_at)
            .then_with(|| a.key.cmp(&b.key))
    });

    if let Some(cursor) = &filter.after
        && let Some(index) = rows.iter().position(|row| &row.key == cursor)
    {
        rows.drain(0..=index);
    }

    let remaining = rows.len();
    rows.truncate(filter.limit.unwrap_or(100).clamp(1, 500));
    let next_cursor = (remaining > rows.len())
        .then(|| rows.last().map(|row| row.key.clone()))
        .flatten();

    DimensionPage {
        kind,
        rows,
        total,
        ungrouped_runs,
        next_cursor,
    }
}

fn contains_ignoring_case(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use aiwatcher_core::ports::{SpanKind, SpanStatus, attr};
    use aiwatcher_core::{Checkpoint, SpanId, TraceId};

    use super::*;

    fn run(run_id: &str, status: RunStatus, started: OffsetDateTime) -> RunSummary {
        RunSummary {
            run_id: run_id.to_owned(),
            conversation_id: None,
            trace_id: TraceId::derive(run_id),
            status,
            agents: vec!["researcher".to_owned()],
            runtimes: vec!["agent-service".to_owned()],
            workflow: None,
            started_at: started,
            ended_at: None,
            duration_ms: Some(100),
            event_count: 4,
            llm_calls: 1,
            tool_calls: 1,
            input_tokens: 10,
            output_tokens: 5,
            cached_tokens: 0,
            error: None,
            last_checkpoint: Checkpoint::beginning(),
        }
    }

    fn llm_span(run_id: &str, model: &str) -> CompletedSpan {
        let trace_id = TraceId::derive(run_id);
        let start = datetime!(2026-08-27 18:20:00 UTC);
        CompletedSpan {
            trace_id,
            span_id: SpanId::derive(trace_id, &format!("llm:{model}")),
            parent_span_id: None,
            name: format!("chat {model}"),
            kind: SpanKind::Client,
            start,
            end: start + time::Duration::milliseconds(500),
            status: SpanStatus::Ok,
            attributes: vec![
                attr(genai::OPERATION_NAME, genai::operation::CHAT),
                attr(genai::REQUEST_MODEL, model),
            ],
            events: Vec::new(),
            links: Vec::new(),
        }
    }

    fn now() -> OffsetDateTime {
        datetime!(2026-08-27 18:20:00 UTC)
    }

    #[test]
    fn the_runtime_dimension_groups_runs_by_source_service() {
        let mut first = run("run-1", RunStatus::Succeeded, now());
        first.runtimes = vec!["planner".to_owned()];
        let mut second = run("run-2", RunStatus::Failed, now());
        second.runtimes = vec!["planner".to_owned(), "executor".to_owned()];

        let page = compute(
            &[first, second],
            &HashMap::new(),
            DimensionKind::Runtime,
            &DimensionFilter::default(),
        );

        assert_eq!(page.total, 2);
        let planner = page.rows.iter().find(|row| row.key == "planner").unwrap();
        assert_eq!(planner.runs, 2);
        assert_eq!(planner.failed, 1);
        // A run that hands off between two services counts in both rows.
        let executor = page.rows.iter().find(|row| row.key == "executor").unwrap();
        assert_eq!(executor.runs, 1);
        assert_eq!(page.ungrouped_runs, 0);
    }

    #[test]
    fn a_run_with_no_workflow_is_counted_as_ungrouped_rather_than_dropped() {
        let mut named = run("run-1", RunStatus::Succeeded, now());
        named.workflow = Some("nightly-summary".to_owned());
        let anonymous = run("run-2", RunStatus::Succeeded, now());

        let page = compute(
            &[named, anonymous],
            &HashMap::new(),
            DimensionKind::Workflow,
            &DimensionFilter::default(),
        );

        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].key, "nightly-summary");
        assert_eq!(page.ungrouped_runs, 1);
    }

    #[test]
    fn a_trace_shared_by_two_runs_is_one_row_in_the_trace_dimension() {
        let shared = TraceId::derive("shared");
        let mut first = run("run-1", RunStatus::Succeeded, now());
        first.trace_id = shared;
        let mut second = run("run-2", RunStatus::Running, now());
        second.trace_id = shared;

        let page = compute(
            &[first, second],
            &HashMap::new(),
            DimensionKind::Trace,
            &DimensionFilter::default(),
        );

        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].key, shared.to_hex());
        assert_eq!(page.rows[0].runs, 2);
        assert_eq!(page.rows[0].running, 1);
    }

    #[test]
    fn the_model_dimension_reads_the_runs_spans_rather_than_its_summary() {
        let spans = HashMap::from([(
            "run-1".to_owned(),
            vec![
                llm_span("run-1", "claude-opus-5"),
                llm_span("run-1", "claude-opus-5"),
            ],
        )]);

        let page = compute(
            &[run("run-1", RunStatus::Succeeded, now())],
            &spans,
            DimensionKind::Model,
            &DimensionFilter::default(),
        );

        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].key, "claude-opus-5");
        // Two spans, one run: the row counts runs, not calls.
        assert_eq!(page.rows[0].runs, 1);
    }

    #[test]
    fn a_cursor_resumes_after_its_row_without_repeating_it() {
        let runs: Vec<RunSummary> = (0..5)
            .map(|index| {
                let mut summary = run(&format!("run-{index}"), RunStatus::Succeeded, now());
                summary.workflow = Some(format!("workflow-{index}"));
                summary
            })
            .collect();

        let first = compute(
            &runs,
            &HashMap::new(),
            DimensionKind::Workflow,
            &DimensionFilter {
                limit: Some(2),
                ..DimensionFilter::default()
            },
        );
        assert_eq!(first.rows.len(), 2);
        let cursor = first.next_cursor.clone().expect("more rows remain");

        let second = compute(
            &runs,
            &HashMap::new(),
            DimensionKind::Workflow,
            &DimensionFilter {
                after: Some(cursor),
                limit: Some(2),
                ..DimensionFilter::default()
            },
        );

        assert_eq!(second.rows.len(), 2);
        for row in &second.rows {
            assert!(!first.rows.iter().any(|seen| seen.key == row.key));
        }
    }

    #[test]
    fn the_search_narrows_keys_rather_than_runs() {
        let mut both = run("run-1", RunStatus::Succeeded, now());
        both.agents = vec!["researcher".to_owned(), "writer".to_owned()];

        let page = compute(
            &[both],
            &HashMap::new(),
            DimensionKind::Agent,
            &DimensionFilter {
                search: Some("WRIT".to_owned()),
                ..DimensionFilter::default()
            },
        );

        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].key, "writer");
    }
}
