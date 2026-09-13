//! What variants were observed doing, folded period by period as the log is
//! read, with a state of its own that survives a restart — and the one answer a
//! window over them gets (ADR_0030, amended).
//!
//! A projection like the others: it reads each event once, in log order, keeps
//! the runs in flight and the periods still open, and writes a period when the
//! log's clock — the latest `min(occurred_at, ingested_at)`, so a skewed
//! producer closes nothing early — has passed it, rolled up into the hour and
//! the day it lies in, so a week is read as a handful of records. Its state is
//! saved with the position it was folded through; a restart loads the one
//! furthest along, the projector resumes from that position when it is behind
//! its checkpoint, and every event at or before it is skipped — so no period
//! goes unwritten and no event counts twice. A run that ends in a period already
//! closed is counted in the oldest one still open and said to be late; a run
//! whose start the fold never saw is counted with no duration, and its period
//! says it is incomplete.
//!
//! A window is answered here and nowhere else ([`PeriodOutput::observe`]):
//! every period it reaches into, whole — written ones from the store, the rest
//! from this fold — and the runs in flight. One source, so no run is counted
//! twice or missed between two; counting starts at the beginning of the period
//! the window's start falls in, which the answer says (`counted_from`).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use futures::{StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use aiwatcher_core::ports::PortError;
use aiwatcher_core::prices::{ModelPrices, ModelUsage};
use aiwatcher_core::{Phase, RecordedEvent, Subject};

use crate::observations::{ObservedPeriod, VariantObservations};
use crate::periods::PeriodStore;

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
    call_ms: Vec<i64>,
    ttft_ms: Vec<i64>,
    calls: BTreeMap<String, OpenCall>,
}

/// One closed period's records, waiting to be written.
#[derive(Clone, Debug, PartialEq)]
pub struct ClosedPeriod {
    /// How wide it is: the fold's width, an hour or a day.
    pub level: i64,
    pub from: i64,
    pub records: Vec<ObservedPeriod>,
}

/// The fold itself: pure, and what is saved.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PeriodFold {
    width: i64,
    grace: i64,
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
    runs: BTreeMap<String, InFlight>,
    /// Periods at the fold's width still open, by their start.
    open: BTreeMap<i64, BTreeMap<String, ObservedPeriod>>,
    /// Hours and days still open, by level and then start: what the closed
    /// periods inside each have added up to so far.
    #[serde(default)]
    rollups: BTreeMap<i64, BTreeMap<i64, BTreeMap<String, ObservedPeriod>>>,
    /// Closed and not yet written; never saved, because a state is saved only
    /// once these are written.
    #[serde(skip)]
    closed: Vec<ClosedPeriod>,
}

fn millis(at: time::OffsetDateTime) -> i64 {
    i64::try_from(at.unix_timestamp_nanos() / 1_000_000).unwrap_or(i64::MAX)
}

impl PeriodFold {
    /// A fold of periods `width` seconds wide, each closing a tenth of that
    /// after its end, at most five minutes. A width is a whole part of an
    /// hour, which is what lets its periods add up to hours and days; one that
    /// is not gets no rollups.
    #[must_use]
    pub fn new(width: i64) -> Self {
        let width = width.max(1);
        Self {
            width,
            grace: (width / 10).clamp(1, 300),
            ..Self::default()
        }
    }

    /// The widths periods are written at, narrowest first.
    #[must_use]
    pub fn levels(&self) -> Vec<i64> {
        let mut levels = vec![self.width];
        if HOUR % self.width == 0 {
            for level in [HOUR, DAY] {
                if level > self.width {
                    levels.push(level);
                }
            }
        }
        levels
    }

    /// The position this fold has read through.
    #[must_use]
    pub fn through(&self) -> Option<u64> {
        self.through
    }

    /// Fold one event in, once: an event at or before `through` is skipped.
    pub fn apply(&mut self, event: &RecordedEvent) {
        let position = event.metadata.global_position;
        if self.through.is_some_and(|through| position <= through) {
            return;
        }
        self.through = Some(position);
        let bound = event
            .metadata
            .occurred_at
            .min(event.metadata.ingested_at)
            .unix_timestamp();
        if self.began.is_none() {
            self.began = Some(bound.div_euclid(self.width) * self.width);
        }
        let subject = event.event_type.subject();
        if subject != Subject::Eval {
            self.fold(event, subject);
        }
        self.clock = Some(self.clock.map_or(bound, |clock| clock.max(bound)));
        self.close();
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
                ModelUsage::add_to(
                    &mut run.models,
                    &ModelUsage {
                        model,
                        calls: 1,
                        input_tokens: input,
                        output_tokens: output,
                        cached_tokens: cached,
                    },
                );
            }
            _ => {}
        }
    }

    /// The open period a run ending at `end_seconds` is counted in, and
    /// whether that is later than the period it ended in.
    fn record(&mut self, variant_id: &str, end_seconds: i64) -> (&mut ObservedPeriod, bool) {
        let ended_in = end_seconds.div_euclid(self.width) * self.width;
        let from = ended_in
            .max(self.closed_through.unwrap_or(ended_in))
            .max(self.began.unwrap_or(ended_in));
        let width = self.width;
        let record = self
            .open
            .entry(from)
            .or_default()
            .entry(variant_id.to_owned())
            .or_insert_with(|| ObservedPeriod {
                variant_id: variant_id.to_owned(),
                from,
                to: from + width,
                complete: true,
                ..ObservedPeriod::default()
            });
        (record, from > ended_in)
    }

    fn finish(&mut self, run: InFlight, ok: bool, at: i64, end_seconds: i64) {
        let (record, late) = self.record(&run.variant_id, end_seconds);
        record.late_runs += u64::from(late);
        if run.measured {
            record.measured_runs += 1;
            return;
        }
        record.runs += 1;
        if ok {
            record.succeeded += 1;
        } else {
            record.failed += 1;
        }
        if run.saw_start {
            record.run_ms.add((at - run.started_ms).max(0));
        } else {
            record.complete = false;
        }
        for took in run.call_ms {
            record.call_ms.add(took);
        }
        for first in run.ttft_ms {
            record.time_to_first_token_ms.add(first);
        }
        record.llm_calls += run.llm_calls;
        record.input_tokens += run.input_tokens;
        record.output_tokens += run.output_tokens;
        for model in &run.models {
            ModelUsage::add_to(&mut record.models, model);
        }
        let started = run.started_ms.div_euclid(1_000);
        record.first_seen_at = Some(
            record
                .first_seen_at
                .map_or(started, |seen| seen.min(started)),
        );
        record.last_seen_at = Some(
            record
                .last_seen_at
                .map_or(end_seconds, |seen| seen.max(end_seconds)),
        );
    }

    /// A run naming a variant whose start this fold never tracked: counted,
    /// with no duration, and its period incomplete.
    fn finish_unseen(&mut self, variant_id: &str, ok: bool, end_seconds: i64) {
        let (record, late) = self.record(variant_id, end_seconds);
        record.late_runs += u64::from(late);
        record.runs += 1;
        if ok {
            record.succeeded += 1;
        } else {
            record.failed += 1;
        }
        record.complete = false;
        record.last_seen_at = Some(
            record
                .last_seen_at
                .map_or(end_seconds, |seen| seen.max(end_seconds)),
        );
    }

    /// Close every period the log's clock has passed, then every hour and day
    /// whose last period that was. A period nothing ended in closes as nothing
    /// and is not written; a rollup that began before the fold did is never
    /// kept, since it would hold only part of its span.
    fn close(&mut self) {
        let (Some(clock), Some(began)) = (self.clock, self.began) else {
            return;
        };
        let edge = (clock - self.grace).div_euclid(self.width) * self.width;
        if edge <= self.closed_through.unwrap_or(began) {
            return;
        }
        let levels = self.levels();
        let closing: Vec<i64> = self.open.range(..edge).map(|(from, _)| *from).collect();
        for from in closing {
            let Some(variants) = self.open.remove(&from) else {
                continue;
            };
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
                    rollup
                        .entry(record.variant_id.clone())
                        .or_insert_with(|| ObservedPeriod {
                            variant_id: record.variant_id.clone(),
                            from: start,
                            to: start + level,
                            complete: true,
                            ..ObservedPeriod::default()
                        })
                        .merge(record);
                }
            }
            self.closed.push(ClosedPeriod {
                level: self.width,
                from,
                records: variants.into_values().collect(),
            });
        }
        for level in &levels[1..] {
            let Some(open) = self.rollups.get_mut(level) else {
                continue;
            };
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
                    });
                }
            }
        }
        self.closed_through = Some(edge);
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

    /// Where the store holds every period of a level: before the first period
    /// of it still waiting to be written, and never past what has closed.
    fn written_through(&self, level: i64, waiting: &[ClosedPeriod]) -> Option<i64> {
        let closed = self.closed_through?.div_euclid(level) * level;
        Some(
            waiting
                .iter()
                .filter(|period| period.level == level)
                .map(|period| period.from)
                .fold(closed, i64::min),
        )
    }
}

/// What a window reads: the periods of the store it spans, and what the fold
/// holds past them.
#[derive(Debug, Default)]
struct Window {
    /// Where counting starts: the period the window's start falls in, or the
    /// first the fold covers when that is later.
    counted_from: Option<i64>,
    before_began: bool,
    /// Periods to read from the store, each `(level, from)`.
    stored: Vec<(i64, i64)>,
    /// The fold's own records past what the store holds.
    held: Vec<ObservedPeriod>,
    /// Runs in flight heard from in the window, by variant.
    running: BTreeMap<String, u64>,
}

impl PeriodFold {
    /// What a window from `since` reads, with the fold's own records in it.
    fn window(&self, variant_ids: &[&str], since: i64) -> Window {
        let Some(began) = self.began else {
            return Window::default();
        };
        let wanted = |variant: &str| variant_ids.contains(&variant);
        let from = (since.div_euclid(self.width) * self.width).max(began);
        let levels = self.levels();
        let written: BTreeMap<i64, i64> = levels
            .iter()
            .filter_map(|level| Some((*level, self.written_through(*level, &self.closed)?)))
            .collect();
        let base_written = written.get(&self.width).copied().unwrap_or(from);
        let mut stored = Vec::new();
        let mut at = from;
        while at < base_written {
            let level = levels
                .iter()
                .rev()
                .copied()
                .find(|level| {
                    at.rem_euclid(*level) == 0
                        && at >= began
                        && at + level <= base_written
                        && written
                            .get(level)
                            .is_some_and(|through| at + level <= *through)
                })
                .unwrap_or(self.width);
            stored.push((level, at));
            at += level;
        }
        let held_from = from.max(base_written);
        let held = self
            .closed
            .iter()
            .filter(|period| period.level == self.width && period.from >= held_from)
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
            counted_from: Some(from),
            before_began: since < began,
            stored,
            held,
            running,
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

    /// Load the saved state furthest along, and answer the position it was
    /// folded through — where the projector has to resume from when its
    /// checkpoint is ahead. A state saved for another width is not this
    /// fold's and starts afresh: windows then reach back only to where the new
    /// one began.
    pub async fn load(&self) -> Option<u64> {
        match self.store.load_fold(&self.processor_id).await {
            Ok(Some(saved)) => {
                let mut fold = self.fold.lock().await;
                if saved.width == fold.width {
                    *fold = saved;
                } else {
                    tracing::warn!(
                        saved = saved.width,
                        configured = fold.width,
                        "the saved observation fold is for another period width; starting afresh"
                    );
                }
                fold.through
            }
            Ok(None) => None,
            Err(error) => {
                tracing::warn!(%error, "the observation fold's saved state could not be read; starting afresh");
                None
            }
        }
    }

    pub async fn apply(&self, event: &RecordedEvent) {
        self.fold.lock().await.apply(event);
    }

    /// Write what closed, then save the state when it is due or `force`d.
    /// `false` when a period could not be written: the caller holds its
    /// checkpoint back, and the period is tried again at the next flush.
    pub async fn flush(&self, force: bool) -> bool {
        let waiting = self.fold.lock().await.closed.clone();
        for period in waiting {
            if let Err(error) = self
                .store
                .write(
                    period.level,
                    period.from,
                    &period.records,
                    time::OffsetDateTime::now_utc().unix_timestamp(),
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
    /// seconds: every period the window reaches into, whole — the written ones
    /// from the store, the rest from this fold — and the runs in flight heard
    /// from in it. One row per variant, in the order asked.
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
                let mine = |records: &[ObservedPeriod]| -> Vec<ObservedPeriod> {
                    records
                        .iter()
                        .filter(|record| record.variant_id == *variant_id)
                        .cloned()
                        .collect()
                };
                crate::observations::from_periods(
                    variant_id,
                    &mine(&stored),
                    &mine(&window.held),
                    window.running.get(*variant_id).copied().unwrap_or(0),
                    window.counted_from,
                    window.before_began,
                    prices,
                )
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
    fn a_run_ending_in_a_closed_period_is_counted_late_in_the_oldest_open_one() {
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
            [(0, 1, 0), (3_600, 2, 1)],
            "counted, and said to be late, rather than lost"
        );
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
    async fn a_window_counts_each_run_once_from_the_store_and_the_fold_and_a_late_one_at_once() {
        let start = HOUR_START.unix_timestamp();
        let mut log = Log::new();
        log.served("r1", 60, 62)
            .served("r2", 1_000, 1_010)
            .served("r3", 3_600 + 10, 3_600 + 20)
            .at("going", EventType::RunStarted, 7_000, serde_json::json!({}))
            .served("r4", 7_200 + 30, 7_200 + 40);
        // Ended in the first hour and received in the third: in no written
        // period, and in the window the moment it ends.
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
            Some(start + 300),
            "from the start of the period the window's start is in"
        );
        let from_the_hour = output
            .observe(&["v1"], start + 3_600, None)
            .await
            .expect("reads");
        assert_eq!(
            from_the_hour[0].runs, 3,
            "r3, r4 and the late one, which is counted where it arrived, in the third hour"
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
