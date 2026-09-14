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
//! than something wrong. The read model holds one copy, gone with a restart
//! that does not replay; the asked index keeps another beside its reach
//! ([`MeasuredState::kept`]), which is the one a traces step reads.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

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

/// One count as it is kept in an object store: the numbers whose start
/// arrived, as runs of consecutive numbers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptCount {
    pub client: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    /// Each `[first, last]` stretch of numbers whose start arrived.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arrived: Vec<[u64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counted: Option<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub overflowed: bool,
}

/// One measurement's counts as they are kept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptMeasurement {
    pub evaluation_id: String,
    /// Where it stands in the order measurements were last heard from.
    pub heard: u64,
    pub counts: Vec<KeptCount>,
}

impl Count {
    fn stretches(&self) -> Vec<[u64; 2]> {
        let mut stretches: Vec<[u64; 2]> = Vec::new();
        for (word_at, word) in self.seen.iter().enumerate() {
            let base = u64::try_from(word_at)
                .unwrap_or(u64::MAX)
                .saturating_mul(64);
            for bit in (0..64).filter(|bit| word & (1 << bit) != 0) {
                let number = base + bit;
                match stretches.last_mut() {
                    Some(last) if last[1] + 1 == number => last[1] = number,
                    _ => stretches.push([number, number]),
                }
            }
        }
        stretches
    }

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

    /// Every count, to be written down and read back with [`Self::from_kept`].
    #[must_use]
    pub fn kept(&self) -> Vec<KeptMeasurement> {
        self.measurements
            .iter()
            .map(|(evaluation_id, measurement)| KeptMeasurement {
                evaluation_id: evaluation_id.clone(),
                heard: measurement.heard,
                counts: measurement
                    .counts
                    .iter()
                    .map(|((client, attempt), count)| KeptCount {
                        client: client.clone(),
                        attempt: *attempt,
                        arrived: count.stretches(),
                        counted: count.counted,
                        overflowed: count.overflowed,
                    })
                    .collect(),
            })
            .collect()
    }

    /// The counts written down by [`Self::kept`], within this fold's bounds.
    #[must_use]
    pub fn from_kept(kept: Vec<KeptMeasurement>) -> Self {
        let mut state = Self::default();
        let mut kept = kept;
        // The ones heard from last are the ones kept past the bound.
        kept.sort_by_key(|measurement| std::cmp::Reverse(measurement.heard));
        for measurement in kept.into_iter().take(MOST_MEASUREMENTS) {
            state.heard = state.heard.max(measurement.heard);
            let mut restored = Measurement {
                heard: measurement.heard,
                counts: BTreeMap::new(),
            };
            for kept_count in measurement.counts.into_iter().take(MOST_COUNTS) {
                let mut count = Count {
                    counted: kept_count.counted,
                    overflowed: kept_count.overflowed,
                    ..Count::default()
                };
                for [first, last] in kept_count.arrived {
                    if first > last || last >= MOST_NUMBERS {
                        count.overflowed = true;
                        continue;
                    }
                    for number in first..=last {
                        count.started(number);
                    }
                }
                restored
                    .counts
                    .insert((kept_count.client, kept_count.attempt), count);
            }
            state
                .measurements
                .insert(measurement.evaluation_id, restored);
        }
        state
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

        let kept = state.kept();
        assert_eq!(
            kept[0].counts[1].arrived,
            [[0, 0], [2, 2]],
            "kept as the stretches that arrived"
        );
        let restored = MeasuredState::from_kept(
            serde_json::from_slice(&serde_json::to_vec(&kept).expect("writes")).expect("reads"),
        );
        assert_eq!(restored.of("e1"), state.of("e1"));
        assert_eq!(restored.of("e2"), state.of("e2"));
    }
}
