//! The numbers the panel's metrics view is built from.
//!
//! Computed from the read model rather than queried out of VictoriaMetrics.
//! The projector already holds every run it retains, with its token counts,
//! statuses and finished spans, so the aggregates are a fold over data that is
//! already in memory — no PromQL, no second query path, and no dependency on a
//! metrics backend being reachable for the page to render.
//!
//! The trade-off is the window: this covers the runs the read model still
//! holds (`ReadModelConfig::max_runs`, evicting finished runs first). The OTLP
//! metrics in VictoriaMetrics remain the long-horizon record; this is the view
//! for "what is my agent doing lately", which is the question people actually
//! open a dashboard to answer.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use aiwatcher_core::attrs::{aiwatcher as own, genai};
use aiwatcher_core::ports::{AttrValue, CompletedSpan, SpanStatus};

use crate::readmodel::{RunStatus, RunSummary};

/// What to include. Every field narrows; `None` means "everything".
#[derive(Clone, Debug, Default, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct MetricsFilter {
    /// Only runs that started within this many seconds of now.
    pub window_seconds: Option<i64>,
    pub agent_id: Option<String>,
    /// Narrows LLM calls, tokens and LLM latency only; run, tool and step
    /// counters still cover all runs selected by the other filters.
    pub model: Option<String>,
    pub conversation_id: Option<String>,
    /// Buckets in the timeline. Clamped to 6..=200.
    pub buckets: Option<usize>,
}

#[derive(Clone, Copy, Debug, Serialize, utoipa::ToSchema)]
pub struct Totals {
    pub runs: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub running: u64,
    pub llm_calls: u64,
    pub tool_calls: u64,
    /// Retrievals, embeddings, reranks, guardrails — everything traced as a
    /// step rather than as an LLM or tool call.
    pub step_calls: u64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_tokens: i64,
    /// Cached as a share of input. The one number that says whether prompt
    /// caching is doing anything.
    pub cache_hit_ratio: f64,
    /// What the providers said the model calls cost, in US dollars — the sum
    /// of every call's `aiwatcher.usage.cost_usd`. Absent where no call
    /// reported one: an unknown cost is not a free one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// Of `llm_calls`, how many reported that cost. Below it, the figure above
    /// is part of the bill rather than the bill.
    pub costed_calls: u64,
}

/// Nearest-rank percentiles, in milliseconds.
#[derive(Clone, Copy, Debug, Default, Serialize, utoipa::ToSchema)]
pub struct Percentiles {
    pub count: u64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct Latency {
    /// End-to-end, per run.
    pub run: Percentiles,
    /// Per LLM call.
    pub llm: Percentiles,
    /// Per tool call.
    pub tool: Percentiles,
    /// Per step — retrieval, embedding, rerank and the rest.
    pub step: Percentiles,
    /// Time to first token, from the `gen_ai.first_token` span event.
    pub time_to_first_token: Percentiles,
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct AgentBreakdown {
    pub agent_id: String,
    pub runs: u64,
    pub llm_calls: u64,
    pub tool_calls: u64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_tokens: i64,
    /// What this agent's model calls cost, where their providers said so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub failures: u64,
    pub llm_latency: Percentiles,
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct ModelBreakdown {
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    pub calls: u64,
    pub failures: u64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_tokens: i64,
    /// What this model's calls cost, where their provider said so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub latency: Percentiles,
}

/// Retrieval, embedding, rerank, guardrail — grouped by kind, then by name.
///
/// Retrieval latency is the number a slow RAG turn gets debugged against, and
/// it is invisible in the LLM and tool breakdowns.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct StepBreakdown {
    pub step_type: String,
    pub name: String,
    pub calls: u64,
    pub failures: u64,
    pub latency: Percentiles,
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct ToolBreakdown {
    pub tool_name: String,
    pub calls: u64,
    pub failures: u64,
    pub latency: Percentiles,
}

/// Current run statuses and observed calls, grouped by run start time.
/// Token and call counts use the same retained completed spans as totals.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct Bucket {
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub runs: u64,
    pub succeeded: u64,
    pub running: u64,
    pub failed: u64,
    pub llm_calls: u64,
    pub tool_calls: u64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_tokens: i64,
    /// Reported cost of the same retained, filtered calls as the totals,
    /// attributed to the run's start like its tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct MetricsWindow {
    #[serde(with = "time::serde::rfc3339")]
    pub from: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub to: OffsetDateTime,
    /// Runs matched after filtering.
    pub runs_considered: u64,
    /// Runs the read model holds in total. A `runs_considered` well below this
    /// means the filter is narrow; the two being equal *and* at the retention
    /// cap means older runs have been evicted and the window is truncated.
    pub runs_retained: u64,
    pub retention_limit: u64,
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct MetricsSummary {
    pub window: MetricsWindow,
    pub totals: Totals,
    pub latency: Latency,
    pub by_agent: Vec<AgentBreakdown>,
    pub by_model: Vec<ModelBreakdown>,
    pub by_tool: Vec<ToolBreakdown>,
    pub by_step: Vec<StepBreakdown>,
    pub timeline: Vec<Bucket>,
}

/// Nearest-rank percentile. `values` need not be sorted; this sorts a copy.
///
/// Nearest-rank rather than interpolated: with the handful of samples a short
/// window holds, interpolation invents a value between two real observations
/// and reads as more precision than there is.
fn percentiles(values: &mut [f64]) -> Percentiles {
    if values.is_empty() {
        return Percentiles::default();
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let at = |quantile: f64| -> f64 {
        let rank = (quantile * values.len() as f64).ceil().max(1.0) as usize;
        values[rank.min(values.len()) - 1]
    };
    Percentiles {
        count: values.len() as u64,
        p50: at(0.50),
        p95: at(0.95),
        p99: at(0.99),
    }
}

pub(crate) fn string_attr<'a>(span: &'a CompletedSpan, key: &str) -> Option<&'a str> {
    span.attributes
        .iter()
        .find_map(|(name, value)| match value {
            AttrValue::Str(inner) if name == key => Some(inner.as_str()),
            _ => None,
        })
}

fn double_attr(span: &CompletedSpan, key: &str) -> Option<f64> {
    span.attributes
        .iter()
        .find_map(|(name, value)| match value {
            AttrValue::Double(inner) if name == key && inner.is_finite() => Some(*inner),
            _ => None,
        })
}

/// Adds a reported cost to a running total that stays absent until something
/// reports one — so a sum of silences is still a silence, not nought.
fn add_cost(total: &mut Option<f64>, cost: Option<f64>) {
    if let Some(cost) = cost {
        *total = Some(total.unwrap_or(0.0) + cost);
    }
}

fn int_attr(span: &CompletedSpan, key: &str) -> i64 {
    span.attributes
        .iter()
        .find_map(|(name, value)| match value {
            AttrValue::Int(inner) if name == key => Some(*inner),
            _ => None,
        })
        .unwrap_or(0)
}

fn duration_ms(span: &CompletedSpan) -> f64 {
    (span.end - span.start).as_seconds_f64() * 1000.0
}

fn failed(span: &CompletedSpan) -> bool {
    matches!(span.status, SpanStatus::Error { .. })
}

/// What kind of operation a span records, from the GenAI attribute the
/// assembler stamps. Reading the attribute rather than the span name keeps this
/// working if a name format changes.
fn operation(span: &CompletedSpan) -> Option<&str> {
    string_attr(span, genai::OPERATION_NAME)
}

#[derive(Default)]
struct Accumulator {
    runs: u64,
    llm_calls: u64,
    tool_calls: u64,
    input_tokens: i64,
    output_tokens: i64,
    cached_tokens: i64,
    cost_usd: Option<f64>,
    failures: u64,
    llm_latencies: Vec<f64>,
}

/// Fold the retained runs into the summary.
pub fn compute(
    runs: &[RunSummary],
    spans: &HashMap<String, Vec<CompletedSpan>>,
    filter: &MetricsFilter,
    retention_limit: usize,
    now: OffsetDateTime,
) -> MetricsSummary {
    let retained = runs.len() as u64;
    let from = filter
        .window_seconds
        .filter(|seconds| *seconds > 0)
        .map(|seconds| now - time::Duration::seconds(seconds));

    let matching: Vec<&RunSummary> = runs
        .iter()
        .filter(|run| from.is_none_or(|start| run.started_at >= start))
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
        .collect();

    let mut totals = Totals {
        runs: matching.len() as u64,
        succeeded: 0,
        failed: 0,
        running: 0,
        llm_calls: 0,
        tool_calls: 0,
        step_calls: 0,
        input_tokens: 0,
        output_tokens: 0,
        cached_tokens: 0,
        cache_hit_ratio: 0.0,
        cost_usd: None,
        costed_calls: 0,
    };

    let mut run_latencies = Vec::new();
    let mut llm_latencies = Vec::new();
    let mut tool_latencies = Vec::new();
    let mut ttft = Vec::new();
    let mut by_agent: HashMap<String, Accumulator> = HashMap::new();
    let mut by_model: HashMap<String, (Option<String>, Accumulator)> = HashMap::new();
    let mut by_tool: HashMap<String, (u64, u64, Vec<f64>)> = HashMap::new();
    let mut by_step: HashMap<(String, String), (u64, u64, Vec<f64>)> = HashMap::new();
    let mut step_latencies = Vec::new();

    // The timeline's extent comes from the runs themselves when no window was
    // asked for, so an idle system does not render an empty chart of the last
    // hour.
    let earliest = matching.iter().map(|run| run.started_at).min();
    let window_from = from.or(earliest).unwrap_or(now);
    let mut timeline = empty_timeline(window_from, now, filter.buckets);
    let width = (now - window_from).as_seconds_f64().max(1.0) / timeline.len() as f64;

    for run in &matching {
        let offset = (run.started_at - window_from).as_seconds_f64();
        let index =
            ((offset / width).floor() as isize).clamp(0, timeline.len() as isize - 1) as usize;
        let Some(bucket) = timeline.get_mut(index) else {
            continue;
        };
        bucket.runs += 1;
        match run.status {
            RunStatus::Succeeded => {
                totals.succeeded += 1;
                bucket.succeeded += 1;
            }
            RunStatus::Failed => {
                totals.failed += 1;
                bucket.failed += 1;
            }
            RunStatus::Running => {
                totals.running += 1;
                bucket.running += 1;
            }
        }
        if let Some(duration) = run.duration_ms {
            run_latencies.push(duration as f64);
        }

        for agent in &run.agents {
            let entry = by_agent.entry(agent.clone()).or_default();
            entry.runs += 1;
            if run.status == RunStatus::Failed {
                entry.failures += 1;
            }
        }

        let Some(run_spans) = spans.get(&run.run_id) else {
            // The run is known but its spans were evicted or never closed.
            // Its run-level counters still count.
            continue;
        };

        for span in run_spans {
            let Some(operation) = operation(span) else {
                continue;
            };
            let agent = string_attr(span, genai::AGENT_ID).unwrap_or("").to_owned();
            let elapsed = duration_ms(span);

            match operation {
                genai::operation::CHAT => {
                    let model = string_attr(span, genai::REQUEST_MODEL)
                        .unwrap_or("unknown")
                        .to_owned();
                    if filter.model.as_ref().is_some_and(|wanted| wanted != &model) {
                        continue;
                    }

                    let input = int_attr(span, genai::USAGE_INPUT_TOKENS);
                    let output = int_attr(span, genai::USAGE_OUTPUT_TOKENS);
                    let cached = int_attr(span, "gen_ai.usage.cached_tokens");
                    let cost = double_attr(span, own::usage::COST_USD);

                    totals.llm_calls += 1;
                    totals.input_tokens += input;
                    totals.output_tokens += output;
                    totals.cached_tokens += cached;
                    add_cost(&mut totals.cost_usd, cost);
                    if cost.is_some() {
                        totals.costed_calls += 1;
                    }
                    bucket.llm_calls += 1;
                    bucket.input_tokens += input;
                    bucket.output_tokens += output;
                    bucket.cached_tokens += cached;
                    add_cost(&mut bucket.cost_usd, cost);
                    llm_latencies.push(elapsed);

                    // Time to first token: the assembler records it as a span
                    // event, so the gap to the span start is TTFT.
                    if let Some(event) = span
                        .events
                        .iter()
                        .find(|event| event.name == "gen_ai.first_token")
                    {
                        ttft.push((event.at - span.start).as_seconds_f64() * 1000.0);
                    }

                    let (provider, entry) = by_model
                        .entry(model)
                        .or_insert_with(|| (None, Accumulator::default()));
                    if provider.is_none() {
                        *provider = string_attr(span, genai::PROVIDER_NAME).map(ToOwned::to_owned);
                    }
                    entry.llm_calls += 1;
                    entry.input_tokens += input;
                    entry.output_tokens += output;
                    entry.cached_tokens += cached;
                    add_cost(&mut entry.cost_usd, cost);
                    entry.llm_latencies.push(elapsed);
                    if failed(span) {
                        entry.failures += 1;
                    }

                    if !agent.is_empty() {
                        let entry = by_agent.entry(agent).or_default();
                        entry.llm_calls += 1;
                        entry.input_tokens += input;
                        entry.output_tokens += output;
                        entry.cached_tokens += cached;
                        add_cost(&mut entry.cost_usd, cost);
                        entry.llm_latencies.push(elapsed);
                    }
                }
                genai::operation::EXECUTE_TOOL => {
                    totals.tool_calls += 1;
                    bucket.tool_calls += 1;
                    tool_latencies.push(elapsed);
                    let name = string_attr(span, genai::TOOL_NAME)
                        .unwrap_or("unknown")
                        .to_owned();
                    let entry = by_tool.entry(name).or_insert((0, 0, Vec::new()));
                    entry.0 += 1;
                    if failed(span) {
                        entry.1 += 1;
                    }
                    entry.2.push(elapsed);

                    if !agent.is_empty() {
                        by_agent.entry(agent).or_default().tool_calls += 1;
                    }
                }
                "step" => {
                    totals.step_calls += 1;
                    step_latencies.push(elapsed);
                    let step_type = string_attr(span, own::span::STEP_TYPE)
                        .unwrap_or("step")
                        .to_owned();
                    let name = string_attr(span, own::span::STEP_NAME)
                        .unwrap_or(span.name.as_str())
                        .to_owned();
                    let entry = by_step
                        .entry((step_type, name))
                        .or_insert((0, 0, Vec::new()));
                    entry.0 += 1;
                    if failed(span) {
                        entry.1 += 1;
                    }
                    entry.2.push(elapsed);
                }
                _ => {}
            }
        }
    }

    totals.cache_hit_ratio = if totals.input_tokens > 0 {
        totals.cached_tokens as f64 / totals.input_tokens as f64
    } else {
        0.0
    };

    let mut by_agent: Vec<AgentBreakdown> = by_agent
        .into_iter()
        .map(|(agent_id, mut acc)| AgentBreakdown {
            agent_id,
            runs: acc.runs,
            llm_calls: acc.llm_calls,
            tool_calls: acc.tool_calls,
            input_tokens: acc.input_tokens,
            output_tokens: acc.output_tokens,
            cached_tokens: acc.cached_tokens,
            cost_usd: acc.cost_usd,
            failures: acc.failures,
            llm_latency: percentiles(&mut acc.llm_latencies),
        })
        .collect();
    // Most expensive first: the ordering someone opening a cost view wants.
    by_agent.sort_by_key(|agent| {
        std::cmp::Reverse(agent.input_tokens.saturating_add(agent.output_tokens))
    });

    let mut by_model: Vec<ModelBreakdown> = by_model
        .into_iter()
        .map(|(model, (provider, mut acc))| ModelBreakdown {
            model,
            provider,
            calls: acc.llm_calls,
            failures: acc.failures,
            input_tokens: acc.input_tokens,
            output_tokens: acc.output_tokens,
            cached_tokens: acc.cached_tokens,
            cost_usd: acc.cost_usd,
            latency: percentiles(&mut acc.llm_latencies),
        })
        .collect();
    by_model.sort_by_key(|model| {
        std::cmp::Reverse(model.input_tokens.saturating_add(model.output_tokens))
    });

    let mut by_tool: Vec<ToolBreakdown> = by_tool
        .into_iter()
        .map(
            |(tool_name, (calls, failures, mut latencies))| ToolBreakdown {
                tool_name,
                calls,
                failures,
                latency: percentiles(&mut latencies),
            },
        )
        .collect();
    by_tool.sort_by_key(|tool| std::cmp::Reverse(tool.calls));

    let mut by_step: Vec<StepBreakdown> = by_step
        .into_iter()
        .map(
            |((step_type, name), (calls, failures, mut latencies))| StepBreakdown {
                step_type,
                name,
                calls,
                failures,
                latency: percentiles(&mut latencies),
            },
        )
        .collect();
    // Slowest first: a step breakdown is opened to find what is taking the
    // time, not to count calls.
    by_step.sort_by(|a, b| {
        b.latency
            .p95
            .partial_cmp(&a.latency.p95)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    MetricsSummary {
        window: MetricsWindow {
            from: window_from,
            to: now,
            runs_considered: totals.runs,
            runs_retained: retained,
            retention_limit: retention_limit as u64,
        },
        totals,
        latency: Latency {
            run: percentiles(&mut run_latencies),
            llm: percentiles(&mut llm_latencies),
            tool: percentiles(&mut tool_latencies),
            step: percentiles(&mut step_latencies),
            time_to_first_token: percentiles(&mut ttft),
        },
        by_agent,
        by_model,
        by_tool,
        by_step,
        timeline,
    }
}

/// Calls and tokens are attributed to the run's start, like its status.
/// Filling these buckets in the totals fold keeps filters and span retention
/// identical across the headline numbers, breakdowns and timeline.
fn empty_timeline(
    from: OffsetDateTime,
    to: OffsetDateTime,
    requested: Option<usize>,
) -> Vec<Bucket> {
    let count = requested.unwrap_or(48).clamp(6, 200);
    let width = (to - from).as_seconds_f64().max(1.0) / count as f64;
    (0..count)
        .map(|index| Bucket {
            at: from + time::Duration::seconds_f64(width * index as f64),
            runs: 0,
            succeeded: 0,
            running: 0,
            failed: 0,
            llm_calls: 0,
            tool_calls: 0,
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
            cost_usd: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use aiwatcher_core::ports::{SpanEvent, SpanKind, attr};
    use aiwatcher_core::{SpanId, TraceId};

    use super::*;

    fn run(run_id: &str, status: RunStatus, started: OffsetDateTime) -> RunSummary {
        RunSummary {
            run_id: run_id.to_owned(),
            conversation_id: Some("conv".to_owned()),
            trace_id: TraceId::derive(run_id),
            status,
            agents: vec!["researcher".to_owned()],
            runtimes: vec!["agent-service".to_owned()],
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
            started_at: started,
            last_event_at: started + time::Duration::seconds(2),
            ended_at: Some(started + time::Duration::seconds(2)),
            duration_ms: Some(2000),
            event_count: 6,
            llm_calls: 1,
            tool_calls: 1,
            input_tokens: 800,
            output_tokens: 200,
            cached_tokens: 400,
            cost_usd: None,
            costed_calls: 0,
            error: None,
            last_checkpoint: aiwatcher_core::Checkpoint::from_global_position(1),
        }
    }

    fn llm_span(run_id: &str, model: &str, millis: i64, with_first_token: bool) -> CompletedSpan {
        let trace_id = TraceId::derive(run_id);
        let start = datetime!(2026-08-27 18:20:00 UTC);
        CompletedSpan {
            trace_id,
            span_id: SpanId::derive(trace_id, &format!("llm:{model}:{millis}")),
            parent_span_id: None,
            name: format!("chat {model}"),
            kind: SpanKind::Client,
            start,
            end: start + time::Duration::milliseconds(millis),
            status: SpanStatus::Ok,
            attributes: vec![
                attr(genai::OPERATION_NAME, genai::operation::CHAT),
                attr(genai::REQUEST_MODEL, model),
                attr(genai::PROVIDER_NAME, "anthropic"),
                attr(genai::AGENT_ID, "researcher"),
                attr(genai::USAGE_INPUT_TOKENS, 800i64),
                attr(genai::USAGE_OUTPUT_TOKENS, 200i64),
                attr("gen_ai.usage.cached_tokens", 400i64),
                attr(own::run::ID, run_id),
            ],
            events: if with_first_token {
                vec![SpanEvent {
                    name: "gen_ai.first_token".to_owned(),
                    at: start + time::Duration::milliseconds(120),
                    attributes: Vec::new(),
                }]
            } else {
                Vec::new()
            },
            links: Vec::new(),
        }
    }

    fn tool_span(run_id: &str, tool: &str, millis: i64, ok: bool) -> CompletedSpan {
        let trace_id = TraceId::derive(run_id);
        let start = datetime!(2026-08-27 18:20:00 UTC);
        CompletedSpan {
            trace_id,
            span_id: SpanId::derive(trace_id, &format!("tool:{tool}")),
            parent_span_id: None,
            name: format!("execute_tool {tool}"),
            kind: SpanKind::Client,
            start,
            end: start + time::Duration::milliseconds(millis),
            status: if ok {
                SpanStatus::Ok
            } else {
                SpanStatus::Error {
                    message: "boom".to_owned(),
                }
            },
            attributes: vec![
                attr(genai::OPERATION_NAME, genai::operation::EXECUTE_TOOL),
                attr(genai::TOOL_NAME, tool),
                attr(genai::AGENT_ID, "researcher"),
                attr(own::run::ID, run_id),
            ],
            events: Vec::new(),
            links: Vec::new(),
        }
    }

    fn now() -> OffsetDateTime {
        datetime!(2026-08-27 18:30:00 UTC)
    }

    #[test]
    fn totals_add_up_across_runs() {
        let runs = vec![
            run(
                "a",
                RunStatus::Succeeded,
                datetime!(2026-08-27 18:20:00 UTC),
            ),
            run("b", RunStatus::Failed, datetime!(2026-08-27 18:21:00 UTC)),
            run("c", RunStatus::Running, datetime!(2026-08-27 18:22:00 UTC)),
        ];
        let spans = HashMap::new();
        let summary = compute(&runs, &spans, &MetricsFilter::default(), 5000, now());

        assert_eq!(summary.totals.runs, 3);
        assert_eq!(summary.totals.succeeded, 1);
        assert_eq!(summary.totals.failed, 1);
        assert_eq!(summary.totals.running, 1);
        assert_eq!(summary.window.runs_retained, 3);
        for bucket in &summary.timeline {
            assert_eq!(
                bucket.runs,
                bucket.succeeded + bucket.failed + bucket.running
            );
        }
        assert_eq!(summary.timeline.iter().map(|b| b.succeeded).sum::<u64>(), 1);
        assert_eq!(summary.timeline.iter().map(|b| b.failed).sum::<u64>(), 1);
        assert_eq!(summary.timeline.iter().map(|b| b.running).sum::<u64>(), 1);
        assert_eq!(
            summary.timeline.iter().map(|b| b.input_tokens).sum::<i64>(),
            0,
            "evicted or unclosed spans supply no token evidence, just like totals"
        );
    }

    #[test]
    fn llm_spans_supply_tokens_models_and_time_to_first_token() {
        let runs = vec![run(
            "a",
            RunStatus::Succeeded,
            datetime!(2026-08-27 18:20:00 UTC),
        )];
        let mut spans = HashMap::new();
        spans.insert(
            "a".to_owned(),
            vec![
                llm_span("a", "claude-opus-5", 1200, true),
                llm_span("a", "claude-opus-5", 400, false),
                tool_span("a", "web_search", 300, true),
            ],
        );
        let summary = compute(&runs, &spans, &MetricsFilter::default(), 5000, now());

        assert_eq!(summary.totals.llm_calls, 2);
        assert_eq!(summary.totals.tool_calls, 1);
        assert_eq!(summary.totals.input_tokens, 1600);
        assert_eq!(summary.totals.output_tokens, 400);
        assert_eq!(summary.totals.cached_tokens, 800);
        assert!((summary.totals.cache_hit_ratio - 0.5).abs() < 1e-9);

        assert_eq!(summary.latency.llm.count, 2);
        assert!(
            (summary.latency.llm.p50 - 400.0).abs() < 1.0,
            "nearest-rank p50 of [400,1200]"
        );
        assert!((summary.latency.llm.p95 - 1200.0).abs() < 1.0);
        assert_eq!(
            summary.latency.time_to_first_token.count, 1,
            "only the span carrying the first-token event contributes"
        );
        assert!((summary.latency.time_to_first_token.p50 - 120.0).abs() < 1.0);

        assert_eq!(summary.by_model.len(), 1);
        assert_eq!(summary.by_model[0].model, "claude-opus-5");
        assert_eq!(summary.by_model[0].provider.as_deref(), Some("anthropic"));
        assert_eq!(summary.by_model[0].calls, 2);
    }

    #[test]
    fn a_reported_cost_is_summed_where_it_was_reported_and_counted_where_it_was_not() {
        let mut first = run(
            "a",
            RunStatus::Succeeded,
            datetime!(2026-08-27 18:20:00 UTC),
        );
        first.cost_usd = Some(0.000_3);
        let runs = vec![first];
        let mut billed = llm_span("a", "gemini-3.7-flash", 900, false);
        billed
            .attributes
            .push(attr(own::usage::COST_USD, 0.000_3_f64));
        let mut spans = HashMap::new();
        spans.insert(
            "a".to_owned(),
            vec![billed, llm_span("a", "claude-opus-5", 400, false)],
        );
        let summary = compute(&runs, &spans, &MetricsFilter::default(), 5000, now());

        assert_eq!(summary.totals.llm_calls, 2);
        assert_eq!(
            summary.totals.costed_calls, 1,
            "one of the two calls said what it cost"
        );
        assert!((summary.totals.cost_usd.expect("one call reported") - 0.000_3).abs() < 1e-12);

        let quiet = summary
            .by_model
            .iter()
            .find(|model| model.model == "claude-opus-5")
            .expect("the quiet model");
        assert_eq!(
            quiet.cost_usd, None,
            "a model whose calls said nothing about money has an unknown cost, not a free one"
        );
        let billed = summary
            .by_model
            .iter()
            .find(|model| model.model == "gemini-3.7-flash")
            .expect("the billed model");
        assert!(billed.cost_usd.is_some());
        assert!(summary.by_agent[0].cost_usd.is_some());
        assert!(
            summary
                .timeline
                .iter()
                .any(|bucket| bucket.cost_usd.is_some()),
            "the run's cost lands in the bucket it started in"
        );
    }

    #[test]
    fn a_failing_tool_is_counted_separately_from_its_calls() {
        let runs = vec![run(
            "a",
            RunStatus::Succeeded,
            datetime!(2026-08-27 18:20:00 UTC),
        )];
        let mut spans = HashMap::new();
        spans.insert(
            "a".to_owned(),
            vec![
                tool_span("a", "web_search", 300, true),
                tool_span("a", "web_search", 900, false),
            ],
        );
        let summary = compute(&runs, &spans, &MetricsFilter::default(), 5000, now());

        assert_eq!(summary.by_tool.len(), 1);
        assert_eq!(summary.by_tool[0].calls, 2);
        assert_eq!(summary.by_tool[0].failures, 1);
        assert_eq!(summary.by_tool[0].latency.count, 2);
    }

    #[test]
    fn the_window_excludes_runs_that_started_before_it() {
        let runs = vec![
            run(
                "old",
                RunStatus::Succeeded,
                datetime!(2026-08-27 17:00:00 UTC),
            ),
            run(
                "new",
                RunStatus::Succeeded,
                datetime!(2026-08-27 18:25:00 UTC),
            ),
        ];
        let summary = compute(
            &runs,
            &HashMap::new(),
            &MetricsFilter {
                window_seconds: Some(600),
                ..MetricsFilter::default()
            },
            5000,
            now(),
        );
        assert_eq!(
            summary.totals.runs, 1,
            "only the run inside the last 10 minutes"
        );
        assert_eq!(
            summary.window.runs_retained, 2,
            "but both are still retained"
        );
    }

    #[test]
    fn filtering_by_agent_narrows_the_runs() {
        let mut other = run(
            "b",
            RunStatus::Succeeded,
            datetime!(2026-08-27 18:21:00 UTC),
        );
        other.agents = vec!["planner".to_owned()];
        let runs = vec![
            run(
                "a",
                RunStatus::Succeeded,
                datetime!(2026-08-27 18:20:00 UTC),
            ),
            other,
        ];
        let summary = compute(
            &runs,
            &HashMap::new(),
            &MetricsFilter {
                agent_id: Some("planner".to_owned()),
                ..MetricsFilter::default()
            },
            5000,
            now(),
        );
        assert_eq!(summary.totals.runs, 1);
    }

    #[test]
    fn the_timeline_covers_the_window_and_places_each_run_once() {
        let runs = vec![
            run(
                "a",
                RunStatus::Succeeded,
                datetime!(2026-08-27 18:20:00 UTC),
            ),
            run("b", RunStatus::Failed, datetime!(2026-08-27 18:20:30 UTC)),
        ];
        let spans = HashMap::from([
            ("a".to_owned(), vec![llm_span("a", "opus", 100, false)]),
            ("b".to_owned(), vec![llm_span("b", "opus", 100, false)]),
        ]);
        let summary = compute(
            &runs,
            &spans,
            &MetricsFilter {
                window_seconds: Some(600),
                buckets: Some(10),
                ..MetricsFilter::default()
            },
            5000,
            now(),
        );

        assert_eq!(summary.timeline.len(), 10);
        assert_eq!(
            summary.timeline.iter().map(|b| b.runs).sum::<u64>(),
            2,
            "each run lands in exactly one bucket"
        );
        assert_eq!(summary.timeline.iter().map(|b| b.failed).sum::<u64>(), 1);
        assert_eq!(
            summary.timeline.iter().map(|b| b.input_tokens).sum::<i64>(),
            1600
        );
    }

    #[test]
    fn model_filter_keeps_timeline_totals_and_breakdowns_on_the_same_evidence() {
        let runs = vec![
            run(
                "a",
                RunStatus::Succeeded,
                now() - time::Duration::minutes(5),
            ),
            run("b", RunStatus::Running, now()),
        ];
        let spans = HashMap::from([(
            "a".to_owned(),
            vec![
                llm_span("a", "opus", 900, true),
                llm_span("a", "haiku", 100, false),
                tool_span("a", "search", 200, true),
            ],
        )]);
        for model in [None, Some("opus"), Some("missing")] {
            let summary = compute(
                &runs,
                &spans,
                &MetricsFilter {
                    model: model.map(str::to_owned),
                    ..MetricsFilter::default()
                },
                5000,
                now(),
            );
            assert_eq!(summary.totals.runs, 2, "model narrows LLM calls, not runs");
            assert_eq!(
                summary.totals.llm_calls,
                match model {
                    None => 2,
                    Some("opus") => 1,
                    _ => 0,
                }
            );
            assert_eq!(
                summary.timeline.iter().map(|b| b.input_tokens).sum::<i64>(),
                summary.totals.input_tokens
            );
            assert_eq!(
                summary
                    .timeline
                    .iter()
                    .map(|b| b.output_tokens)
                    .sum::<i64>(),
                summary.totals.output_tokens
            );
            assert_eq!(
                summary
                    .timeline
                    .iter()
                    .map(|b| b.cached_tokens)
                    .sum::<i64>(),
                summary.totals.cached_tokens
            );
            assert_eq!(
                summary.timeline.iter().map(|b| b.llm_calls).sum::<u64>(),
                summary.totals.llm_calls
            );
            assert_eq!(
                summary.timeline.iter().map(|b| b.tool_calls).sum::<u64>(),
                summary.totals.tool_calls
            );
            assert_eq!(
                summary.by_model.iter().map(|m| m.input_tokens).sum::<i64>(),
                summary.totals.input_tokens
            );
            assert_eq!(
                summary.timeline.last().expect("last bucket").running,
                1,
                "run at the right boundary is included once"
            );
        }
    }

    #[test]
    fn timeline_costs_follow_model_filters_and_span_retention_without_double_counting() {
        let mut observed = run("a", RunStatus::Succeeded, now());
        observed.cost_usd = Some(99.0); // Run totals must not be added to span costs.
        let runs = vec![observed];
        let mut opus = llm_span("a", "opus", 100, true);
        opus.attributes.push(attr(own::usage::COST_USD, 0.25_f64));
        let mut haiku = llm_span("a", "haiku", 100, true);
        haiku.attributes.push(attr(own::usage::COST_USD, 0.5_f64));
        let spans = HashMap::from([("a".to_owned(), vec![opus, haiku])]);
        for (model, expected, calls) in [
            (None, Some(0.75), 2),
            (Some("opus"), Some(0.25), 1),
            (Some("missing"), None, 0),
        ] {
            let summary = compute(
                &runs,
                &spans,
                &MetricsFilter {
                    model: model.map(str::to_owned),
                    ..MetricsFilter::default()
                },
                5000,
                now(),
            );
            assert_eq!(summary.totals.cost_usd, expected);
            assert_eq!(summary.totals.costed_calls, calls);
            let costs: Vec<_> = summary
                .timeline
                .iter()
                .filter_map(|bucket| bucket.cost_usd)
                .collect();
            assert_eq!(costs, expected.into_iter().collect::<Vec<_>>());
            assert_eq!(
                summary
                    .timeline
                    .iter()
                    .map(|bucket| bucket.runs)
                    .sum::<u64>(),
                1
            );
        }
        let evicted = compute(
            &runs,
            &HashMap::new(),
            &MetricsFilter::default(),
            5000,
            now(),
        );
        assert_eq!(evicted.totals.cost_usd, None);
        assert!(
            evicted
                .timeline
                .iter()
                .all(|bucket| bucket.cost_usd.is_none())
        );
    }

    fn step_span(
        run_id: &str,
        step_type: &str,
        name: &str,
        millis: i64,
        ok: bool,
    ) -> CompletedSpan {
        let trace_id = TraceId::derive(run_id);
        let start = datetime!(2026-08-27 18:20:00 UTC);
        CompletedSpan {
            trace_id,
            span_id: SpanId::derive(trace_id, &format!("step:{name}:{millis}")),
            parent_span_id: None,
            name: name.to_owned(),
            kind: SpanKind::Client,
            start,
            end: start + time::Duration::milliseconds(millis),
            status: if ok {
                SpanStatus::Ok
            } else {
                SpanStatus::Error {
                    message: "blocked".to_owned(),
                }
            },
            attributes: vec![
                attr(genai::OPERATION_NAME, "step"),
                attr(own::span::STEP_TYPE, step_type),
                attr(own::span::STEP_NAME, name),
                attr(own::run::ID, run_id),
            ],
            events: Vec::new(),
            links: Vec::new(),
        }
    }

    #[test]
    fn steps_get_their_own_breakdown_ordered_by_what_is_slow() {
        let runs = vec![run(
            "a",
            RunStatus::Succeeded,
            datetime!(2026-08-27 18:20:00 UTC),
        )];
        let mut spans = HashMap::new();
        spans.insert(
            "a".to_owned(),
            vec![
                step_span("a", "retriever", "knowledge_base", 340, true),
                step_span("a", "retriever", "knowledge_base", 120, true),
                step_span("a", "parser", "json", 4, true),
                step_span("a", "guardrail", "pii", 9, false),
            ],
        );
        let summary = compute(&runs, &spans, &MetricsFilter::default(), 5000, now());

        assert_eq!(summary.totals.step_calls, 4);
        assert_eq!(summary.latency.step.count, 4);
        assert_eq!(summary.by_step.len(), 3, "grouped by (kind, name)");
        assert_eq!(
            summary.by_step[0].name, "knowledge_base",
            "the slowest leads — a step breakdown is opened to find time, not counts"
        );
        assert_eq!(summary.by_step[0].calls, 2);
        assert_eq!(summary.by_step[0].step_type, "retriever");

        let guardrail = summary
            .by_step
            .iter()
            .find(|step| step.name == "pii")
            .expect("the guardrail");
        assert_eq!(guardrail.failures, 1);
    }

    #[test]
    fn percentiles_of_an_empty_sample_are_zero_rather_than_absent() {
        let summary = compute(&[], &HashMap::new(), &MetricsFilter::default(), 5000, now());
        assert_eq!(summary.latency.llm.count, 0);
        assert_eq!(summary.latency.llm.p95, 0.0);
        assert_eq!(summary.totals.cache_hit_ratio, 0.0, "no division by zero");
        assert!(summary.by_model.is_empty());
    }

    #[test]
    fn breakdowns_are_ordered_by_what_costs_most() {
        let runs = vec![run(
            "a",
            RunStatus::Succeeded,
            datetime!(2026-08-27 18:20:00 UTC),
        )];
        let mut spans = HashMap::new();
        let mut cheap = llm_span("a", "haiku", 100, false);
        for (key, value) in cheap.attributes.iter_mut() {
            if key == genai::USAGE_INPUT_TOKENS {
                *value = AttrValue::Int(10);
            }
            if key == genai::USAGE_OUTPUT_TOKENS {
                *value = AttrValue::Int(5);
            }
        }
        spans.insert(
            "a".to_owned(),
            vec![cheap, llm_span("a", "opus", 900, false)],
        );

        let summary = compute(&runs, &spans, &MetricsFilter::default(), 5000, now());
        assert_eq!(
            summary.by_model[0].model, "opus",
            "the expensive model leads"
        );
        assert_eq!(summary.by_model[1].model, "haiku");
    }
}
