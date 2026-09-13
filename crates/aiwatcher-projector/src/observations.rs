//! What a declared variant was observed doing, from the runs that name it.
//!
//! A published result says what a variant scored on a pinned set of cases; a
//! run carrying its `variant_id` is the same variant answering somebody using
//! the application. The two are different samples on different clocks — the
//! evidence outlives the log, this is gone with the run — so they are never
//! folded into one number, and a run the variant made to answer a
//! measurement's case (`evaluation_id` on `run.started`) is counted apart and
//! in no figure here: a benchmark is not an observation.
//!
//! Folded from the read model, like [`crate::dimensions`], and bounded by what
//! it holds — so closed periods are also written down as they close
//! ([`ObservedPeriod`], [`crate::periods`]), and a window reaching further back
//! than the read model is answered from those, whole periods at a time, with
//! the live fold for the runs no written period holds. A period keeps its
//! durations as a histogram, so periods add up and still answer a percentile,
//! within one bucket.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use aiwatcher_core::attrs::genai;
use aiwatcher_core::ports::{AttrValue, CompletedSpan};
use aiwatcher_core::prices::ModelPrices;

use crate::readmodel::{RunStatus, RunSummary};

/// One variant's runs in the window.
#[derive(Clone, Debug, PartialEq, Serialize, utoipa::ToSchema)]
pub struct VariantObservations {
    pub variant_id: String,
    /// Runs naming the variant that no measurement made.
    pub runs: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub running: u64,
    /// Runs naming it that answered a measurement's cases, left out of every
    /// other figure.
    pub measured_runs: u64,
    /// How long a finished run took, end to end. A run is one request
    /// answered, so this stands beside a result's per-case latency — on
    /// another clock, over other inputs. Absent when none finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<DurationSummary>,
    /// How long each model call took, from the spans of those runs — which the
    /// read model sheds before it sheds a run, so it may count fewer calls
    /// than `llm_calls`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_ms: Option<DurationSummary>,
    /// Time to a call's first token, where the call reported one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_to_first_token_ms: Option<DurationSummary>,
    pub llm_calls: u64,
    /// What the runs' model calls reported. A call that reported no usage
    /// counts nothing, which the call count beside it lets a reader see.
    pub input_tokens: i64,
    pub output_tokens: i64,
    /// The same calls by the model they named, from their spans.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ObservedModel>,
    /// What those calls cost at the deployment's price table. Absent when the
    /// deployment loaded none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<ObservedCost>,
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub first_seen_at: Option<OffsetDateTime>,
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_seen_at: Option<OffsetDateTime>,
    /// Written periods these figures include. Nought when the window asked for
    /// none, or reached no further back than what the read model holds.
    pub periods: usize,
    /// Of `runs`, those counted from written periods rather than from the read
    /// model — which leaves out every run that ended in one.
    pub runs_from_periods: u64,
    /// Of the periods, those whose fold could not vouch it held every run that
    /// ended in them.
    pub incomplete_periods: usize,
}

/// Durations, in milliseconds, over this many.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct DurationSummary {
    pub count: u64,
    pub p50: i64,
    pub p90: i64,
    pub p99: i64,
    pub max: i64,
    /// The percentiles are a bucket's upper bound rather than a duration
    /// somebody measured — at most an eighth of a doubling high — because a
    /// written period keeps buckets.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bucketed: bool,
}

impl DurationSummary {
    fn exact(mut durations: Vec<i64>) -> Option<Self> {
        durations.sort_unstable();
        let last = durations.len().checked_sub(1)?;
        let rank = |quantile: f64| {
            let at = ((quantile * durations.len() as f64).ceil() as usize).max(1) - 1;
            durations[at.min(last)]
        };
        Some(Self {
            count: durations.len() as u64,
            p50: rank(0.5),
            p90: rank(0.9),
            p99: rank(0.99),
            max: durations[last],
            bucketed: false,
        })
    }
}

/// Durations in milliseconds, in buckets an eighth of a doubling wide: what a
/// written period keeps, so periods add and still answer a percentile.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DurationHistogram {
    /// Bucket to how many durations fell in it. Bucket `b` holds durations
    /// below `2^((b+1)/8) − 1` milliseconds.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub buckets: BTreeMap<u16, u64>,
    pub count: u64,
    pub max: i64,
}

impl DurationHistogram {
    fn bucket(milliseconds: i64) -> u16 {
        let place = ((milliseconds.max(0) as f64 + 1.0).log2() * 8.0).floor();
        place.clamp(0.0, f64::from(u16::MAX)) as u16
    }

    fn upper(bucket: u16) -> i64 {
        ((f64::from(bucket) + 1.0) / 8.0).exp2().ceil() as i64 - 1
    }

    pub fn add(&mut self, milliseconds: i64) {
        *self.buckets.entry(Self::bucket(milliseconds)).or_default() += 1;
        self.count += 1;
        self.max = self.max.max(milliseconds);
    }

    pub fn merge(&mut self, other: &Self) {
        for (bucket, count) in &other.buckets {
            *self.buckets.entry(*bucket).or_default() += count;
        }
        self.count += other.count;
        self.max = self.max.max(other.max);
    }

    /// Nearest-rank percentiles, each the upper bound of the bucket the rank
    /// falls in and never above the largest duration counted.
    #[must_use]
    pub fn summary(&self) -> Option<DurationSummary> {
        if self.count == 0 {
            return None;
        }
        let rank = |quantile: f64| {
            let wanted = ((quantile * self.count as f64).ceil() as u64).max(1);
            let mut seen = 0;
            for (bucket, count) in &self.buckets {
                seen += count;
                if seen >= wanted {
                    return Self::upper(*bucket).min(self.max);
                }
            }
            self.max
        };
        Some(DurationSummary {
            count: self.count,
            p50: rank(0.5),
            p90: rank(0.9),
            p99: rank(0.99),
            max: self.max,
            bucketed: true,
        })
    }
}

/// Model calls that named one model, and what they reported using.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ObservedModel {
    pub model: String,
    pub calls: u64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_tokens: i64,
}

/// What calls cost at the deployment's prices, and what the figure rests on.
#[derive(Clone, Debug, PartialEq, Serialize, utoipa::ToSchema)]
pub struct ObservedCost {
    pub currency: String,
    pub amount: f64,
    /// Calls whose model the table prices.
    pub priced_calls: u64,
    /// Calls whose model it does not, which cost something nobody priced —
    /// never nought.
    pub unpriced_calls: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unpriced_models: Vec<String>,
    /// Where each price used was read, and when.
    pub prices: Vec<PriceUsed>,
}

/// One price a cost used, with where and when it was read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct PriceUsed {
    pub model: String,
    pub source: String,
    pub as_of: String,
}

/// One variant's runs that ended in one closed period, as written down when the
/// period closed — the record that outlives the read model.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ObservedPeriod {
    pub variant_id: String,
    /// The period, `[from, to)`, in Unix seconds.
    pub from: i64,
    pub to: i64,
    pub runs: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub measured_runs: u64,
    pub run_ms: DurationHistogram,
    pub call_ms: DurationHistogram,
    pub time_to_first_token_ms: DurationHistogram,
    pub llm_calls: u64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ObservedModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_seen_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<i64>,
    /// Whether the fold that wrote it held every run that ended in the period
    /// and every such run's spans.
    pub complete: bool,
}

/// One variant's figures while they are being folded.
#[derive(Debug, Default)]
struct Accumulated {
    runs: u64,
    succeeded: u64,
    failed: u64,
    running: u64,
    measured_runs: u64,
    run_ms: Vec<i64>,
    call_ms: Vec<i64>,
    ttft_ms: Vec<i64>,
    llm_calls: u64,
    input_tokens: i64,
    output_tokens: i64,
    models: BTreeMap<String, ObservedModel>,
    first_seen_at: Option<OffsetDateTime>,
    last_seen_at: Option<OffsetDateTime>,
}

impl Accumulated {
    fn add(&mut self, run: &RunSummary, spans: Option<&Vec<CompletedSpan>>) {
        if run.evaluation_id.is_some() {
            self.measured_runs += 1;
            return;
        }
        self.runs += 1;
        match run.status {
            RunStatus::Succeeded => self.succeeded += 1,
            RunStatus::Failed => self.failed += 1,
            RunStatus::Running => self.running += 1,
        }
        if let Some(duration) = run.duration_ms {
            self.run_ms.push(duration);
        }
        self.llm_calls += run.llm_calls;
        self.input_tokens += run.input_tokens;
        self.output_tokens += run.output_tokens;
        self.first_seen_at = Some(
            self.first_seen_at
                .map_or(run.started_at, |seen| seen.min(run.started_at)),
        );
        self.last_seen_at = Some(
            self.last_seen_at
                .map_or(run.last_event_at, |seen| seen.max(run.last_event_at)),
        );
        for span in spans.into_iter().flatten() {
            if text(span, genai::OPERATION_NAME) != Some(genai::operation::CHAT) {
                continue;
            }
            self.call_ms
                .push((span.end - span.start).whole_milliseconds() as i64);
            if let Some(first) = span
                .events
                .iter()
                .find(|event| event.name == "gen_ai.first_token")
            {
                self.ttft_ms
                    .push((first.at - span.start).whole_milliseconds() as i64);
            }
            let model = text(span, genai::REQUEST_MODEL).unwrap_or("unknown");
            let entry = self
                .models
                .entry(model.to_owned())
                .or_insert_with(|| ObservedModel {
                    model: model.to_owned(),
                    ..ObservedModel::default()
                });
            entry.calls += 1;
            entry.input_tokens += number(span, genai::USAGE_INPUT_TOKENS);
            entry.output_tokens += number(span, genai::USAGE_OUTPUT_TOKENS);
            entry.cached_tokens += number(span, "gen_ai.usage.cached_tokens");
        }
    }

    fn period(self, variant_id: &str, from: i64, to: i64, complete: bool) -> ObservedPeriod {
        let histogram = |values: &[i64]| {
            let mut histogram = DurationHistogram::default();
            for value in values {
                histogram.add(*value);
            }
            histogram
        };
        ObservedPeriod {
            variant_id: variant_id.to_owned(),
            from,
            to,
            runs: self.runs,
            succeeded: self.succeeded,
            failed: self.failed,
            measured_runs: self.measured_runs,
            run_ms: histogram(&self.run_ms),
            call_ms: histogram(&self.call_ms),
            time_to_first_token_ms: histogram(&self.ttft_ms),
            llm_calls: self.llm_calls,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            models: self.models.into_values().collect(),
            first_seen_at: self.first_seen_at.map(OffsetDateTime::unix_timestamp),
            last_seen_at: self.last_seen_at.map(OffsetDateTime::unix_timestamp),
            complete,
        }
    }

    /// The figures a reader sees: this fold's, and every written period's.
    fn observations(
        self,
        variant_id: &str,
        periods: &[&ObservedPeriod],
        prices: Option<&ModelPrices>,
    ) -> VariantObservations {
        let mut models = self.models;
        let (mut runs, mut succeeded, mut failed, mut measured) =
            (self.runs, self.succeeded, self.failed, self.measured_runs);
        let (mut llm_calls, mut input, mut output) =
            (self.llm_calls, self.input_tokens, self.output_tokens);
        let (mut first, mut last) = (self.first_seen_at, self.last_seen_at);
        let seconds = |at: i64| OffsetDateTime::from_unix_timestamp(at).ok();
        let mut run_ms = DurationHistogram::default();
        let mut call_ms = DurationHistogram::default();
        let mut ttft_ms = DurationHistogram::default();
        for period in periods {
            runs += period.runs;
            succeeded += period.succeeded;
            failed += period.failed;
            measured += period.measured_runs;
            llm_calls += period.llm_calls;
            input += period.input_tokens;
            output += period.output_tokens;
            run_ms.merge(&period.run_ms);
            call_ms.merge(&period.call_ms);
            ttft_ms.merge(&period.time_to_first_token_ms);
            for model in &period.models {
                let entry = models
                    .entry(model.model.clone())
                    .or_insert_with(|| ObservedModel {
                        model: model.model.clone(),
                        ..ObservedModel::default()
                    });
                entry.calls += model.calls;
                entry.input_tokens += model.input_tokens;
                entry.output_tokens += model.output_tokens;
                entry.cached_tokens += model.cached_tokens;
            }
            if let Some(at) = period.first_seen_at.and_then(seconds) {
                first = Some(first.map_or(at, |seen| seen.min(at)));
            }
            if let Some(at) = period.last_seen_at.and_then(seconds) {
                last = Some(last.map_or(at, |seen| seen.max(at)));
            }
        }
        // Exact where nothing written is included; buckets once anything is,
        // with this fold's own durations counted into them.
        let summary = |exact: Vec<i64>, mut histogram: DurationHistogram| {
            if periods.is_empty() {
                return DurationSummary::exact(exact);
            }
            for value in exact {
                histogram.add(value);
            }
            histogram.summary()
        };
        let models: Vec<ObservedModel> = models.into_values().collect();
        let cost = prices.map(|table| cost_of(table, &models));
        VariantObservations {
            variant_id: variant_id.to_owned(),
            runs,
            succeeded,
            failed,
            running: self.running,
            measured_runs: measured,
            duration_ms: summary(self.run_ms, run_ms),
            call_ms: summary(self.call_ms, call_ms),
            time_to_first_token_ms: summary(self.ttft_ms, ttft_ms),
            llm_calls,
            input_tokens: input,
            output_tokens: output,
            models,
            cost,
            first_seen_at: first,
            last_seen_at: last,
            periods: periods.len(),
            runs_from_periods: periods.iter().map(|period| period.runs).sum(),
            incomplete_periods: periods.iter().filter(|period| !period.complete).count(),
        }
    }
}

fn cost_of(table: &ModelPrices, models: &[ObservedModel]) -> ObservedCost {
    let mut cost = ObservedCost {
        currency: table.currency.clone(),
        amount: 0.0,
        priced_calls: 0,
        unpriced_calls: 0,
        unpriced_models: Vec::new(),
        prices: Vec::new(),
    };
    for model in models {
        match table.get(&model.model) {
            Some(price) => {
                cost.amount +=
                    price.cost(model.input_tokens, model.output_tokens, model.cached_tokens);
                cost.priced_calls += model.calls;
                cost.prices.push(PriceUsed {
                    model: price.model.clone(),
                    source: price.source.clone(),
                    as_of: price.as_of.clone(),
                });
            }
            None => {
                cost.unpriced_calls += model.calls;
                cost.unpriced_models.push(model.model.clone());
            }
        }
    }
    cost
}

fn text<'a>(span: &'a CompletedSpan, key: &str) -> Option<&'a str> {
    span.attributes
        .iter()
        .find_map(|(name, value)| match value {
            AttrValue::Str(text) if name == key => Some(text.as_str()),
            _ => None,
        })
}

fn number(span: &CompletedSpan, key: &str) -> i64 {
    span.attributes
        .iter()
        .find_map(|(name, value)| match value {
            AttrValue::Int(number) if name == key => Some(*number),
            _ => None,
        })
        .unwrap_or(0)
}

/// Whether `ended` falls in one of these written periods.
fn written(periods: &[(i64, i64)], ended: Option<OffsetDateTime>) -> bool {
    ended.is_some_and(|at| {
        let at = at.unix_timestamp();
        periods.iter().any(|(from, to)| (*from..*to).contains(&at))
    })
}

/// One row per variant asked about, in the order asked, whether or not any
/// run named it — "never observed" is an answer, and an absent row would read
/// as a variant nobody asked about.
///
/// `written` are the periods whose record is included: a run that ended in one
/// is that record's, and the live fold leaves it out rather than count it
/// twice.
#[must_use]
pub fn compute<'a>(
    runs: impl IntoIterator<Item = &'a RunSummary>,
    spans: &HashMap<String, Vec<CompletedSpan>>,
    variant_ids: &[&str],
    window_seconds: Option<i64>,
    now: OffsetDateTime,
    periods: &[ObservedPeriod],
    prices: Option<&ModelPrices>,
) -> Vec<VariantObservations> {
    let since = crate::window::cutoff(window_seconds, now);
    let ranges: Vec<(i64, i64)> = {
        let mut ranges: Vec<(i64, i64)> = periods
            .iter()
            .map(|period| (period.from, period.to))
            .collect();
        ranges.sort_unstable();
        ranges.dedup();
        ranges
    };
    let mut rows: Vec<(&str, Accumulated)> = variant_ids
        .iter()
        .map(|variant_id| (*variant_id, Accumulated::default()))
        .collect();
    for run in runs {
        if since.is_some_and(|start| run.last_event_at < start) || written(&ranges, run.ended_at) {
            continue;
        }
        let Some((_, row)) = run
            .variant_id
            .as_deref()
            .and_then(|named| rows.iter_mut().find(|(variant, _)| *variant == named))
        else {
            continue;
        };
        row.add(run, spans.get(&run.run_id));
    }
    rows.into_iter()
        .map(|(variant_id, row)| {
            let own: Vec<&ObservedPeriod> = periods
                .iter()
                .filter(|period| period.variant_id == variant_id)
                .collect();
            row.observations(variant_id, &own, prices)
        })
        .collect()
}

/// Every variant's runs that ended in `[from, to)`, one record each for the
/// variants any run named.
#[must_use]
pub fn period<'a>(
    runs: impl IntoIterator<Item = &'a RunSummary>,
    spans: &HashMap<String, Vec<CompletedSpan>>,
    from: i64,
    to: i64,
    complete: bool,
) -> Vec<ObservedPeriod> {
    let mut rows: BTreeMap<&str, Accumulated> = BTreeMap::new();
    for run in runs {
        let (Some(variant_id), true) = (
            run.variant_id.as_deref(),
            written(&[(from, to)], run.ended_at),
        ) else {
            continue;
        };
        rows.entry(variant_id)
            .or_default()
            .add(run, spans.get(&run.run_id));
    }
    rows.into_iter()
        .map(|(variant_id, row)| row.period(variant_id, from, to, complete))
        .collect()
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use aiwatcher_core::attrs::aiwatcher as own;
    use aiwatcher_core::ports::{SpanEvent, SpanKind, SpanStatus, attr};
    use aiwatcher_core::prices::ModelPrice;
    use aiwatcher_core::{Checkpoint, SpanId, TraceId};

    use super::*;

    fn run(run_id: &str, variant: Option<&str>, status: RunStatus, took: i64) -> RunSummary {
        let started = datetime!(2026-09-13 10:00:00 UTC);
        let ended = started + time::Duration::milliseconds(took);
        RunSummary {
            run_id: run_id.to_owned(),
            conversation_id: None,
            trace_id: TraceId::derive(run_id),
            status,
            agents: Vec::new(),
            runtimes: Vec::new(),
            workflow: None,
            variant_id: variant.map(ToOwned::to_owned),
            evaluation_id: None,
            published_by: None,
            caller_run_id: None,
            workflow_topology: None,
            nodes_run: Vec::new(),
            node_steps: Vec::new(),
            started_at: started,
            last_event_at: ended,
            ended_at: (status != RunStatus::Running).then_some(ended),
            duration_ms: (status != RunStatus::Running).then_some(took),
            event_count: 4,
            llm_calls: 1,
            tool_calls: 0,
            input_tokens: 10,
            output_tokens: 3,
            cached_tokens: 0,
            error: None,
            last_checkpoint: Checkpoint::beginning(),
        }
    }

    fn call(run_id: &str, model: &str, took: i64, first_token: Option<i64>) -> CompletedSpan {
        let trace_id = TraceId::derive(run_id);
        let start = datetime!(2026-09-13 10:00:00 UTC);
        CompletedSpan {
            trace_id,
            span_id: SpanId::derive(trace_id, "llm"),
            parent_span_id: None,
            name: format!("chat {model}"),
            kind: SpanKind::Client,
            start,
            end: start + time::Duration::milliseconds(took),
            status: SpanStatus::Ok,
            attributes: vec![
                attr(own::run::ID, run_id),
                attr(genai::OPERATION_NAME, genai::operation::CHAT),
                attr(genai::REQUEST_MODEL, model),
                attr(genai::USAGE_INPUT_TOKENS, 1_000_i64),
                attr(genai::USAGE_OUTPUT_TOKENS, 100_i64),
            ],
            events: first_token
                .map(|at| SpanEvent {
                    name: "gen_ai.first_token".to_owned(),
                    at: start + time::Duration::milliseconds(at),
                    attributes: Vec::new(),
                })
                .into_iter()
                .collect(),
            links: Vec::new(),
        }
    }

    const NOW: OffsetDateTime = datetime!(2026-09-13 11:00:00 UTC);

    #[test]
    fn a_variant_s_runs_are_counted_and_its_finished_runs_ranked() {
        let runs: Vec<RunSummary> = (1..=10)
            .map(|at| {
                run(
                    &format!("r{at}"),
                    Some("v1"),
                    RunStatus::Succeeded,
                    at * 100,
                )
            })
            .chain([
                run("failed", Some("v1"), RunStatus::Failed, 50),
                run("going", Some("v1"), RunStatus::Running, 0),
                run("other", Some("v2"), RunStatus::Succeeded, 9_000),
                run("nobody", None, RunStatus::Succeeded, 9_000),
            ])
            .collect();

        let [row] = compute(&runs, &HashMap::new(), &["v1"], None, NOW, &[], None)
            .try_into()
            .expect("one row per variant asked about");

        assert_eq!(
            (row.runs, row.succeeded, row.failed, row.running),
            (12, 10, 1, 1)
        );
        assert_eq!(
            row.duration_ms,
            Some(DurationSummary {
                count: 11,
                p50: 500,
                p90: 900,
                p99: 1_000,
                max: 1_000,
                bucketed: false,
            })
        );
        assert_eq!(
            (row.llm_calls, row.input_tokens, row.output_tokens),
            (12, 120, 36)
        );
        assert!(row.cost.is_none(), "no table, no price");
    }

    #[test]
    fn a_run_made_for_a_measurement_is_counted_apart_and_in_no_figure() {
        let mut measured = run("benchmark", Some("v1"), RunStatus::Succeeded, 5_000);
        measured.evaluation_id = Some("answers-candidate".to_owned());
        let runs = [
            measured,
            run("served", Some("v1"), RunStatus::Succeeded, 200),
        ];

        let row = &compute(&runs, &HashMap::new(), &["v1"], None, NOW, &[], None)[0];

        assert_eq!((row.runs, row.measured_runs), (1, 1));
        assert_eq!(
            row.duration_ms.as_ref().map(|summary| summary.max),
            Some(200)
        );
        assert_eq!(row.input_tokens, 10);
    }

    #[test]
    fn a_variant_nothing_named_is_a_row_of_nothing_rather_than_no_row() {
        let rows = compute(
            &[run("other", Some("v2"), RunStatus::Succeeded, 10)],
            &HashMap::new(),
            &["v1", "v2"],
            None,
            NOW,
            &[],
            None,
        );

        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].runs, rows[0].duration_ms.is_none()), (0, true));
        assert!(rows[0].last_seen_at.is_none());
        assert_eq!(rows[1].runs, 1);
    }

    #[test]
    fn a_run_outside_the_window_is_not_observed() {
        let rows = compute(
            &[run("old", Some("v1"), RunStatus::Succeeded, 10)],
            &HashMap::new(),
            &["v1"],
            Some(60),
            NOW,
            &[],
            None,
        );

        assert_eq!(rows[0].runs, 0);
    }

    #[test]
    fn each_call_is_timed_from_its_span_and_priced_where_the_table_prices_its_model() {
        let runs = [
            run("a", Some("v1"), RunStatus::Succeeded, 900),
            run("b", Some("v1"), RunStatus::Succeeded, 700),
        ];
        let spans = HashMap::from([
            ("a".to_owned(), vec![call("a", "gpt-4o", 400, Some(120))]),
            ("b".to_owned(), vec![call("b", "local-llama", 300, None)]),
        ]);
        let prices = ModelPrices {
            currency: "USD".into(),
            prices: vec![ModelPrice {
                model: "gpt-4o".into(),
                input_per_million: 2.5,
                output_per_million: 10.0,
                cached_input_per_million: None,
                source: "https://openai.com/api/pricing".into(),
                as_of: "2026-09-01".into(),
            }],
        };

        let row = &compute(&runs, &spans, &["v1"], None, NOW, &[], Some(&prices))[0];

        assert_eq!(
            row.call_ms.as_ref().map(|calls| (calls.count, calls.max)),
            Some((2, 400))
        );
        assert_eq!(
            row.time_to_first_token_ms.as_ref().map(|first| first.p50),
            Some(120)
        );
        let cost = row.cost.as_ref().expect("a table was loaded");
        assert!((cost.amount - (1_000.0 * 2.5 + 100.0 * 10.0) / 1e6).abs() < 1e-12);
        assert_eq!((cost.priced_calls, cost.unpriced_calls), (1, 1));
        assert_eq!(cost.unpriced_models, ["local-llama"]);
        assert_eq!(cost.prices[0].as_of, "2026-09-01");
    }

    #[test]
    fn a_written_period_is_counted_once_and_its_runs_are_left_out_of_the_live_fold() {
        let runs: Vec<RunSummary> = (1..=4)
            .map(|at| {
                run(
                    &format!("r{at}"),
                    Some("v1"),
                    RunStatus::Succeeded,
                    at * 100,
                )
            })
            .collect();
        let from = datetime!(2026-09-13 10:00:00 UTC).unix_timestamp();
        let written = period(&runs[..2], &HashMap::new(), from, from + 3_600, true);
        assert_eq!(written.len(), 1);
        // Two more runs ended in the same period, which the record holds, and
        // the read model since lost the first two.
        let [row] = compute(
            &runs[2..],
            &HashMap::new(),
            &["v1"],
            Some(7_200),
            NOW,
            &written,
            None,
        )
        .try_into()
        .expect("one row");

        assert_eq!(
            row.runs, 2,
            "only what the record holds: the rest ended in it"
        );
        assert_eq!(row.runs_from_periods, 2);
        assert_eq!((row.periods, row.incomplete_periods), (1, 0));

        let later = datetime!(2026-09-13 12:00:00 UTC);
        let mut after = run("r9", Some("v1"), RunStatus::Succeeded, 250);
        after.started_at = later;
        after.last_event_at = later + time::Duration::milliseconds(250);
        after.ended_at = Some(after.last_event_at);
        let [row] = compute(
            [&after],
            &HashMap::new(),
            &["v1"],
            Some(10_800),
            later + time::Duration::minutes(1),
            &written,
            None,
        )
        .try_into()
        .expect("one row");
        assert_eq!(row.runs, 3, "the record's two and the live one");
        let duration = row.duration_ms.expect("finished runs");
        assert!(duration.bucketed);
        assert_eq!(duration.count, 3);
        assert!(duration.max == 250 && duration.p50 >= 200 && duration.p50 <= 250);
    }

    #[test]
    fn a_histogram_s_percentile_is_within_a_bucket_and_never_above_the_largest() {
        let mut histogram = DurationHistogram::default();
        for value in 1..=1_000 {
            histogram.add(value);
        }
        let summary = histogram.summary().expect("counted");
        for (exact, bucketed) in [(500, summary.p50), (900, summary.p90), (990, summary.p99)] {
            assert!(
                bucketed >= exact && (bucketed as f64) <= exact as f64 * 1.1 + 1.0,
                "{exact} → {bucketed}"
            );
        }
        assert_eq!(summary.max, 1_000);
        let mut other = DurationHistogram::default();
        other.add(5);
        histogram.merge(&other);
        assert_eq!(histogram.count, 1_001);
    }
}
