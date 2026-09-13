//! What variants were observed doing, folded period by period as the log is
//! read — with a state of its own that survives a restart.
//!
//! A snapshot of the read model could only write a period the read model still
//! held whole, so a process resuming from a checkpoint (Laser, which does not
//! replay its topic) left every period before it empty, and a run whose end
//! arrived after its period was written was in none. This fold is a projection
//! like the others instead: it reads each event once, in log order, keeps the
//! runs in flight and the periods still open, and writes a period when the
//! log's clock has passed it. Its state is saved beside the periods with the
//! position it was folded through; a restart loads it, the projector resumes
//! from that position when it is behind the stored checkpoint, and every event
//! at or before it is skipped — so no period goes unwritten and no event counts
//! twice.
//!
//! The log's clock is the latest `min(occurred_at, ingested_at)` folded: a
//! producer's clock bounded by the server's, so a skewed producer cannot close
//! tomorrow's periods, and a replay closes them where the first reading did. A
//! run that ends in a period already written is counted in the oldest period
//! still open and said to be late (`late_runs`); a run whose start the fold
//! never saw is counted with no duration, and its period says it is incomplete.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use aiwatcher_core::prices::ModelUsage;
use aiwatcher_core::{Phase, RecordedEvent, Subject};

use crate::observations::ObservedPeriod;
use crate::periods::PeriodStore;

/// Runs in flight past this are not tracked; each is counted at its end with
/// no duration, in a period that says it is incomplete.
const MOST_IN_FLIGHT: usize = 50_000;
/// Call durations a run keeps; its call count goes on past it.
const MOST_CALLS_TIMED: usize = 1_000;
/// Empty periods written in a row before a gap is jumped rather than marked.
const MOST_EMPTY_IN_A_ROW: i64 = 1_000;
/// How often the state is saved when nothing forces it.
const SAVE_EVERY: Duration = Duration::from_secs(15);

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
    pub from: i64,
    pub to: i64,
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
    /// The end of the last period closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    closed_through: Option<i64>,
    runs: BTreeMap<String, InFlight>,
    open: BTreeMap<i64, BTreeMap<String, ObservedPeriod>>,
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
    /// after its end, at most five minutes.
    #[must_use]
    pub fn new(width: i64) -> Self {
        let width = width.max(1);
        Self {
            width,
            grace: (width / 10).clamp(1, 300),
            ..Self::default()
        }
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
        let subject = event.event_type.subject();
        if subject == Subject::Eval {
            return;
        }
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

        let bound = event
            .metadata
            .occurred_at
            .min(event.metadata.ingested_at)
            .unix_timestamp();
        self.clock = Some(self.clock.map_or(bound, |clock| clock.max(bound)));
        self.close();
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
        let (from, late) = match self.closed_through {
            Some(closed) if ended_in < closed => (closed, true),
            _ => (ended_in, false),
        };
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
        (record, late)
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

    /// Close every period the log's clock has passed.
    fn close(&mut self) {
        let Some(clock) = self.clock else { return };
        let edge = (clock - self.grace).div_euclid(self.width) * self.width;
        let Some(mut from) = self
            .closed_through
            .or_else(|| self.open.keys().next().copied())
        else {
            return;
        };
        let mut empty_in_a_row = 0;
        let mut advanced = false;
        while from + self.width <= edge {
            advanced = true;
            let records: Vec<ObservedPeriod> = self
                .open
                .remove(&from)
                .map(|variants| variants.into_values().collect())
                .unwrap_or_default();
            if records.is_empty() {
                empty_in_a_row += 1;
                if empty_in_a_row > MOST_EMPTY_IN_A_ROW {
                    // A long gap: jump to the next period anything ended in.
                    let next = self.open.keys().next().copied().unwrap_or(edge);
                    from = next.min(edge).max(from + self.width);
                    continue;
                }
            } else {
                empty_in_a_row = 0;
            }
            self.closed.push(ClosedPeriod {
                from,
                to: from + self.width,
                records,
            });
            from += self.width;
        }
        if advanced {
            self.closed_through = Some(from);
        }
    }

    /// Take what is closed and waiting to be written.
    pub fn take_closed(&mut self) -> Vec<ClosedPeriod> {
        std::mem::take(&mut self.closed)
    }

    /// Put back what could not be written, ahead of anything closed since.
    pub fn put_back(&mut self, mut unwritten: Vec<ClosedPeriod>) {
        unwritten.append(&mut self.closed);
        self.closed = unwritten;
    }
}

/// The fold as a projector output: applied per event, written at each flush.
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

    /// Load the saved state, and answer the position it was folded through —
    /// where the projector has to resume from when its checkpoint is ahead.
    /// A state saved for another width is not this fold's and starts afresh.
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
        let closed = self.fold.lock().await.take_closed();
        let mut pending = closed.into_iter();
        while let Some(period) = pending.next() {
            if let Err(error) = self
                .store
                .write(
                    period.from,
                    period.to,
                    &period.records,
                    time::OffsetDateTime::now_utc().unix_timestamp(),
                )
                .await
            {
                tracing::warn!(%error, from = period.from, "an observed period could not be written; holding the checkpoint");
                let mut unwritten = vec![period];
                unwritten.extend(pending);
                self.fold.lock().await.put_back(unwritten);
                return false;
            }
        }
        let mut saved_at = self.saved_at.lock().await;
        if force || saved_at.is_none_or(|at| at.elapsed() >= SAVE_EVERY) {
            let state = self.fold.lock().await.clone();
            if !state.closed.is_empty() {
                return true;
            }
            match self.store.save_fold(&self.processor_id, &state).await {
                Ok(()) => *saved_at = Some(Instant::now()),
                Err(error) => {
                    tracing::warn!(%error, "the observation fold's state could not be saved");
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use aiwatcher_core::{EventEnvelope, EventType, Sdk, Source};

    use super::*;

    const HOUR: time::OffsetDateTime = datetime!(2026-09-13 09:00:00 UTC);

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
            let at = HOUR + time::Duration::seconds(seconds);
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

    fn folded(events: &[RecordedEvent]) -> (PeriodFold, Vec<ClosedPeriod>) {
        let mut fold = PeriodFold::new(3_600);
        let mut closed = Vec::new();
        for event in events {
            fold.apply(event);
            closed.extend(fold.take_closed());
        }
        (fold, closed)
    }

    #[test]
    fn a_period_closes_once_the_log_s_clock_has_passed_it_and_holds_its_runs() {
        let mut log = Log::new();
        log.served("r1", 60, 62).served("r2", 120, 125);
        // Nothing has passed the hour yet.
        assert!(folded(&log.events).1.is_empty());
        log.served("r3", 3_600 + 400, 3_600 + 401);

        let (_, closed) = folded(&log.events);

        assert_eq!(closed.len(), 1);
        let record = &closed[0].records[0];
        assert_eq!(
            (record.runs, record.late_runs, record.complete),
            (2, 0, true)
        );
        assert_eq!(record.run_ms.count, 2);
        assert_eq!(record.call_ms.count, 2);
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
    fn a_run_ending_in_a_written_period_is_counted_late_in_the_oldest_open_one() {
        let mut log = Log::new();
        log.served("r1", 60, 62)
            .served("r2", 3_600 + 400, 3_600 + 401);
        // Its end arrives after the first hour closed, dated inside it.
        log.at("late", EventType::RunStarted, 100, serde_json::json!({}))
            .at("late", EventType::RunCompleted, 200, serde_json::json!({}))
            .served("r3", 7_200 + 400, 7_200 + 401);

        let (_, closed) = folded(&log.events);

        assert_eq!(closed.len(), 2);
        assert_eq!(
            (closed[0].records[0].runs, closed[0].records[0].late_runs),
            (1, 0)
        );
        assert_eq!(
            (closed[1].records[0].runs, closed[1].records[0].late_runs),
            (2, 1),
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
        let (_, whole) = folded(&log.events);

        // Saved after the first six events — r2 in flight — then a restart that
        // is handed the log again from the start, as a replay would be.
        let (saved, before) = folded(&log.events[..6]);
        let bytes = serde_json::to_vec(&saved).expect("saves");
        let mut resumed: PeriodFold = serde_json::from_slice(&bytes).expect("loads");
        let mut after = Vec::new();
        for event in &log.events {
            resumed.apply(event);
            after.extend(resumed.take_closed());
        }

        let reread: Vec<ClosedPeriod> = before.into_iter().chain(after).collect();
        assert_eq!(reread, whole);
        assert_eq!(whole[1].records[0].runs, 2, "r2 and r3, once each");
    }

    #[test]
    fn a_run_whose_start_the_fold_never_saw_is_counted_without_a_duration_and_says_so() {
        let mut log = Log::new();
        log.at("before", EventType::RunCompleted, 30, serde_json::json!({}))
            .served("r1", 60, 62)
            .served("r2", 3_600 + 400, 3_600 + 401);

        let (_, closed) = folded(&log.events);

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
        skewed.metadata.occurred_at = HOUR + time::Duration::days(1);
        let mut events = log.events.clone();
        events.push(skewed);

        assert!(
            folded(&events).1.is_empty(),
            "ingested at nine, whatever it says"
        );
    }
}
