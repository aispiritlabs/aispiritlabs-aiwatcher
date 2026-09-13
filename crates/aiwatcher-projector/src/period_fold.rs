//! What variants were observed doing, folded period by period as the log is
//! read, with a state of its own that survives a restart — and the one answer a
//! window over them gets (ADR_0030, amended).
//!
//! A projection like the others: it reads each event once, in log order, keeps
//! the runs in flight and the periods still open, and writes a period when the
//! log's clock — the latest `min(occurred_at, ingested_at)`, so a skewed
//! producer closes nothing early — has passed it, rolled up into the hour and
//! the day it lies in. A run is counted in the period it ended in: one whose
//! end reaches the log after that period closed is held, by that period, in the
//! oldest one still open (`late`), and each period keeps its runs by the second
//! they ended in, so a window starting inside a period counts what ended from
//! its start on, whatever the period's width.
//! Its state is saved with the position it was folded through; a restart loads
//! the one furthest along — or, with none left, starts again from the last
//! period written — and skips what it holds. On a log that numbers every event,
//! a position the log no longer holds when the fold comes to it — retention
//! passed it — is written down with the span of time it may have lain in, and
//! the periods that span reaches say they are incomplete. A width configured
//! anew takes over at the next hour, so periods of two widths never overlap.
//!
//! A window is answered here and nowhere else ([`PeriodOutput::observe`]):
//! the periods it reaches into — written ones from the store, the rest from
//! this fold — and the runs in flight. One source, so no run is counted twice
//! or missed between two.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use futures::{StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use aiwatcher_core::ports::PortError;
use aiwatcher_core::prices::{ModelPrices, ModelUsage};
use aiwatcher_core::{Phase, RecordedEvent, Subject};

use crate::observations::{Counted, CountedPart, ObservedPeriod, VariantObservations};
use crate::periods::{FoldAt, LogGap, PeriodStore};

/// Runs in flight past this are not tracked; each is counted at its end with
/// no duration, in a period that says it is incomplete.
const MOST_IN_FLIGHT: usize = 50_000;
/// Call durations a run keeps; its call count goes on past it.
const MOST_CALLS_TIMED: usize = 1_000;
/// How often the state is saved when nothing forces it.
const SAVE_EVERY: Duration = Duration::from_secs(15);
/// Periods read from the store at once when a window is answered.
const READ_AT_ONCE: usize = 16;
const HOUR: i64 = 3_600;
const DAY: i64 = 86_400;

/// A model call a tracked run has open.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct OpenCall {
    started_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    first_token_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
}

/// A run naming a variant, while it is in flight.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct InFlight {
    variant_id: String,
    measured: bool,
    saw_start: bool,
    started_ms: i64,
    /// When it was last heard from, which a window's running count reads.
    #[serde(default)]
    last_ms: i64,
    llm_calls: u64,
    input_tokens: i64,
    output_tokens: i64,
    models: Vec<ModelUsage>,
    /// The same calls by the day each ended on, which is what prices them.
    #[serde(default)]
    models_by_day: BTreeMap<String, Vec<ModelUsage>>,
    call_ms: Vec<i64>,
    ttft_ms: Vec<i64>,
    calls: BTreeMap<String, OpenCall>,
}

/// One closed period's records, waiting to be written.
#[derive(Clone, Debug, PartialEq)]
pub struct ClosedPeriod {
    /// How wide it is: a period's own width, an hour or a day.
    pub level: i64,
    pub from: i64,
    pub records: Vec<ObservedPeriod>,
    /// The position the fold had read through when it closed.
    pub through: Option<u64>,
}

/// The fold itself: pure, and what is saved.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PeriodFold {
    /// The width of periods from each moment on. A width configured anew takes
    /// over at the next hour, which every width divides.
    #[serde(default)]
    widths: BTreeMap<i64, i64>,
    /// The one width a state saved before `widths` had.
    #[serde(default, rename = "width", skip_serializing)]
    saved_width: i64,
    /// The global position of the last event folded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    through: Option<u64>,
    /// The log's clock, in Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    clock: Option<i64>,
    /// The start of the first period this fold covers: it saw nothing before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    began: Option<i64>,
    /// The end of the last period closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    closed_through: Option<i64>,
    /// Where a fold started again from its written periods resumed: the runs
    /// that ended from there before it are not all in them, so the period
    /// starting there says it is incomplete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    gap_from: Option<i64>,
    runs: BTreeMap<String, InFlight>,
    /// Periods at their own width still open, by their start.
    open: BTreeMap<i64, BTreeMap<String, ObservedPeriod>>,
    /// Hours and days still open, by level and then start: what the closed
    /// periods inside each have added up to so far.
    #[serde(default)]
    rollups: BTreeMap<i64, BTreeMap<i64, BTreeMap<String, ObservedPeriod>>>,
    /// Closed and not yet written; never saved, because a state is saved only
    /// once these are written.
    #[serde(skip)]
    closed: Vec<ClosedPeriod>,
    /// Positions the log no longer held when the fold came to them, not yet
    /// written down.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    missed: Vec<LogGap>,
    /// The spans of time those positions may have lain in, `(from, until)`,
    /// while a period they reach may still be counted into: a run placed in
    /// one of those periods says it is incomplete.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    missed_spans: Vec<(i64, i64)>,
    /// Whether the log numbers every event one after the last, so a jump in
    /// positions is events it no longer holds. The log's to say, never saved.
    #[serde(skip)]
    contiguous: bool,
}

fn millis(at: time::OffsetDateTime) -> i64 {
    i64::try_from(at.unix_timestamp_nanos() / 1_000_000).unwrap_or(i64::MAX)
}

/// A record of nothing yet, for `[from, to)`.
fn empty(variant_id: &str, from: i64, to: i64) -> ObservedPeriod {
    ObservedPeriod {
        variant_id: variant_id.to_owned(),
        from,
        to,
        complete: true,
        ..ObservedPeriod::default()
    }
}

impl PeriodFold {
    /// A fold of periods `width` seconds wide, each closing a tenth of that
    /// after its end, at most five minutes. A width is a whole part of an
    /// hour, which is what lets its periods add up to hours and days; one that
    /// is not gets no rollups.
    #[must_use]
    pub fn new(width: i64) -> Self {
        Self {
            widths: BTreeMap::from([(i64::MIN, width.max(1))]),
            ..Self::default()
        }
    }

    /// A state saved before widths had a history reads as one width for ever.
    fn settled(mut self) -> Self {
        if self.widths.is_empty() {
            self.widths.insert(i64::MIN, self.saved_width.max(1));
        }
        self
    }

    fn width_at(&self, at: i64) -> i64 {
        self.widths
            .range(..=at)
            .next_back()
            .or_else(|| self.widths.iter().next())
            .map_or(1, |(_, width)| *width)
    }

    /// The width the fold was last configured with.
    fn width_now(&self) -> i64 {
        self.widths.values().next_back().copied().unwrap_or(1)
    }

    fn grace_at(&self, at: i64) -> i64 {
        (self.width_at(at) / 10).clamp(1, 300)
    }

    /// The start of the period `at` falls in.
    fn floor_at(&self, at: i64) -> i64 {
        let width = self.width_at(at);
        at.div_euclid(width) * width
    }

    fn levels_at(&self, at: i64) -> Vec<i64> {
        let width = self.width_at(at);
        let mut levels = vec![width];
        if HOUR % width == 0 {
            levels.extend([HOUR, DAY].into_iter().filter(|level| *level > width));
        }
        levels
    }

    /// The widths periods are written at now, narrowest first.
    #[must_use]
    pub fn levels(&self) -> Vec<i64> {
        self.levels_at(i64::MAX)
    }

    /// The position this fold has read through.
    #[must_use]
    pub fn through(&self) -> Option<u64> {
        self.through
    }

    /// Periods `width` wide from the next hour this fold has not closed or
    /// opened a period in; before it, the widths it had.
    pub fn change_width(&mut self, width: i64) {
        if width == self.width_now() {
            return;
        }
        let Some(began) = self.began else {
            self.widths = BTreeMap::from([(i64::MIN, width)]);
            return;
        };
        let reached = self
            .open
            .keys()
            .map(|from| from + self.width_at(*from))
            .chain(self.closed_through)
            .chain(self.clock)
            .fold(began, i64::max);
        let switch = (reached + HOUR - 1).div_euclid(HOUR) * HOUR;
        self.widths.retain(|from, _| *from < switch);
        self.widths.insert(switch, width);
    }

    /// Read positions as a log that numbers every event one after the last.
    pub fn reads_contiguous_positions(&mut self, contiguous: bool) {
        self.contiguous = contiguous;
    }

    /// Fold one event in, once: an event at or before `through` is skipped.
    pub fn apply(&mut self, event: &RecordedEvent) {
        let position = event.metadata.global_position;
        if self.through.is_some_and(|through| position <= through) {
            return;
        }
        let bound = event
            .metadata
            .occurred_at
            .min(event.metadata.ingested_at)
            .unix_timestamp();
        if let Some(through) = self
            .through
            .filter(|through| self.contiguous && position > through + 1)
        {
            self.missed_before(through, position, bound);
        }
        self.through = Some(position);
        if self.began.is_none() {
            self.began = Some(self.floor_at(bound));
        }
        let subject = event.event_type.subject();
        if subject != Subject::Eval {
            self.fold(event, subject);
        }
        self.clock = Some(self.clock.map_or(bound, |clock| clock.max(bound)));
        self.close();
    }

    /// The log gave `position` after `through`, and holds nothing between:
    /// what lay there may have ended runs anywhere from the log's clock before
    /// it to `bound`, so each period that span reaches — open now, or opened by
    /// a run placed there later — says it is incomplete.
    fn missed_before(&mut self, through: u64, position: u64, bound: i64) {
        let from = self
            .clock
            .or(self.closed_through)
            .or(self.began)
            .unwrap_or(bound)
            .min(bound);
        self.missed.push(LogGap {
            first_position: through + 1,
            last_position: position - 1,
            from,
            until: bound,
        });
        self.missed_spans.push((from, bound));
        let reached: Vec<i64> = self
            .open
            .keys()
            .copied()
            .filter(|start| *start <= bound && from < start + self.width_at(*start))
            .collect();
        for start in reached {
            for record in self
                .open
                .get_mut(&start)
                .into_iter()
                .flat_map(|v| v.values_mut())
            {
                record.complete = false;
            }
        }
        for (level, open) in &mut self.rollups {
            for (start, variants) in open.range_mut(..=bound) {
                if from < start + level {
                    for record in variants.values_mut() {
                        record.complete = false;
                    }
                }
            }
        }
    }

    /// Whether a period `[from, to)` reaches a span the log no longer held.
    fn reaches_missed(&self, from: i64, to: i64) -> bool {
        self.missed_spans
            .iter()
            .any(|(gap_from, until)| *gap_from < to && from <= *until)
    }

    fn fold(&mut self, event: &RecordedEvent, subject: Subject) {
        let at = millis(event.metadata.occurred_at);
        let phase = event.event_type.phase();
        let run_id = event.metadata.run_id.as_str();
        let ends = matches!(subject, Subject::Run | Subject::Execution)
            && matches!(phase, Some(Phase::End { .. }));

        if !self.runs.contains_key(run_id)
            && !ends
            && self.runs.len() < MOST_IN_FLIGHT
            && let Some(variant_id) = &event.metadata.variant_id
        {
            self.runs.insert(
                run_id.to_owned(),
                InFlight {
                    variant_id: variant_id.clone(),
                    started_ms: at,
                    ..InFlight::default()
                },
            );
        }
        if let Some(run) = self.runs.get_mut(run_id) {
            run.started_ms = run.started_ms.min(at);
            run.last_ms = run.last_ms.max(at);
            if subject == Subject::Run && phase == Some(Phase::Start) {
                run.saw_start = true;
                run.measured |= event.data_str("evaluation_id").is_some();
            }
            if subject == Subject::Llm {
                Self::call(run, event, at);
            }
        }
        if let Some(Phase::End { ok }) = phase
            && ends
        {
            let end_seconds = event.metadata.occurred_at.unix_timestamp();
            match self.runs.remove(run_id) {
                Some(run) => self.finish(run, ok, at, end_seconds),
                None => {
                    if let Some(variant_id) = &event.metadata.variant_id {
                        self.finish_unseen(variant_id, ok, end_seconds);
                    }
                }
            }
        }
    }

    fn call(run: &mut InFlight, event: &RecordedEvent, at: i64) {
        let key = event.metadata.span_key.clone();
        match event.event_type.phase() {
            Some(Phase::Start) => {
                run.llm_calls += 1;
                run.calls.insert(
                    key,
                    OpenCall {
                        started_ms: at,
                        first_token_ms: None,
                        model: event.data_str("model").map(ToOwned::to_owned),
                    },
                );
            }
            Some(Phase::Point) if event.event_type.as_str() == "llm.first_token" => {
                if let Some(call) = run.calls.get_mut(&key) {
                    call.first_token_ms.get_or_insert(at);
                }
            }
            Some(Phase::End { ok }) => {
                let open = run.calls.remove(&key);
                let started = open.as_ref().map_or(at, |call| call.started_ms);
                let took = event
                    .data_f64("duration_ms")
                    .filter(|ms| ms.is_finite() && *ms >= 0.0)
                    .map_or(at - started, |ms| ms as i64);
                if run.call_ms.len() < MOST_CALLS_TIMED {
                    run.call_ms.push(took.max(0));
                    if let Some(first) = open.as_ref().and_then(|call| call.first_token_ms) {
                        run.ttft_ms.push((first - started).max(0));
                    }
                }
                let model = event
                    .data_str("model")
                    .map(ToOwned::to_owned)
                    .or_else(|| open.and_then(|call| call.model))
                    .unwrap_or_else(|| "unknown".to_owned());
                let (input, output, cached) = if ok {
                    (
                        event
                            .data_i64("prompt_tokens")
                            .or_else(|| event.data_i64("input_tokens"))
                            .unwrap_or(0),
                        event
                            .data_i64("completion_tokens")
                            .or_else(|| event.data_i64("output_tokens"))
                            .unwrap_or(0),
                        event.data_i64("cached_tokens").unwrap_or(0),
                    )
                } else {
                    (0, 0, 0)
                };
                run.input_tokens += input;
                run.output_tokens += output;
                let usage = ModelUsage {
                    model,
                    calls: 1,
                    input_tokens: input,
                    output_tokens: output,
                    cached_tokens: cached,
                };
                ModelUsage::add_to(
                    run.models_by_day
                        .entry(aiwatcher_core::prices::day_of(at.div_euclid(1_000)))
                        .or_default(),
                    &usage,
                );
                ModelUsage::add_to(&mut run.models, &usage);
            }
            _ => {}
        }
    }

    /// Count one run's figures in the period it ended in and in its second —
    /// held in the oldest open period, by the one it ended in, when that one
    /// has closed.
    fn place(&mut self, variant_id: &str, end_seconds: i64, run: &ObservedPeriod) {
        let ended_in = self.floor_at(end_seconds);
        let open_from = ended_in
            .max(self.closed_through.unwrap_or(ended_in))
            .max(self.began.unwrap_or(ended_in));
        let ended_width = self.width_at(ended_in);
        let open_width = self.width_at(open_from);
        let offset = end_seconds - ended_in;
        let missed = self.reaches_missed(ended_in, ended_in + ended_width)
            || self.reaches_missed(open_from, open_from + open_width);
        let record = self
            .open
            .entry(open_from)
            .or_default()
            .entry(variant_id.to_owned())
            .or_insert_with(|| empty(variant_id, open_from, open_from + open_width));
        if missed
            || self
                .gap_from
                .is_some_and(|gap| gap == ended_in || gap == open_from)
        {
            record.complete = false;
        }
        let counted = if open_from > ended_in {
            record.late_runs += run.runs;
            record
                .late
                .entry(ended_in)
                .or_insert_with(|| empty("", ended_in, ended_in + ended_width))
        } else {
            record
        };
        counted.merge(run);
        counted
            .slices
            .entry(u32::try_from(offset).unwrap_or(u32::MAX))
            .or_insert_with(|| empty("", ended_in + offset, ended_in + offset + 1))
            .merge(run);
    }

    fn finish(&mut self, run: InFlight, ok: bool, at: i64, end_seconds: i64) {
        let mut counted = empty("", 0, 0);
        if run.measured {
            counted.measured_runs = 1;
        } else {
            counted.runs = 1;
            counted.succeeded = u64::from(ok);
            counted.failed = u64::from(!ok);
            if run.saw_start {
                counted.run_ms.add((at - run.started_ms).max(0));
            } else {
                counted.complete = false;
            }
            for took in &run.call_ms {
                counted.call_ms.add(*took);
            }
            for first in &run.ttft_ms {
                counted.time_to_first_token_ms.add(*first);
            }
            counted.llm_calls = run.llm_calls;
            counted.input_tokens = run.input_tokens;
            counted.output_tokens = run.output_tokens;
            counted.models.clone_from(&run.models);
            counted.models_by_day.clone_from(&run.models_by_day);
            counted.first_seen_at = Some(run.started_ms.div_euclid(1_000));
            counted.last_seen_at = Some(end_seconds);
        }
        self.place(&run.variant_id, end_seconds, &counted);
    }

    /// A run naming a variant whose start this fold never tracked: counted,
    /// with no duration, and its period incomplete.
    fn finish_unseen(&mut self, variant_id: &str, ok: bool, end_seconds: i64) {
        let counted = ObservedPeriod {
            runs: 1,
            succeeded: u64::from(ok),
            failed: u64::from(!ok),
            last_seen_at: Some(end_seconds),
            complete: false,
            ..ObservedPeriod::default()
        };
        self.place(variant_id, end_seconds, &counted);
    }

    /// Close every period the log's clock has passed, then every hour and day
    /// whose last period that was. A period nothing ended in closes as nothing
    /// and is not written; a rollup that began before the fold did is never
    /// kept, since it would hold only part of its span. A rollup adds up its
    /// periods' figures and their late runs, and not their slices.
    fn close(&mut self) {
        let (Some(clock), Some(began)) = (self.clock, self.began) else {
            return;
        };
        let edge = self.floor_at(clock - self.grace_at(clock));
        if edge <= self.closed_through.unwrap_or(began) {
            return;
        }
        let closing: Vec<i64> = self
            .open
            .range(..edge)
            .map(|(from, _)| *from)
            .filter(|from| from + self.width_at(*from) <= edge)
            .collect();
        for from in closing {
            let Some(variants) = self.open.remove(&from) else {
                continue;
            };
            let levels = self.levels_at(from);
            for level in &levels[1..] {
                let start = from.div_euclid(*level) * level;
                if start < began {
                    continue;
                }
                let rollup = self
                    .rollups
                    .entry(*level)
                    .or_default()
                    .entry(start)
                    .or_default();
                for record in variants.values() {
                    let into = rollup
                        .entry(record.variant_id.clone())
                        .or_insert_with(|| empty(&record.variant_id, start, start + level));
                    into.merge(record);
                    into.merge_late(record);
                }
            }
            self.closed.push(ClosedPeriod {
                level: levels[0],
                from,
                records: variants.into_values().collect(),
                through: self.through,
            });
        }
        for (level, open) in &mut self.rollups {
            let ended: Vec<i64> = open
                .range(..edge)
                .map(|(from, _)| *from)
                .filter(|from| from + level <= edge)
                .collect();
            for from in ended {
                if let Some(variants) = open.remove(&from) {
                    self.closed.push(ClosedPeriod {
                        level: *level,
                        from,
                        records: variants.into_values().collect(),
                        through: self.through,
                    });
                }
            }
        }
        self.closed_through = Some(edge);
        // A late run may still be placed in a period a day behind the edge.
        self.missed_spans.retain(|(_, until)| until + DAY >= edge);
    }

    /// Take what is closed and waiting to be written.
    pub fn take_closed(&mut self) -> Vec<ClosedPeriod> {
        std::mem::take(&mut self.closed)
    }

    /// Forget a closed period once it is written: until then a window reads it
    /// from here rather than from a store that may not hold it yet.
    fn written(&mut self, level: i64, from: i64) {
        if let Some(at) = self
            .closed
            .iter()
            .position(|period| period.level == level && period.from == from)
        {
            self.closed.remove(at);
        }
    }

    /// Forget a gap once it is written down.
    fn gap_written(&mut self, gap: &LogGap) {
        self.missed.retain(|held| held != gap);
    }

    /// Where the fold was, for a marker to say.
    fn at(&self, through: Option<u64>) -> FoldAt {
        FoldAt {
            through,
            began: self.began,
            widths: self.widths.clone(),
        }
    }

    /// Whether a closed period is one of the fold's own periods rather than an
    /// hour or a day that adds them up.
    fn is_base(&self, period: &ClosedPeriod) -> bool {
        period.level == self.width_at(period.from)
    }

    /// Where the store holds every period of the fold's own widths: before the
    /// first still waiting to be written, and never past what has closed.
    fn base_written(&self) -> Option<i64> {
        let closed = self.closed_through?;
        Some(
            self.closed
                .iter()
                .filter(|period| self.is_base(period))
                .map(|period| period.from)
                .fold(closed, i64::min),
        )
    }

    /// Where the store holds every hour, or every day.
    fn rollup_written(&self, level: i64) -> Option<i64> {
        let closed = self.closed_through?.div_euclid(level) * level;
        Some(
            self.closed
                .iter()
                .filter(|period| period.level == level && !self.is_base(period))
                .map(|period| period.from)
                .fold(closed, i64::min),
        )
    }

    /// The chunks that cover `[from, until)` from the store, widest first: a
    /// day or an hour only where it has been written or was empty, and the
    /// fold's own period at the edge of a window that starts inside one.
    fn plan(&self, from: i64, until: i64, sliced: bool, narrower_than: i64) -> Vec<(i64, i64)> {
        let Some(began) = self.began else {
            return Vec::new();
        };
        let mut chunks = Vec::new();
        let mut at = from;
        while at < until {
            let levels = self.levels_at(at);
            let base = levels[0];
            let level = if sliced && at == from {
                base
            } else {
                levels
                    .iter()
                    .rev()
                    .copied()
                    .filter(|level| *level < narrower_than)
                    .find(|level| {
                        at.rem_euclid(*level) == 0
                            && at >= began
                            && at + level <= until
                            && (*level == base
                                || self
                                    .rollup_written(*level)
                                    .is_some_and(|through| at + level <= through))
                    })
                    .unwrap_or(base)
            };
            chunks.push((level, at));
            at += level;
        }
        chunks
    }
}

/// What a window reads: the periods of the store it spans, and what the fold
/// holds past them.
#[derive(Debug, Default)]
struct Window {
    /// The start of the period the window's start falls in, or of the first
    /// the fold covers when that is later.
    edge: i64,
    since: i64,
    counted_from: Option<i64>,
    before_began: bool,
    /// Periods to read from the store, each `(level, from)`.
    stored: Vec<(i64, i64)>,
    /// The fold's own records past what the store holds.
    held: Vec<ObservedPeriod>,
    /// Runs in flight heard from in the window, by variant.
    running: BTreeMap<String, u64>,
    /// Gaps in the log reaching the window that are not written down yet.
    missed: Vec<LogGap>,
}

impl PeriodFold {
    /// What a window from `since` reads, with the fold's own records in it.
    fn window(&self, variant_ids: &[&str], since: i64) -> Window {
        let (Some(began), Some(base_written)) = (self.began, self.base_written().or(self.began))
        else {
            return Window::default();
        };
        let wanted = |variant: &str| variant_ids.contains(&variant);
        let edge = self.floor_at(since).max(began);
        let sliced = since > edge;
        let stored = self.plan(edge, base_written, sliced, i64::MAX);
        let held_from = edge.max(base_written);
        let held = self
            .closed
            .iter()
            .filter(|period| self.is_base(period) && period.from >= held_from)
            .flat_map(|period| period.records.iter())
            .chain(
                self.open
                    .range(held_from..)
                    .flat_map(|(_, variants)| variants.values()),
            )
            .filter(|record| wanted(&record.variant_id))
            .cloned()
            .collect();
        let mut running = BTreeMap::new();
        for run in self.runs.values() {
            if !run.measured && run.last_ms >= since * 1_000 && wanted(&run.variant_id) {
                *running.entry(run.variant_id.clone()).or_default() += 1;
            }
        }
        Window {
            edge,
            since,
            counted_from: Some(if sliced { since } else { edge }),
            before_began: since < began,
            stored,
            held,
            running,
            missed: self
                .missed
                .iter()
                .filter(|gap| gap.until >= since)
                .cloned()
                .collect(),
        }
    }
}

/// The fold as a projector output: applied per event, written at each flush,
/// and read by a window.
#[derive(Debug)]
pub struct PeriodOutput {
    store: PeriodStore,
    processor_id: String,
    fold: Mutex<PeriodFold>,
    saved_at: Mutex<Option<Instant>>,
}

impl PeriodOutput {
    #[must_use]
    pub fn new(store: PeriodStore, processor_id: impl Into<String>, width: i64) -> Self {
        Self {
            store,
            processor_id: processor_id.into(),
            fold: Mutex::new(PeriodFold::new(width)),
            saved_at: Mutex::new(None),
        }
    }

    /// Load the saved state furthest along that reads, and answer the position
    /// it was folded through — where the projector has to resume from when its
    /// checkpoint is ahead. With no state left, the fold starts again from
    /// where the last period written says it was, and adds the hour and the
    /// day that period lies in back up from what the store holds. A width
    /// configured anew takes over at the next hour.
    pub async fn load(&self) -> Option<u64> {
        let width = self.fold.lock().await.width_now();
        let loaded = match self.store.load_fold(&self.processor_id).await {
            Ok(Some(saved)) => Some(saved.settled()),
            Ok(None) => match self.recover(width).await {
                Ok(recovered) => recovered,
                Err(error) => {
                    tracing::warn!(%error, "the observation fold could not start again from the periods it wrote; starting afresh");
                    None
                }
            },
            Err(error) => {
                tracing::warn!(%error, "the observation fold's saved state could not be read; starting afresh");
                None
            }
        };
        let mut fold = self.fold.lock().await;
        if let Some(mut loaded) = loaded {
            loaded.change_width(width);
            loaded.contiguous = fold.contiguous;
            *fold = loaded;
        }
        fold.through
    }

    /// Whether the log this fold reads numbers every event one after the
    /// last, so a jump in positions is events it no longer holds.
    pub async fn reads_contiguous_positions(&self, contiguous: bool) {
        self.fold
            .lock()
            .await
            .reads_contiguous_positions(contiguous);
    }

    /// A fold started again from the last period written, with the hours and
    /// the day it was still adding up read back from the store.
    async fn recover(&self, width: i64) -> Result<Option<PeriodFold>, PortError> {
        let Some((level, from, at)) = self.store.last_written().await? else {
            return Ok(None);
        };
        tracing::warn!(
            through = at.through,
            "the observation fold has no saved state; starting again from the last period it wrote"
        );
        let mut fold = PeriodFold {
            widths: if at.widths.is_empty() {
                BTreeMap::from([(i64::MIN, width)])
            } else {
                at.widths
            },
            through: at.through,
            began: at.began.or(Some(from)),
            closed_through: Some(from + level),
            gap_from: Some(from + level),
            ..PeriodFold::default()
        };
        let (Some(began), Some(closed)) = (fold.began, fold.closed_through) else {
            return Ok(Some(fold));
        };
        for rollup in [HOUR, DAY] {
            let start = closed.div_euclid(rollup) * rollup;
            if start < began || start >= closed || fold.width_at(start) >= rollup {
                continue;
            }
            let mut adding: BTreeMap<String, ObservedPeriod> = BTreeMap::new();
            for (level, from) in fold.plan(start, closed, false, rollup) {
                for record in self.store.records(level, from).await? {
                    let into = adding
                        .entry(record.variant_id.clone())
                        .or_insert_with(|| empty(&record.variant_id, start, start + rollup));
                    into.merge(&record);
                    into.merge_late(&record);
                }
            }
            if !adding.is_empty() {
                fold.rollups
                    .entry(rollup)
                    .or_default()
                    .insert(start, adding);
            }
        }
        Ok(Some(fold))
    }

    pub async fn apply(&self, event: &RecordedEvent) {
        self.fold.lock().await.apply(event);
    }

    /// Write what closed, then save the state when it is due or `force`d.
    /// `false` when a period could not be written: the caller holds its
    /// checkpoint back, and the period is tried again at the next flush.
    pub async fn flush(&self, force: bool) -> bool {
        let (missed, waiting, at) = {
            let fold = self.fold.lock().await;
            (fold.missed.clone(), fold.closed.clone(), fold.at(None))
        };
        for gap in missed {
            if let Err(error) = self.store.write_gap(&gap).await {
                tracing::warn!(%error, first = gap.first_position, last = gap.last_position, "a gap in the log could not be written down; holding the checkpoint");
                return false;
            }
            tracing::warn!(
                first = gap.first_position,
                last = gap.last_position,
                events = gap.events(),
                "the log no longer held events the observation fold came to; the periods they may have ended runs in say they are incomplete"
            );
            self.fold.lock().await.gap_written(&gap);
        }
        for period in waiting {
            let at = FoldAt {
                through: period.through,
                ..at.clone()
            };
            if let Err(error) = self
                .store
                .write(
                    period.level,
                    period.from,
                    &period.records,
                    time::OffsetDateTime::now_utc().unix_timestamp(),
                    &at,
                )
                .await
            {
                tracing::warn!(%error, level = period.level, from = period.from, "an observed period could not be written; holding the checkpoint");
                return false;
            }
            self.fold.lock().await.written(period.level, period.from);
        }
        let mut saved_at = self.saved_at.lock().await;
        if force || saved_at.is_none_or(|at| at.elapsed() >= SAVE_EVERY) {
            let state = self.fold.lock().await.clone();
            let Some(through) = state.through.filter(|_| state.closed.is_empty()) else {
                return true;
            };
            match self
                .store
                .save_fold(&self.processor_id, through, &state)
                .await
            {
                Ok(()) => *saved_at = Some(Instant::now()),
                Err(error) => {
                    tracing::warn!(%error, "the observation fold's state could not be saved");
                }
            }
        }
        true
    }

    /// What each of these variants was observed doing since `since`, in Unix
    /// seconds: the runs that ended from then on, where they ended — from the
    /// written periods, from this fold, and, at the period the window starts
    /// in, from the slices after its start — and the runs in flight heard from
    /// in it. One row per variant, in the order asked.
    ///
    /// # Errors
    ///
    /// The store's own failure, or a written period that no longer reads.
    pub async fn observe(
        &self,
        variant_ids: &[&str],
        since: i64,
        prices: Option<&ModelPrices>,
    ) -> Result<Vec<VariantObservations>, PortError> {
        let window = self.fold.lock().await.window(variant_ids, since);
        let mut missed = self.store.gaps(since).await?;
        for gap in &window.missed {
            if !missed.contains(gap) {
                missed.push(gap.clone());
            }
        }
        let wanted: Vec<String> = variant_ids.iter().map(|id| (*id).to_owned()).collect();
        let stored: Vec<Vec<ObservedPeriod>> = futures::stream::iter(window.stored.clone())
            .map(|(level, from)| {
                let store = self.store.clone();
                let wanted = wanted.clone();
                async move {
                    let wanted: Vec<&str> = wanted.iter().map(String::as_str).collect();
                    store.period(level, from, &wanted).await
                }
            })
            .buffered(READ_AT_ONCE)
            .map_ok(Option::unwrap_or_default)
            .try_collect()
            .await?;
        let stored: Vec<ObservedPeriod> = stored.into_iter().flatten().collect();
        Ok(variant_ids
            .iter()
            .map(|variant_id| {
                let mut counted = Counted {
                    running: window.running.get(*variant_id).copied().unwrap_or(0),
                    counted_from: window.counted_from,
                    before_observations: window.before_began,
                    missed: missed.clone(),
                    ..Counted::default()
                };
                for (records, written) in [(&stored, true), (&window.held, false)] {
                    for record in records
                        .iter()
                        .filter(|record| record.variant_id == *variant_id)
                    {
                        counted.periods += usize::from(written);
                        if let Some(from) = record.straddled_from(window.edge, window.since) {
                            counted.counted_from =
                                Some(counted.counted_from.map_or(from, |at| at.min(from)));
                        }
                        counted.parts.extend(
                            record
                                .counted_since(window.edge, window.since)
                                .into_iter()
                                .map(|(record, late)| CountedPart {
                                    record,
                                    written,
                                    late,
                                }),
                        );
                    }
                }
                crate::observations::from_periods(variant_id, &counted, prices)
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use time::macros::datetime;

    use aiwatcher_core::{EventEnvelope, EventType, Sdk, Source};

    use super::*;
    use crate::periods::tests::MemoryObjectStore;

    const HOUR_START: time::OffsetDateTime = datetime!(2026-09-13 09:00:00 UTC);

    struct Log {
        events: Vec<RecordedEvent>,
    }

    impl Log {
        fn new() -> Self {
            Self { events: Vec::new() }
        }

        fn at(
            &mut self,
            run_id: &str,
            event_type: EventType,
            seconds: i64,
            data: serde_json::Value,
        ) -> &mut Self {
            let at = HOUR_START + time::Duration::seconds(seconds);
            let mut envelope =
                EventEnvelope::new(event_type, run_id, at, Source::new("app", Sdk::Python))
                    .with_data(data);
            envelope.variant_id = Some("v1".to_owned());
            let position = self.events.len() as u64 + 1;
            self.events
                .push(envelope.record(position, position, at, None));
            self
        }

        fn served(&mut self, run_id: &str, start: i64, end: i64) -> &mut Self {
            let call = serde_json::json!({"call_id": "c", "model": "gpt-4o"});
            let done = serde_json::json!({"call_id": "c", "model": "gpt-4o",
                "prompt_tokens": 10, "completion_tokens": 2});
            self.at(run_id, EventType::RunStarted, start, serde_json::json!({}))
                .at(run_id, EventType::LlmStarted, start, call)
                .at(run_id, EventType::LlmCompleted, end, done)
                .at(run_id, EventType::RunCompleted, end, serde_json::json!({}))
        }
    }

    fn folded(width: i64, events: &[RecordedEvent]) -> (PeriodFold, Vec<ClosedPeriod>) {
        let mut fold = PeriodFold::new(width);
        let mut closed = Vec::new();
        for event in events {
            fold.apply(event);
            closed.extend(fold.take_closed());
        }
        (fold, closed)
    }

    fn at(closed: &[ClosedPeriod], level: i64) -> Vec<(i64, u64, u64)> {
        let start = HOUR_START.unix_timestamp();
        closed
            .iter()
            .filter(|period| period.level == level)
            .map(|period| {
                let runs = period.records.iter().map(|record| record.runs).sum();
                let late = period.records.iter().map(|record| record.late_runs).sum();
                (period.from - start, runs, late)
            })
            .collect()
    }

    #[test]
    fn a_period_closes_once_the_log_s_clock_has_passed_it_and_holds_its_runs() {
        let mut log = Log::new();
        log.served("r1", 60, 62).served("r2", 120, 125);
        // Nothing has passed the hour yet.
        assert!(folded(3_600, &log.events).1.is_empty());
        log.served("r3", 3_600 + 400, 3_600 + 401);

        let (_, closed) = folded(3_600, &log.events);

        assert_eq!(at(&closed, 3_600), [(0, 2, 0)]);
        let record = &closed[0].records[0];
        assert!(record.complete);
        assert_eq!((record.run_ms.count, record.call_ms.count), (2, 2));
        assert_eq!(
            (
                record.llm_calls,
                record.input_tokens,
                record.models[0].calls
            ),
            (2, 20, 2)
        );
    }

    #[test]
    fn periods_add_up_to_the_hour_and_the_day_and_a_quiet_period_is_not_written() {
        let mut log = Log::new();
        log.served("r1", 60, 62)
            .served("r2", 1_000, 1_010)
            .served("r3", 3_600 + 10, 3_600 + 20);
        // The day the hours are in ends when a run ends past it.
        log.served("r4", 86_400 + 400, 86_400 + 401);

        let (fold, closed) = folded(300, &log.events);

        assert_eq!(fold.levels(), [300, 3_600, 86_400]);
        assert_eq!(
            at(&closed, 300),
            [(0, 1, 0), (900, 1, 0), (3_600, 1, 0)],
            "the periods anything ended in, and no others"
        );
        assert_eq!(at(&closed, 3_600), [(0, 2, 0), (3_600, 1, 0)]);
        assert!(
            at(&closed, 86_400).is_empty(),
            "the day began before the fold did, so it holds only part of itself and is not kept"
        );
        let first_hour = closed
            .iter()
            .find(|period| period.level == 3_600)
            .expect("the first hour");
        assert_eq!(
            (
                first_hour.records[0].run_ms.count,
                first_hour.records[0].llm_calls
            ),
            (2, 2)
        );
    }

    #[test]
    fn a_run_across_midnight_keeps_each_call_on_the_day_it_ended() {
        let midnight = 15 * 3_600;
        let mut log = Log::new();
        let call = |id: &str| serde_json::json!({"call_id": id, "model": "gpt-4o"});
        let done =
            |id: &str| serde_json::json!({"call_id": id, "model": "gpt-4o", "prompt_tokens": 10});
        log.at(
            "r1",
            EventType::RunStarted,
            midnight - 20,
            serde_json::json!({}),
        )
        .at("r1", EventType::LlmStarted, midnight - 20, call("a"))
        .at("r1", EventType::LlmCompleted, midnight - 10, done("a"))
        .at("r1", EventType::LlmStarted, midnight - 5, call("b"))
        .at("r1", EventType::LlmCompleted, midnight + 5, done("b"))
        .at(
            "r1",
            EventType::RunCompleted,
            midnight + 10,
            serde_json::json!({}),
        );

        let (fold, _) = folded(300, &log.events);

        let record = fold
            .open
            .values()
            .flat_map(BTreeMap::values)
            .next()
            .expect("the run's period is open");
        let days: Vec<(&str, u64)> = record
            .models_by_day
            .iter()
            .map(|(day, models)| (day.as_str(), models[0].calls))
            .collect();
        assert_eq!(days, [("2026-09-13", 1), ("2026-09-14", 1)]);
    }

    #[test]
    fn a_run_ending_in_a_closed_period_is_held_in_the_oldest_open_one_by_the_period_it_ended_in() {
        let mut log = Log::new();
        log.served("r1", 60, 62)
            .served("r2", 3_600 + 400, 3_600 + 401);
        // Its end arrives after the first hour closed, dated inside it.
        log.at("late", EventType::RunStarted, 100, serde_json::json!({}))
            .at("late", EventType::RunCompleted, 200, serde_json::json!({}))
            .served("r3", 7_200 + 400, 7_200 + 401);

        let (_, closed) = folded(3_600, &log.events);

        assert_eq!(
            at(&closed, 3_600),
            [(0, 1, 0), (3_600, 1, 1)],
            "held, and said to be late, rather than lost"
        );
        let held = &closed[1].records[0];
        assert_eq!(
            held.late.keys().copied().collect::<Vec<_>>(),
            [HOUR_START.unix_timestamp()],
            "by the hour it ended in"
        );
        assert_eq!(held.late[&HOUR_START.unix_timestamp()].runs, 1);
    }

    #[test]
    fn a_fold_saved_midway_and_resumed_writes_what_one_reading_would_and_counts_nothing_twice() {
        let mut log = Log::new();
        log.served("r1", 60, 62)
            .at("r2", EventType::RunStarted, 3_500, serde_json::json!({}))
            .served("r3", 3_600 + 10, 3_600 + 20)
            .at(
                "r2",
                EventType::RunCompleted,
                3_600 + 30,
                serde_json::json!({}),
            )
            .served("r4", 7_200 + 400, 7_200 + 401);
        let (_, whole) = folded(300, &log.events);

        // Saved after the first six events — r2 in flight — then a restart that
        // is handed the log again from the start, as a replay would be.
        let (saved, before) = folded(300, &log.events[..6]);
        let bytes = serde_json::to_vec(&saved).expect("saves");
        let mut resumed: PeriodFold = serde_json::from_slice(&bytes).expect("loads");
        let mut after = Vec::new();
        for event in &log.events {
            resumed.apply(event);
            after.extend(resumed.take_closed());
        }

        let reread: Vec<ClosedPeriod> = before.into_iter().chain(after).collect();
        assert_eq!(reread, whole);
        assert_eq!(
            at(&whole, 3_600),
            [(0, 1, 0), (3_600, 2, 0)],
            "r1 in the first hour, and r2 and r3 in the second, once each"
        );
    }

    #[test]
    fn a_run_whose_start_the_fold_never_saw_is_counted_without_a_duration_and_says_so() {
        let mut log = Log::new();
        log.at("before", EventType::RunCompleted, 30, serde_json::json!({}))
            .served("r1", 60, 62)
            .served("r2", 3_600 + 400, 3_600 + 401);

        let (_, closed) = folded(3_600, &log.events);

        let record = &closed[0].records[0];
        assert_eq!(
            (record.runs, record.run_ms.count, record.complete),
            (2, 1, false)
        );
    }

    #[test]
    fn a_producer_clock_in_the_future_closes_nothing_the_server_has_not_reached() {
        let mut log = Log::new();
        log.served("r1", 60, 62);
        let mut skewed = log.events[0].clone();
        skewed.metadata.global_position = 99;
        skewed.metadata.run_id = "skewed".to_owned();
        skewed.metadata.occurred_at = HOUR_START + time::Duration::days(1);
        let mut events = log.events.clone();
        events.push(skewed);

        assert!(
            folded(3_600, &events).1.is_empty(),
            "ingested at nine, whatever it says"
        );
    }

    async fn output_over(events: &[RecordedEvent]) -> PeriodOutput {
        let output = PeriodOutput::new(
            PeriodStore::new(Arc::new(MemoryObjectStore::default())),
            "projector",
            300,
        );
        for event in events {
            output.apply(event).await;
            assert!(output.flush(false).await, "the memory store writes");
        }
        output
    }

    #[tokio::test]
    async fn a_window_counts_each_run_once_where_it_ended_from_the_second_the_window_starts() {
        let start = HOUR_START.unix_timestamp();
        let mut log = Log::new();
        log.served("r1", 60, 62)
            .served("r2", 1_000, 1_010)
            .served("r3", 3_600 + 10, 3_600 + 20)
            .at("going", EventType::RunStarted, 7_000, serde_json::json!({}))
            .served("r4", 7_200 + 30, 7_200 + 40);
        // Ended in the first hour and received in the third: counted where it
        // ended, the moment it ends.
        log.at("late", EventType::RunStarted, 500, serde_json::json!({}))
            .at("late", EventType::RunCompleted, 510, serde_json::json!({}));
        let output = output_over(&log.events).await;

        let [row] = output
            .observe(&["v1"], start + 500, None)
            .await
            .expect("reads")
            .try_into()
            .expect("one row");

        assert_eq!(
            (row.runs, row.late_runs, row.running),
            (4, 1, 1),
            "r2 through r4 and the late one, and the run in flight"
        );
        assert!(row.periods > 0 && row.runs_from_periods > 0);
        assert_eq!(
            row.counted_from.map(time::OffsetDateTime::unix_timestamp),
            Some(start + 500),
            "from the second the window starts"
        );
        let from_the_hour = output
            .observe(&["v1"], start + 3_600, None)
            .await
            .expect("reads");
        assert_eq!(
            from_the_hour[0].runs, 2,
            "r3 and r4: the late one ended before the window, wherever it arrived"
        );
        let from_after_r2 = output
            .observe(&["v1"], start + 1_011, None)
            .await
            .expect("reads");
        assert_eq!(
            from_after_r2[0].runs, 2,
            "r2 ended a second before, inside the period the window starts in"
        );
        assert!(!row.window_before_observations);
        let before = output
            .observe(&["v1"], start - 86_400, None)
            .await
            .expect("reads");
        assert!(before[0].window_before_observations);
        assert_eq!(before[0].runs, 5);
    }

    #[tokio::test]
    async fn an_hour_wide_period_counts_a_window_from_its_second_and_an_older_slice_says_where() {
        let start = HOUR_START.unix_timestamp();
        let mut log = Log::new();
        log.served("before", 990, 1_000)
            .served("after", 995, 1_005)
            .served("next hour", 4_000, 4_010);
        let objects = Arc::new(MemoryObjectStore::default());
        let store = PeriodStore::new(objects.clone());
        let output = PeriodOutput::new(store.clone(), "projector", 3_600);
        for event in &log.events {
            output.apply(event).await;
            assert!(output.flush(false).await, "the memory store writes");
        }

        let [row] = output
            .observe(&["v1"], start + 1_003, None)
            .await
            .expect("reads")
            .try_into()
            .expect("one row");
        assert_eq!(
            row.runs, 2,
            "the run that ended at 1005 and the next hour's, not the one at 1000"
        );
        assert_eq!(
            row.counted_from.map(time::OffsetDateTime::unix_timestamp),
            Some(start + 1_003)
        );

        // The same hour as it was written when an hour kept twelve-second
        // slices: counted from where the slice holding the window's start
        // began, and saying so.
        let mut written = store
            .records(3_600, start)
            .await
            .expect("reads")
            .into_iter()
            .next()
            .expect("the first hour was written");
        let mut wide = ObservedPeriod {
            from: start + 996,
            to: start + 1_008,
            ..ObservedPeriod::default()
        };
        for slice in std::mem::take(&mut written.slices).into_values() {
            wide.merge(&slice);
        }
        written.slices = BTreeMap::from([(996, wide)]);
        assert_eq!(
            written.straddled_from(start, start + 1_003),
            Some(start + 996)
        );
        assert_eq!(written.straddled_from(start, start + 1_008), None);
    }

    #[tokio::test]
    async fn positions_the_log_no_longer_holds_are_written_down_and_the_periods_they_reach_say_so()
    {
        let start = HOUR_START.unix_timestamp();
        let mut log = Log::new();
        log.served("r1", 60, 62)
            .served("evicted", 400, 410)
            .served("r3", 700, 710);
        // Retention took the second run's four events before the fold read them.
        let kept: Vec<RecordedEvent> = log
            .events
            .iter()
            .filter(|event| !(5..=8).contains(&event.metadata.global_position))
            .cloned()
            .collect();
        let store = PeriodStore::new(Arc::new(MemoryObjectStore::default()));
        let output = PeriodOutput::new(store.clone(), "projector", 300);
        output.reads_contiguous_positions(true).await;
        for event in &kept {
            output.apply(event).await;
            assert!(output.flush(false).await, "the memory store writes");
        }

        let [row] = output
            .observe(&["v1"], start, None)
            .await
            .expect("reads")
            .try_into()
            .expect("one row");
        assert_eq!(row.runs, 2, "what the log still held");
        assert_eq!(
            row.missed
                .iter()
                .map(|gap| (
                    gap.events,
                    gap.from.unix_timestamp() - start,
                    gap.until.unix_timestamp() - start
                ))
                .collect::<Vec<_>>(),
            [(4, 62, 700)],
            "from the log's clock before them to the first event after"
        );
        assert_eq!(
            row.incomplete_periods, 2,
            "the period open when the gap was found, and the one a run was placed in after"
        );
        assert_eq!(store.gaps(start).await.expect("lists").len(), 1);
        let after = output
            .observe(&["v1"], start + 701, None)
            .await
            .expect("reads");
        assert!(after[0].missed.is_empty(), "a window after the span");

        let unnumbered = output_over(&kept).await;
        let [row] = unnumbered
            .observe(&["v1"], start, None)
            .await
            .expect("reads")
            .try_into()
            .expect("one row");
        assert!(
            row.missed.is_empty() && row.incomplete_periods == 0,
            "a log that does not number every event says nothing by a jump"
        );
    }

    #[tokio::test]
    async fn a_width_configured_anew_takes_over_at_the_next_hour_and_a_window_reads_both() {
        let start = HOUR_START.unix_timestamp();
        let objects = Arc::new(MemoryObjectStore::default());
        let store = PeriodStore::new(objects.clone());
        let mut log = Log::new();
        log.served("r1", 60, 62)
            .at("r2", EventType::RunStarted, 3_800, serde_json::json!({}))
            .served("r3", 3_850, 3_900);
        let hourly = PeriodOutput::new(store.clone(), "projector", 3_600);
        for event in &log.events {
            hourly.apply(event).await;
        }
        assert!(hourly.flush(true).await);

        // The same fold, configured for five minutes, reading on.
        log.at("r2", EventType::RunCompleted, 5_000, serde_json::json!({}))
            .served("r4", 7_250, 7_300)
            .served("r5", 9_000, 9_200);
        let fine = PeriodOutput::new(store.clone(), "projector", 300);
        let resumed = fine.load().await.expect("saved");
        for event in log.events.iter().skip(usize::try_from(resumed).unwrap()) {
            fine.apply(event).await;
        }
        assert!(fine.flush(true).await);

        assert!(
            store
                .period(3_600, start + 3_600, &["v1"])
                .await
                .unwrap()
                .is_some(),
            "the hour before the switch, at the old width"
        );
        assert!(
            store
                .period(300, start + 7_200, &["v1"])
                .await
                .unwrap()
                .is_some(),
            "and five minutes after it"
        );
        assert!(
            store
                .period(300, start + 3_600, &["v1"])
                .await
                .unwrap()
                .is_none(),
            "never both over one span"
        );
        let [row] = fine
            .observe(&["v1"], start, None)
            .await
            .expect("reads")
            .try_into()
            .expect("one row");
        assert_eq!(
            (row.runs, row.running),
            (5, 0),
            "each run once, the one in flight across the switch included"
        );
        let after = fine
            .observe(&["v1"], start + 7_280, None)
            .await
            .expect("reads");
        assert_eq!(
            after[0].runs, 2,
            "r4 ended at 7300 and r5 after, at the new width"
        );
    }

    #[tokio::test]
    async fn a_fold_with_no_saved_state_starts_again_from_the_last_period_it_wrote() {
        let midnight = -9 * 3_600;
        let day = HOUR_START.unix_timestamp() + midnight;
        let objects = Arc::new(MemoryObjectStore::default());
        let store = PeriodStore::new(objects.clone());
        let mut log = Log::new();
        log.served("r1", midnight + 60, midnight + 70)
            .served("r2", midnight + 3_700, midnight + 3_710)
            .served("r3", midnight + 7_300, midnight + 7_310);
        let first = PeriodOutput::new(store.clone(), "projector", 300);
        for event in &log.events {
            first.apply(event).await;
            assert!(first.flush(false).await);
        }
        assert!(first.flush(true).await);
        let written = log.events.len();

        // Every saved state is lost.
        objects
            .0
            .write()
            .await
            .retain(|key, _| !key.contains("variant-observations/fold/"));

        let second = PeriodOutput::new(store.clone(), "projector", 300);
        let resumed = second.load().await.expect("started again from its periods");
        assert!(usize::try_from(resumed).unwrap() <= written);
        // The log from the start, as a replay hands it, and a day later.
        log.served("r4", midnight + 7_600, midnight + 7_620).served(
            "r5",
            midnight + 86_400 + 400,
            midnight + 86_400 + 410,
        );
        for event in &log.events {
            second.apply(event).await;
            assert!(second.flush(false).await);
        }

        let [row] = second
            .observe(&["v1"], day, None)
            .await
            .expect("reads")
            .try_into()
            .expect("one row");
        assert_eq!(row.runs, 5, "the periods before, and each run after, once");
        let whole_day = store.records(86_400, day).await.unwrap();
        assert_eq!(
            whole_day.iter().map(|record| record.runs).sum::<u64>(),
            4,
            "the day adds up the hours written before as well as after"
        );
        assert!(
            !row.window_before_observations,
            "observations still began at midnight"
        );
    }

    #[tokio::test]
    async fn a_period_that_could_not_be_written_is_read_from_the_fold_until_it_is() {
        let start = HOUR_START.unix_timestamp();
        let mut log = Log::new();
        log.served("r1", 60, 62)
            .served("r2", 3_600 + 10, 3_600 + 20)
            .served("r3", 7_200 + 400, 7_200 + 410);
        let output = PeriodOutput::new(
            PeriodStore::new(Arc::new(MemoryObjectStore::default())),
            "projector",
            300,
        );
        for event in &log.events {
            output.apply(event).await;
        }
        // Nothing flushed: every closed period is still the fold's.
        let [row] = output
            .observe(&["v1"], start, None)
            .await
            .expect("reads")
            .try_into()
            .expect("one row");
        assert_eq!((row.runs, row.periods), (3, 0));

        assert!(output.flush(true).await);
        let [row] = output
            .observe(&["v1"], start, None)
            .await
            .expect("reads")
            .try_into()
            .expect("one row");
        assert_eq!(
            (row.runs, row.runs_from_periods),
            (3, 2),
            "r3's period is open"
        );
    }
}
