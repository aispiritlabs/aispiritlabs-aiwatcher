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
//! Two answers, never mixed. Without a window, the read model's, like
//! [`crate::dimensions`]: every run it holds, timed exactly, and bounded by
//! what it holds. With a window, the period fold's ([`crate::period_fold`]),
//! wherever there is a store to write periods to: every period the window
//! reaches into, whole, from the records written as the log passed them and
//! from the fold's own memory past those — one source, so a run that ended
//! late or was evicted from the read model is counted once. A period keeps its
//! durations as a histogram, so periods add up and still answer a percentile,
//! within one bucket.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use aiwatcher_core::attrs::genai;
use aiwatcher_core::ports::{AttrValue, CompletedSpan};
use aiwatcher_core::prices::{ModelPrices, ModelUsage, TokenCost, day_of};

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
    pub models: Vec<ModelUsage>,
    /// What those calls cost at the deployment's price table. Absent when the
    /// deployment loaded none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<TokenCost>,
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
    /// Written periods these figures include. Nought without a window, which
    /// the read model answers.
    pub periods: usize,
    /// Of `runs`, those counted from written periods rather than from the
    /// periods the fold has not written yet.
    pub runs_from_periods: u64,
    /// Of the periods counted, written or not, those whose fold could not
    /// vouch it held every run that ended in them.
    pub incomplete_periods: usize,
    /// Of `runs`, those whose end reached the log after the period they ended
    /// in had closed, counted in the period that was open when it did.
    #[serde(default)]
    pub late_runs: u64,
    /// Where a window's counting starts: the beginning of the period its start
    /// falls in, which may be before it. Absent without a window.
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub counted_from: Option<OffsetDateTime>,
    /// The window reaches back before the fold began observing, so nothing
    /// before `counted_from` could be counted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub window_before_observations: bool,
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

/// One variant's runs that ended in one period, as the fold counted them — the
/// record written when the period closed, which outlives the read model.
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
    pub models: Vec<ModelUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_seen_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<i64>,
    /// Runs counted here whose end was dated in a period already written —
    /// an end the log received after that period closed.
    #[serde(default)]
    pub late_runs: u64,
    /// Whether the fold saw every run counted here from its start: one it
    /// did not is counted with no duration.
    pub complete: bool,
}

impl ObservedPeriod {
    /// Add another period's runs of the same variant to this one, as a period
    /// inside an hour adds up to the hour.
    pub fn merge(&mut self, other: &Self) {
        self.runs += other.runs;
        self.succeeded += other.succeeded;
        self.failed += other.failed;
        self.measured_runs += other.measured_runs;
        self.run_ms.merge(&other.run_ms);
        self.call_ms.merge(&other.call_ms);
        self.time_to_first_token_ms
            .merge(&other.time_to_first_token_ms);
        self.llm_calls += other.llm_calls;
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        for model in &other.models {
            ModelUsage::add_to(&mut self.models, model);
        }
        let earliest = |one: Option<i64>, other: Option<i64>| match (one, other) {
            (Some(one), Some(other)) => Some(one.min(other)),
            (one, other) => one.or(other),
        };
        let latest = |one: Option<i64>, other: Option<i64>| match (one, other) {
            (Some(one), Some(other)) => Some(one.max(other)),
            (one, other) => one.or(other),
        };
        self.first_seen_at = earliest(self.first_seen_at, other.first_seen_at);
        self.last_seen_at = latest(self.last_seen_at, other.last_seen_at);
        self.late_runs += other.late_runs;
        self.complete &= other.complete;
    }
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
    models: BTreeMap<String, ModelUsage>,
    /// The same calls by the day their run started, which is what they are
    /// priced on.
    by_day: BTreeMap<String, Vec<ModelUsage>>,
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
            let usage = ModelUsage {
                model: text(span, genai::REQUEST_MODEL)
                    .unwrap_or("unknown")
                    .to_owned(),
                calls: 1,
                input_tokens: number(span, genai::USAGE_INPUT_TOKENS),
                output_tokens: number(span, genai::USAGE_OUTPUT_TOKENS),
                cached_tokens: number(span, "gen_ai.usage.cached_tokens"),
            };
            let entry = self
                .models
                .entry(usage.model.clone())
                .or_insert_with(|| ModelUsage {
                    model: usage.model.clone(),
                    ..ModelUsage::default()
                });
            entry.calls += 1;
            entry.input_tokens += usage.input_tokens;
            entry.output_tokens += usage.output_tokens;
            entry.cached_tokens += usage.cached_tokens;
            ModelUsage::add_to(
                self.by_day
                    .entry(day_of(run.started_at.unix_timestamp()))
                    .or_default(),
                &usage,
            );
        }
    }

    /// The figures a reader sees.
    fn observations(self, variant_id: &str, prices: Option<&ModelPrices>) -> VariantObservations {
        let models: Vec<ModelUsage> = self.models.into_values().collect();
        let cost = prices.map(|table| {
            table.cost_by_day(
                self.by_day
                    .iter()
                    .map(|(day, usage)| (day.as_str(), usage.as_slice())),
            )
        });
        VariantObservations {
            variant_id: variant_id.to_owned(),
            runs: self.runs,
            succeeded: self.succeeded,
            failed: self.failed,
            running: self.running,
            measured_runs: self.measured_runs,
            duration_ms: DurationSummary::exact(self.run_ms),
            call_ms: DurationSummary::exact(self.call_ms),
            time_to_first_token_ms: DurationSummary::exact(self.ttft_ms),
            llm_calls: self.llm_calls,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            models,
            cost,
            first_seen_at: self.first_seen_at,
            last_seen_at: self.last_seen_at,
            periods: 0,
            runs_from_periods: 0,
            incomplete_periods: 0,
            late_runs: 0,
            counted_from: None,
            window_before_observations: false,
        }
    }
}

/// One variant's figures over a window, from the periods it reaches into:
/// those written, and those the fold has not written yet — each counted once,
/// because a period is in one or the other — with the runs in flight.
#[must_use]
pub fn from_periods(
    variant_id: &str,
    written: &[ObservedPeriod],
    unwritten: &[ObservedPeriod],
    running: u64,
    counted_from: Option<i64>,
    window_before_observations: bool,
    prices: Option<&ModelPrices>,
) -> VariantObservations {
    let mut total = ObservedPeriod {
        variant_id: variant_id.to_owned(),
        complete: true,
        ..ObservedPeriod::default()
    };
    let mut by_day: BTreeMap<String, Vec<ModelUsage>> = BTreeMap::new();
    for period in written.iter().chain(unwritten) {
        total.merge(period);
        let day = by_day.entry(day_of(period.from)).or_default();
        for model in &period.models {
            ModelUsage::add_to(day, model);
        }
    }
    let seconds = |at: Option<i64>| at.and_then(|at| OffsetDateTime::from_unix_timestamp(at).ok());
    VariantObservations {
        variant_id: variant_id.to_owned(),
        runs: total.runs,
        succeeded: total.succeeded,
        failed: total.failed,
        running,
        measured_runs: total.measured_runs,
        duration_ms: total.run_ms.summary(),
        call_ms: total.call_ms.summary(),
        time_to_first_token_ms: total.time_to_first_token_ms.summary(),
        llm_calls: total.llm_calls,
        input_tokens: total.input_tokens,
        output_tokens: total.output_tokens,
        cost: prices.map(|table| {
            table.cost_by_day(
                by_day
                    .iter()
                    .map(|(day, usage)| (day.as_str(), usage.as_slice())),
            )
        }),
        models: total.models,
        first_seen_at: seconds(total.first_seen_at),
        last_seen_at: seconds(total.last_seen_at),
        periods: written.len(),
        runs_from_periods: written.iter().map(|period| period.runs).sum(),
        incomplete_periods: written
            .iter()
            .chain(unwritten)
            .filter(|period| !period.complete)
            .count(),
        late_runs: total.late_runs,
        counted_from: seconds(counted_from),
        window_before_observations,
    }
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

/// One row per variant asked about, in the order asked, whether or not any
/// run named it — "never observed" is an answer, and an absent row would read
/// as a variant nobody asked about.
#[must_use]
pub fn compute<'a>(
    runs: impl IntoIterator<Item = &'a RunSummary>,
    spans: &HashMap<String, Vec<CompletedSpan>>,
    variant_ids: &[&str],
    window_seconds: Option<i64>,
    now: OffsetDateTime,
    prices: Option<&ModelPrices>,
) -> Vec<VariantObservations> {
    let since = crate::window::cutoff(window_seconds, now);
    let mut rows: Vec<(&str, Accumulated)> = variant_ids
        .iter()
        .map(|variant_id| (*variant_id, Accumulated::default()))
        .collect();
    for run in runs {
        if since.is_some_and(|start| run.last_event_at < start) {
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
        .map(|(variant_id, row)| row.observations(variant_id, prices))
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
            node_steps_dropped: false,
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

        let [row] = compute(&runs, &HashMap::new(), &["v1"], None, NOW, None)
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

        let row = &compute(&runs, &HashMap::new(), &["v1"], None, NOW, None)[0];

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

        let row = &compute(&runs, &spans, &["v1"], None, NOW, Some(&prices))[0];

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
    fn a_window_s_periods_add_up_written_or_not_and_each_is_priced_on_its_day() {
        let day = datetime!(2026-09-13 00:00:00 UTC).unix_timestamp();
        let period = |from: i64, runs: u64, took: &[i64], complete: bool| {
            let mut record = ObservedPeriod {
                variant_id: "v1".to_owned(),
                from,
                to: from + 3_600,
                runs,
                succeeded: runs,
                llm_calls: runs,
                models: vec![ModelUsage {
                    model: "gpt-4o".to_owned(),
                    calls: runs,
                    input_tokens: 1_000_000 * i64::try_from(runs).unwrap(),
                    output_tokens: 0,
                    cached_tokens: 0,
                }],
                late_runs: u64::from(!complete),
                complete,
                ..ObservedPeriod::default()
            };
            for ms in took {
                record.run_ms.add(*ms);
            }
            record
        };
        let written = [period(day - 3_600, 2, &[100, 200], true)];
        let unwritten = [period(day, 1, &[250], false)];
        let prices = ModelPrices {
            currency: "USD".into(),
            prices: [("2026-09-01", 1.0), ("2026-09-13", 2.0)]
                .into_iter()
                .map(|(as_of, input)| ModelPrice {
                    model: "gpt-4o".into(),
                    input_per_million: input,
                    output_per_million: 0.0,
                    cached_input_per_million: None,
                    source: "https://openai.com/api/pricing".into(),
                    as_of: as_of.into(),
                })
                .collect(),
        };

        let row = from_periods(
            "v1",
            &written,
            &unwritten,
            4,
            Some(day - 3_600),
            false,
            Some(&prices),
        );

        assert_eq!((row.runs, row.running, row.late_runs), (3, 4, 1));
        assert_eq!(
            (row.periods, row.runs_from_periods, row.incomplete_periods),
            (1, 2, 1)
        );
        let duration = row.duration_ms.expect("finished runs");
        assert!(duration.bucketed && duration.count == 3 && duration.max == 250);
        let cost = row.cost.expect("a table");
        assert!(
            (cost.amount - (2.0 * 1.0 + 1.0 * 2.0)).abs() < 1e-9,
            "the day before at the first price, the day itself at the second"
        );
        assert_eq!(
            row.counted_from.map(OffsetDateTime::unix_timestamp),
            Some(day - 3_600)
        );
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
