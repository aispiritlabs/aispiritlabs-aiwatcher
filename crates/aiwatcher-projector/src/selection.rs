//! What every list means by "these runs".
//!
//! The runs list, the dimension fold and the metrics fold are three reads of
//! one population, and until this they disagreed about how to name it: `/runs`
//! took nine axes, `/dimensions/{kind}` took one and `/metrics` took three, and
//! the one axis all three had meant something different on the third. A panel
//! cannot offer one filter over reads that do not agree what a filter is, so
//! the agreement is here, in one predicate the three call.
//!
//! **The line between selecting and narrowing.** An axis the *run* carries —
//! its session, its agents, its runtimes, its workflow, its variant, its trace,
//! its status — selects runs and narrows nothing else. An axis only its
//! *spans* carry — the model a call ran on, the tool it invoked, the registered
//! prompt it named — also selects runs, because a run that never touched that
//! model is not a run anybody asked about; and where a counter is about the
//! thing that axis names, the counter is narrowed to it as well. That second
//! half is `metrics::compute`'s, because it is the only one of the three that
//! counts something smaller than a run. `DimensionKind::needs_spans` is the
//! same line drawn for the same reason.
//!
//! Repeated, never flattened: these filters are `deny_unknown_fields` query
//! structs, and serde's `flatten` does not compose with that (CLAUDE.md). So
//! the *fields* are written out per filter and the *rule* is shared.

use aiwatcher_core::attrs::{aiwatcher as own, genai};
use aiwatcher_core::ports::{AttrValue, CompletedSpan};

use crate::readmodel::{RunStatus, RunSummary};

/// One read's narrowing, borrowed from whichever filter struct carries it.
#[derive(Clone, Copy, Debug, Default)]
pub struct RunSelection<'a> {
    pub conversation_id: Option<&'a str>,
    pub agent_id: Option<&'a str>,
    pub runtime: Option<&'a str>,
    pub workflow: Option<&'a str>,
    pub variant_id: Option<&'a str>,
    pub trace_id: Option<&'a str>,
    pub model: Option<&'a str>,
    pub tool: Option<&'a str>,
    /// The registered prompt a call named — `aiwatcher.prompt.name`, never its
    /// text (ADR_0011).
    pub prompt: Option<&'a str>,
    pub status: Option<RunStatus>,
}

impl RunSelection<'_> {
    /// Whether one run has every named property.
    ///
    /// Conjunctive between axes. `spans` is the run's, and only the three
    /// span-level axes look at it — a caller that has no span map and names
    /// none of them gets the same answer.
    #[must_use]
    pub fn matches(&self, run: &RunSummary, spans: Option<&Vec<CompletedSpan>>) -> bool {
        if self
            .conversation_id
            .is_some_and(|wanted| run.conversation_id.as_deref() != Some(wanted))
        {
            return false;
        }
        if self
            .agent_id
            .is_some_and(|wanted| !run.agents.iter().any(|agent| agent == wanted))
        {
            return false;
        }
        if self
            .runtime
            .is_some_and(|wanted| !run.runtimes.iter().any(|runtime| runtime == wanted))
        {
            return false;
        }
        if self
            .workflow
            .is_some_and(|wanted| run.workflow.as_deref() != Some(wanted))
        {
            return false;
        }
        if self
            .variant_id
            .is_some_and(|wanted| run.variant_id.as_deref() != Some(wanted))
        {
            return false;
        }
        if self
            .trace_id
            .is_some_and(|wanted| run.trace_id.to_hex() != wanted)
        {
            return false;
        }
        if self.status.is_some_and(|wanted| run.status != wanted) {
            return false;
        }
        for (key, wanted) in [
            (genai::REQUEST_MODEL, self.model),
            (genai::TOOL_NAME, self.tool),
            (own::prompt::NAME, self.prompt),
        ] {
            if wanted.is_some_and(|wanted| !span_attribute_matches(spans, key, wanted)) {
                return false;
            }
        }
        true
    }

    /// Whether answering this needs the run's spans at all.
    ///
    /// The caller can skip cloning or looking up the span map when nothing
    /// span-level was asked for, which is every ordinary read.
    #[must_use]
    pub const fn needs_spans(&self) -> bool {
        self.model.is_some() || self.tool.is_some() || self.prompt.is_some()
    }
}

/// Whether any span of a run carries this string attribute with this value.
pub fn span_attribute_matches(spans: Option<&Vec<CompletedSpan>>, key: &str, wanted: &str) -> bool {
    spans.is_some_and(|spans| {
        spans.iter().any(|span| {
            span.attributes.iter().any(|(name, value)| {
                name == key && matches!(value, AttrValue::Str(inner) if inner == wanted)
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use aiwatcher_core::ports::{SpanKind, SpanStatus, attr};
    use aiwatcher_core::{Checkpoint, SpanId, TraceId};

    use super::*;

    fn run() -> RunSummary {
        RunSummary {
            run_id: "run-1".to_owned(),
            conversation_id: Some("session-1".to_owned()),
            trace_id: TraceId::derive("run-1"),
            status: RunStatus::Succeeded,
            agents: vec!["researcher".to_owned(), "writer".to_owned()],
            runtimes: vec!["planner".to_owned()],
            workflow: Some("house-import".to_owned()),
            variant_id: None,
            evaluation_id: None,
            published_by: None,
            caller_run_id: None,
            workflow_topology: None,
            nodes_run: Vec::new(),
            node_steps: Vec::new(),
            node_steps_dropped: false,
            started_at: datetime!(2026-09-18 10:00:00 UTC),
            last_event_at: datetime!(2026-09-18 10:01:00 UTC),
            ended_at: None,
            duration_ms: Some(100),
            event_count: 4,
            llm_calls: 1,
            tool_calls: 1,
            input_tokens: 10,
            output_tokens: 5,
            cached_tokens: 0,
            cost_usd: None,
            costed_calls: 0,
            error: None,
            last_checkpoint: Checkpoint::beginning(),
        }
    }

    fn call(prompt: Option<&str>) -> Vec<CompletedSpan> {
        let trace_id = TraceId::derive("run-1");
        let start = datetime!(2026-09-18 10:00:30 UTC);
        let mut attributes = vec![
            attr(genai::OPERATION_NAME, genai::operation::CHAT),
            attr(genai::REQUEST_MODEL, "opus"),
        ];
        if let Some(name) = prompt {
            attributes.push(attr(own::prompt::NAME, name));
        }
        vec![CompletedSpan {
            trace_id,
            span_id: SpanId::derive(trace_id, "llm:opus"),
            parent_span_id: None,
            name: "chat opus".to_owned(),
            kind: SpanKind::Client,
            start,
            end: start + time::Duration::milliseconds(500),
            status: SpanStatus::Ok,
            attributes,
            events: Vec::new(),
            links: Vec::new(),
        }]
    }

    #[test]
    fn an_empty_selection_holds_every_run() {
        assert!(RunSelection::default().matches(&run(), None));
    }

    #[test]
    fn a_run_is_selected_by_any_of_the_agents_it_ran() {
        let held = run();
        assert!(
            RunSelection {
                agent_id: Some("writer"),
                ..RunSelection::default()
            }
            .matches(&held, None)
        );
        assert!(
            !RunSelection {
                agent_id: Some("reviewer"),
                ..RunSelection::default()
            }
            .matches(&held, None)
        );
    }

    #[test]
    fn axes_are_conjunctive_so_a_run_needs_every_one_of_them() {
        assert!(
            !RunSelection {
                agent_id: Some("writer"),
                workflow: Some("other"),
                ..RunSelection::default()
            }
            .matches(&run(), None)
        );
    }

    #[test]
    fn a_span_level_axis_reads_the_runs_spans_rather_than_its_summary() {
        let held = run();
        let spans = call(Some("planner.assistant"));
        assert!(
            RunSelection {
                model: Some("opus"),
                prompt: Some("planner.assistant"),
                ..RunSelection::default()
            }
            .matches(&held, Some(&spans))
        );
        assert!(
            !RunSelection {
                prompt: Some("other.prompt"),
                ..RunSelection::default()
            }
            .matches(&held, Some(&spans))
        );
    }

    #[test]
    fn a_span_level_axis_with_no_spans_selects_nothing_rather_than_everything() {
        // A run whose spans were evicted cannot be shown to have called a
        // model, and reporting it as a match would put it under a row it may
        // not belong to.
        assert!(
            !RunSelection {
                model: Some("opus"),
                ..RunSelection::default()
            }
            .matches(&run(), None)
        );
    }

    #[test]
    fn only_a_span_level_axis_asks_for_the_spans() {
        assert!(
            !RunSelection {
                agent_id: Some("writer"),
                ..RunSelection::default()
            }
            .needs_spans()
        );
        assert!(
            RunSelection {
                prompt: Some("planner.assistant"),
                ..RunSelection::default()
            }
            .needs_spans()
        );
    }
}
