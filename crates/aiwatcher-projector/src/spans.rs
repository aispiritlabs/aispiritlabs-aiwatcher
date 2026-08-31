//! Spans as a flat, searchable list.
//!
//! The read model stores spans the way the waterfall needs them — keyed by run,
//! nested by parent. That answers "show me this run" and nothing else. It
//! cannot answer "every `execute_tool` span that took over two seconds", which
//! is the question someone asks when they are looking for the problem rather
//! than looking at a known one.
//!
//! So this is the same spans, indexed the other way: one row per span, run id
//! carried on the row (a [`CompletedSpan`] does not know its own run — the map
//! key does), and the attributes that matter for filtering lifted out of the
//! attribute list into fields. No second copy is stored; this is a pass over
//! what the read model already holds.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use aiwatcher_core::attrs::{aiwatcher as own, genai};
use aiwatcher_core::ports::{CompletedSpan, SpanKind, SpanStatus};
use aiwatcher_core::{SpanId, TraceId};

use crate::metrics::string_attr;

/// One span, flattened for a list rather than for a tree.
#[derive(Clone, Debug, PartialEq, Serialize, utoipa::ToSchema)]
pub struct SpanRow {
    /// From the map key: a `CompletedSpan` carries no run id of its own.
    pub run_id: String,
    pub trace_id: TraceId,
    pub span_id: SpanId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<SpanId>,
    pub name: String,
    pub kind: SpanKind,
    #[serde(with = "time::serde::rfc3339")]
    pub start: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub end: OffsetDateTime,
    pub duration_ms: i64,
    pub status: SpanStatus,
    /// The four attributes worth filtering on, lifted out of `attributes` so
    /// the panel does not have to walk the list to render a row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_type: Option<String>,
}

impl SpanRow {
    fn build(run_id: &str, span: &CompletedSpan) -> Self {
        Self {
            run_id: run_id.to_owned(),
            trace_id: span.trace_id,
            span_id: span.span_id,
            parent_span_id: span.parent_span_id,
            name: span.name.clone(),
            kind: span.kind,
            start: span.start,
            end: span.end,
            duration_ms: (span.end - span.start).whole_milliseconds().max(0) as i64,
            status: span.status.clone(),
            operation: string_attr(span, genai::OPERATION_NAME).map(ToOwned::to_owned),
            agent_id: string_attr(span, genai::AGENT_ID).map(ToOwned::to_owned),
            model: string_attr(span, genai::REQUEST_MODEL)
                .or_else(|| string_attr(span, genai::RESPONSE_MODEL))
                .map(ToOwned::to_owned),
            tool: string_attr(span, genai::TOOL_NAME).map(ToOwned::to_owned),
            step_type: string_attr(span, own::span::STEP_TYPE).map(ToOwned::to_owned),
        }
    }

    /// The cursor value for this row.
    ///
    /// Run id and span id together: a span id is derived from its trace and a
    /// key, so it is unique inside a trace but carries no promise across the
    /// whole list.
    fn cursor(&self) -> String {
        format!("{}:{}", self.run_id, self.span_id.to_hex())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SpanOutcome {
    Ok,
    Error,
}

#[derive(Clone, Debug, Default, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct SpanFilter {
    /// Only spans that ended in the last this-many seconds. See
    /// [`crate::window`].
    pub window_seconds: Option<i64>,
    pub run_id: Option<String>,
    pub trace_id: Option<String>,
    pub agent_id: Option<String>,
    pub model: Option<String>,
    pub tool: Option<String>,
    pub step_type: Option<String>,
    pub operation: Option<String>,
    pub status: Option<SpanOutcome>,
    /// The filter that turns this list into a hunt for a problem: everything
    /// slower than a threshold, whatever it is.
    pub min_duration_ms: Option<i64>,
    /// Substring over the span name and the lifted attributes.
    pub search: Option<String>,
    /// Cursor: `run_id:span_id`, from a previous page's `next_cursor`.
    pub after: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct SpanPage {
    pub spans: Vec<SpanRow>,
    /// Spans matching the filter, before the page limit.
    pub total_known: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

fn matches(row: &SpanRow, filter: &SpanFilter) -> bool {
    if filter
        .run_id
        .as_ref()
        .is_some_and(|wanted| &row.run_id != wanted)
    {
        return false;
    }
    if filter
        .trace_id
        .as_ref()
        .is_some_and(|wanted| &row.trace_id.to_hex() != wanted)
    {
        return false;
    }
    if filter
        .agent_id
        .as_ref()
        .is_some_and(|wanted| row.agent_id.as_ref() != Some(wanted))
    {
        return false;
    }
    if filter
        .model
        .as_ref()
        .is_some_and(|wanted| row.model.as_ref() != Some(wanted))
    {
        return false;
    }
    if filter
        .tool
        .as_ref()
        .is_some_and(|wanted| row.tool.as_ref() != Some(wanted))
    {
        return false;
    }
    if filter
        .step_type
        .as_ref()
        .is_some_and(|wanted| row.step_type.as_ref() != Some(wanted))
    {
        return false;
    }
    if filter
        .operation
        .as_ref()
        .is_some_and(|wanted| row.operation.as_ref() != Some(wanted))
    {
        return false;
    }
    if let Some(outcome) = filter.status {
        let errored = matches!(row.status, SpanStatus::Error { .. });
        if (outcome == SpanOutcome::Error) != errored {
            return false;
        }
    }
    if filter
        .min_duration_ms
        .is_some_and(|floor| row.duration_ms < floor)
    {
        return false;
    }
    if let Some(needle) = &filter.search {
        let needle = needle.to_lowercase();
        let haystack = [
            Some(&row.name),
            row.operation.as_ref(),
            row.agent_id.as_ref(),
            row.model.as_ref(),
            row.tool.as_ref(),
            row.step_type.as_ref(),
        ];
        if !haystack
            .into_iter()
            .flatten()
            .any(|field| field.to_lowercase().contains(&needle))
        {
            return false;
        }
    }
    true
}

/// Flatten and filter every retained span.
pub fn compute(
    spans: &HashMap<String, Vec<CompletedSpan>>,
    filter: &SpanFilter,
    now: OffsetDateTime,
) -> SpanPage {
    let since = crate::window::cutoff(filter.window_seconds, now);
    let mut rows: Vec<SpanRow> = spans
        .iter()
        // `run_id` is the map key, so narrowing by it skips whole runs before
        // building a single row.
        .filter(|(run_id, _)| {
            filter
                .run_id
                .as_ref()
                .is_none_or(|wanted| *run_id == wanted)
        })
        .flat_map(|(run_id, spans)| spans.iter().map(move |span| SpanRow::build(run_id, span)))
        // On the end, not the start: a span is only ever written when it
        // finishes, so "in the last fifteen minutes" is a question about when
        // it landed.
        .filter(|row| since.is_none_or(|start| row.end >= start))
        .filter(|row| matches(row, filter))
        .collect();

    // Newest first, cursor as the tie-break so the order is total — an
    // unstable order would make a page boundary skip or repeat a row.
    rows.sort_by(|a, b| {
        b.start
            .cmp(&a.start)
            .then_with(|| a.cursor().cmp(&b.cursor()))
    });

    if let Some(cursor) = &filter.after
        && let Some(index) = rows.iter().position(|row| &row.cursor() == cursor)
    {
        rows.drain(0..=index);
    }

    let total_known = rows.len();
    rows.truncate(filter.limit.unwrap_or(100).clamp(1, 500));
    let next_cursor = (total_known > rows.len())
        .then(|| rows.last().map(SpanRow::cursor))
        .flatten();

    SpanPage {
        spans: rows,
        total_known,
        next_cursor,
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use aiwatcher_core::ports::attr;

    use super::*;

    fn now() -> OffsetDateTime {
        datetime!(2026-08-27 18:30:00 UTC)
    }

    fn span(run_id: &str, name: &str, millis: i64, step_type: Option<&str>) -> CompletedSpan {
        let trace_id = TraceId::derive(run_id);
        let start = datetime!(2026-08-27 18:20:00 UTC);
        let mut attributes = vec![
            attr(genai::OPERATION_NAME, genai::operation::EXECUTE_TOOL),
            attr(genai::TOOL_NAME, name),
        ];
        if let Some(step) = step_type {
            attributes.push(attr(own::span::STEP_TYPE, step));
        }
        CompletedSpan {
            trace_id,
            span_id: SpanId::derive(trace_id, &format!("{name}:{millis}")),
            parent_span_id: None,
            name: name.to_owned(),
            kind: SpanKind::Internal,
            start,
            end: start + time::Duration::milliseconds(millis),
            status: SpanStatus::Ok,
            attributes,
            events: Vec::new(),
            links: Vec::new(),
        }
    }

    fn model() -> HashMap<String, Vec<CompletedSpan>> {
        HashMap::from([
            (
                "run-1".to_owned(),
                vec![
                    span("run-1", "search", 3_000, Some("tool")),
                    span("run-1", "embed", 40, Some("embedding")),
                ],
            ),
            (
                "run-2".to_owned(),
                vec![span("run-2", "search", 120, Some("tool"))],
            ),
        ])
    }

    #[test]
    fn every_span_carries_the_run_it_belongs_to() {
        let page = compute(&model(), &SpanFilter::default(), now());
        assert_eq!(page.total_known, 3);
        assert!(page.spans.iter().all(|row| !row.run_id.is_empty()));
    }

    /// A span is written when it ends, so the window asks about its end.
    #[test]
    fn the_window_keeps_the_spans_that_ended_inside_it() {
        let page = compute(
            &model(),
            &SpanFilter {
                window_seconds: Some(60),
                ..SpanFilter::default()
            },
            now(),
        );

        assert_eq!(
            page.total_known, 0,
            "every span in the fixture ended ten minutes ago"
        );

        let page = compute(
            &model(),
            &SpanFilter {
                window_seconds: Some(3600),
                ..SpanFilter::default()
            },
            now(),
        );
        assert_eq!(page.total_known, 3);
    }

    #[test]
    fn a_span_filter_by_step_type_ignores_spans_from_other_runs() {
        let page = compute(
            &model(),
            &SpanFilter {
                run_id: Some("run-1".to_owned()),
                step_type: Some("tool".to_owned()),
                ..SpanFilter::default()
            },
            now(),
        );

        assert_eq!(page.spans.len(), 1);
        assert_eq!(page.spans[0].run_id, "run-1");
        assert_eq!(page.spans[0].tool.as_deref(), Some("search"));
    }

    #[test]
    fn a_duration_floor_keeps_only_the_slow_spans() {
        let page = compute(
            &model(),
            &SpanFilter {
                min_duration_ms: Some(1_000),
                ..SpanFilter::default()
            },
            now(),
        );

        assert_eq!(page.spans.len(), 1);
        assert_eq!(page.spans[0].duration_ms, 3_000);
    }

    #[test]
    fn a_span_page_resumes_from_its_cursor_without_repeating_a_span() {
        let spans = model();
        let first = compute(
            &spans,
            &SpanFilter {
                limit: Some(2),
                ..SpanFilter::default()
            },
            now(),
        );
        assert_eq!(first.spans.len(), 2);
        let cursor = first.next_cursor.clone().expect("a third span remains");

        let second = compute(
            &spans,
            &SpanFilter {
                after: Some(cursor),
                limit: Some(2),
                ..SpanFilter::default()
            },
            now(),
        );

        assert_eq!(second.spans.len(), 1);
        assert!(!first.spans.iter().any(|seen| {
            seen.span_id == second.spans[0].span_id && seen.run_id == second.spans[0].run_id
        }));
    }

    #[test]
    fn the_search_is_case_insensitive_over_the_lifted_attributes() {
        let page = compute(
            &model(),
            &SpanFilter {
                search: Some("EMBED".to_owned()),
                ..SpanFilter::default()
            },
            now(),
        );

        assert_eq!(page.spans.len(), 1);
        assert_eq!(page.spans[0].name, "embed");
    }
}
