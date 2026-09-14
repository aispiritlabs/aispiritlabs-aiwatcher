//! What clients said about the runs they opened to answer a measurement.
//!
//! A run made for a result's case (`data.evaluation_id`) is numbered by its
//! client apart from what the variant was observed doing — for that result and
//! the attempt at generating it (`data.generation_attempt`) — and a
//! `client.counted` says how many such runs the client had opened. Read
//! together, they say how many runs of an attempt never reached the log: a
//! number passed over, or one past the last start that arrived. That is what
//! tells an answer naming a run the log never received that was lost in
//! transport from one naming a run nobody opened.
//!
//! Bounded like the rest of the read model: a measurement heard from longest
//! ago is forgotten first, and a count past what this keeps says nothing rather
//! than something wrong.

use std::collections::BTreeMap;

use aiwatcher_core::{EventType, RecordedEvent};

/// Measurements whose counts are kept; the one heard from longest ago goes.
const MOST_MEASUREMENTS: usize = 1_024;
/// Clients and attempts kept for one measurement.
const MOST_COUNTS: usize = 256;
/// Run numbers one count tracks; a count numbering past it says nothing.
const MOST_NUMBERS: u64 = 1 << 20;

/// One client's count of the runs it opened for one measurement, at one
/// attempt of generating its answers, as far as the log says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeasuredRuns {
    /// The client that counted (`source.client`).
    pub client: String,
    /// The attempt of generating the answers it counted for, where it said.
    pub attempt: Option<u32>,
    /// How many runs it opened: its own count where one arrived, and never
    /// fewer than one past the highest number a start that arrived carried.
    pub opened: u64,
    /// How many of those numbers a start that arrived carried.
    pub arrived: u64,
}

#[derive(Debug, Default)]
struct Count {
    /// One bit per run number whose start arrived.
    seen: Vec<u64>,
    arrived: u64,
    highest: Option<u64>,
    counted: Option<u64>,
    /// A number past what is tracked arrived: the count says nothing.
    overflowed: bool,
}

impl Count {
    fn started(&mut self, number: u64) {
        if number >= MOST_NUMBERS {
            self.overflowed = true;
            return;
        }
        let (word, bit) = (
            usize::try_from(number / 64).unwrap_or(usize::MAX),
            number % 64,
        );
        if self.seen.len() <= word {
            self.seen.resize(word + 1, 0);
        }
        if self.seen[word] & (1 << bit) == 0 {
            self.seen[word] |= 1 << bit;
            self.arrived += 1;
        }
        self.highest = Some(self.highest.map_or(number, |highest| highest.max(number)));
    }
}

#[derive(Debug, Default)]
struct Measurement {
    heard: u64,
    counts: BTreeMap<(String, Option<u32>), Count>,
}

/// Every measurement's counts, as the log was read.
#[derive(Debug, Default)]
pub struct MeasuredState {
    measurements: BTreeMap<String, Measurement>,
    /// A clock of this fold's own: which measurement was heard from last.
    heard: u64,
}

impl MeasuredState {
    /// Read a measured run's start, or a client's count of such runs.
    pub fn apply(&mut self, event: &RecordedEvent) {
        let Some(evaluation_id) = event.data_str("evaluation_id") else {
            return;
        };
        let Some(client) = event.metadata.source.client.as_deref() else {
            return;
        };
        let attempt = event
            .data
            .get("generation_attempt")
            .and_then(serde_json::Value::as_u64)
            .and_then(|attempt| u32::try_from(attempt).ok());
        let (started, counted) = match event.event_type {
            EventType::RunStarted => match event.metadata.run_sequence {
                Some(number) => (Some(number), None),
                None => return,
            },
            EventType::ClientCounted => {
                match event.data.get("runs").and_then(serde_json::Value::as_u64) {
                    Some(runs) => (None, Some(runs)),
                    None => return,
                }
            }
            _ => return,
        };
        self.heard += 1;
        if !self.measurements.contains_key(evaluation_id)
            && self.measurements.len() >= MOST_MEASUREMENTS
            && let Some(oldest) = self
                .measurements
                .iter()
                .min_by_key(|(_, measurement)| measurement.heard)
                .map(|(key, _)| key.clone())
        {
            self.measurements.remove(&oldest);
        }
        let measurement = self
            .measurements
            .entry(evaluation_id.to_owned())
            .or_default();
        measurement.heard = self.heard;
        let key = (client.to_owned(), attempt);
        if !measurement.counts.contains_key(&key) && measurement.counts.len() >= MOST_COUNTS {
            return;
        }
        let count = measurement.counts.entry(key).or_default();
        if let Some(number) = started {
            count.started(number);
        }
        if let Some(runs) = counted {
            count.counted = Some(count.counted.map_or(runs, |before| before.max(runs)));
        }
    }

    /// What each client said of the runs it opened for one measurement.
    #[must_use]
    pub fn of(&self, evaluation_id: &str) -> Vec<MeasuredRuns> {
        self.measurements
            .get(evaluation_id)
            .into_iter()
            .flat_map(|measurement| &measurement.counts)
            .filter(|(_, count)| !count.overflowed)
            .map(|((client, attempt), count)| MeasuredRuns {
                client: client.clone(),
                attempt: *attempt,
                opened: count
                    .counted
                    .unwrap_or(0)
                    .max(count.highest.map_or(0, |highest| highest + 1)),
                arrived: count.arrived,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use aiwatcher_core::envelope::{EventEnvelope, Sdk, Source};
    use time::macros::datetime;

    use super::*;

    fn event(
        event_type: EventType,
        client: &str,
        number: Option<u64>,
        data: serde_json::Value,
    ) -> RecordedEvent {
        let at = datetime!(2026-09-14 09:00:00 UTC);
        let mut source = Source::new("worker", Sdk::Python);
        source.client = Some(client.to_owned());
        let mut envelope = EventEnvelope::new(event_type, "run", at, source).with_data(data);
        envelope.run_sequence = number;
        envelope.record(1, 1, at, None)
    }

    #[test]
    fn a_measurement_s_starts_and_its_client_s_count_say_how_many_of_its_runs_never_arrived() {
        let mut state = MeasuredState::default();
        let start = |client: &str, number: u64, attempt: u64| {
            event(
                EventType::RunStarted,
                client,
                Some(number),
                serde_json::json!({"evaluation_id": "e1", "generation_attempt": attempt}),
            )
        };
        for applied in [
            start("worker", 0, 1),
            start("worker", 2, 1),
            start("worker", 2, 1),
            event(
                EventType::ClientCounted,
                "worker",
                None,
                serde_json::json!({"evaluation_id": "e1", "generation_attempt": 1, "runs": 5}),
            ),
            start("worker", 0, 2),
            start("worker", 0, 1),
            start("other", 9, 1),
            event(
                EventType::RunStarted,
                "worker",
                Some(3),
                serde_json::json!({"evaluation_id": "e2"}),
            ),
            event(
                EventType::RunStarted,
                "worker",
                None,
                serde_json::json!({"evaluation_id": "e1", "generation_attempt": 1}),
            ),
        ] {
            state.apply(&applied);
        }

        assert_eq!(
            state.of("e1"),
            [
                MeasuredRuns {
                    client: "other".to_owned(),
                    attempt: Some(1),
                    opened: 10,
                    arrived: 1
                },
                MeasuredRuns {
                    client: "worker".to_owned(),
                    attempt: Some(1),
                    opened: 5,
                    arrived: 2
                },
                MeasuredRuns {
                    client: "worker".to_owned(),
                    attempt: Some(2),
                    opened: 1,
                    arrived: 1
                },
            ],
            "numbers 1, 3 and 4 of the first attempt never arrived, and a redelivery is one"
        );
        assert_eq!(state.of("e2")[0].opened, 4);
        assert!(state.of("nothing").is_empty());
    }
}
