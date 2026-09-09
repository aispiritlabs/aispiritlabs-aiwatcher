//! What this deployment is actually doing, as numbers somebody can graph.
//!
//! Two readings, together because they are the same kind of question: how far
//! behind is this, and how much is piling up.
//!
//! **How late is a scheduled run, and how many were due at once?** Lateness is
//! measured from the slot to the moment the tick *found* it, never to the
//! moment the run started — the second folds the store's latency and the
//! compiler's into a number about the clock. It rides with the backlog from the
//! same tick, because one slot four minutes behind and forty of them are the
//! same lateness and very different news.
//!
//! **How much is in the object store that nothing will delete?** Workflow
//! retention prunes executions and does not follow their artifacts. Whether an
//! object is still *reachable* is a question about streams retention has been
//! deleting, so this counts what is there and says nothing about what should
//! be; the curve over weeks is what decides whether a collector is worth
//! building.
//!
//! Staging is **not** here. A notebook's staged rows live in
//! `services/ml_pipeline`, in that process's own scratch directory, keyed by a
//! hash of the context — this crate cannot list that disk, and a zero reported
//! from here would be worse than no number. `GET /ml-pipeline/staging` answers
//! for it.

use std::sync::Arc;

use aiwatcher_core::ports::{MetricKind, MetricSample, MetricSink, attr};
use aiwatcher_core::prompts::ObjectStore;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;

/// How late one slot was, and how many the tick found with it.
///
/// Both from the same tick, because reading either alone misleads: one slot
/// four minutes late is a tick that was busy, and forty slots four minutes late
/// is an instance that has fallen behind. The count is per tick rather than
/// cumulative — a backlog is a depth, not a total.
pub fn slot_samples(
    definition: &str,
    kind: &str,
    slot: OffsetDateTime,
    found_at: OffsetDateTime,
    due_this_tick: usize,
) -> Vec<MetricSample> {
    // Clamped at zero rather than allowed negative. A slot found before it was
    // due means the two clocks disagree, which is a fact about the host and not
    // about the scheduler — and a negative lateness would poison the histogram
    // for the question it exists to answer.
    let lateness = (found_at - slot).as_seconds_f64().max(0.0);
    let where_from = vec![
        attr("definition", definition.to_owned()),
        attr("definition_kind", kind.to_owned()),
    ];
    vec![
        MetricSample {
            name: "aiwatcher.schedule.slot.lateness".to_owned(),
            kind: MetricKind::Histogram,
            value: lateness,
            unit: Some("s".to_owned()),
            at: found_at,
            attributes: where_from.clone(),
        },
        MetricSample {
            name: "aiwatcher.schedule.slots.due".to_owned(),
            kind: MetricKind::Gauge,
            value: due_this_tick as f64,
            unit: None,
            at: found_at,
            attributes: where_from,
        },
    ]
}

/// How often the object store is walked for what retention leaves behind.
///
/// An hour. This is a `list` over a whole prefix, which is the one operation
/// here that costs money on a real bucket, and the number it answers moves over
/// days rather than minutes. A deployment that wants it finer can afford to ask
/// again; one that wants it cheaper turns the sink off.
const SWEEP: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// What one walk of the artifact prefix found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stored {
    pub objects: u64,
    pub bytes: u64,
    /// The oldest thing still there, in whole days. `None` when nothing has a
    /// modification time — the filesystem adapter has one, and a bucket may not
    /// report one for every key.
    pub oldest_days: Option<u64>,
}

/// Walk the artifact prefix and say what is in it.
///
/// Deliberately a count and not an opinion. Whether an object is *reachable* is
/// a question about every execution's stream, which retention has already been
/// deleting — so the honest thing this can report is what is there, and the
/// shape of the curve over weeks is what says whether a collector is worth
/// building.
pub fn summarise(entries: &[aiwatcher_core::prompts::ObjectEntry], now: OffsetDateTime) -> Stored {
    Stored {
        objects: entries.len() as u64,
        bytes: entries.iter().map(|entry| entry.size).sum(),
        oldest_days: entries
            .iter()
            .filter_map(|entry| entry.last_modified)
            .map(|at| (now - at).whole_days().max(0).unsigned_abs())
            .max(),
    }
}

impl Stored {
    fn samples(self, at: OffsetDateTime) -> Vec<MetricSample> {
        let gauge = |name: &str, value: f64| MetricSample {
            name: name.to_owned(),
            kind: MetricKind::Gauge,
            value,
            unit: None,
            at,
            attributes: vec![attr("prefix", crate::execution::artifacts::PREFIX)],
        };
        let mut samples = vec![
            gauge("aiwatcher.artifacts.objects", self.objects as f64),
            gauge("aiwatcher.artifacts.bytes", self.bytes as f64),
        ];
        // Absent rather than zero. "Nothing here is older than today" and "no
        // key reports a date" are different states, and a zero says the first
        // when the store meant the second.
        if let Some(days) = self.oldest_days {
            samples.push(gauge("aiwatcher.artifacts.oldest_days", days as f64));
        }
        samples
    }
}

/// Report what the object store holds, every hour, until shutdown.
///
/// Spawned only where both an object store and a metric sink exist. Neither is
/// required to run aiwatcher, and a deployment with no sink would be paying for
/// a full `list` to throw the answer away.
pub fn spawn_storage_sweep(
    store: Arc<dyn ObjectStore>,
    metrics: Arc<dyn MetricSink>,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tracing::info!(
        seconds = SWEEP.as_secs(),
        "measuring what the artifact prefix holds"
    );
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(SWEEP) => {}
            }
            let prefix = format!("{}/", crate::execution::artifacts::PREFIX);
            let entries = match store.list(&prefix).await {
                Ok(entries) => entries,
                Err(error) => {
                    // A measurement that could not be taken is not a failure of
                    // anything this instance is doing. Said once and dropped.
                    tracing::warn!(%error, "the artifact prefix could not be walked");
                    continue;
                }
            };
            let now = OffsetDateTime::now_utc();
            let stored = summarise(&entries, now);
            tracing::debug!(
                objects = stored.objects,
                bytes = stored.bytes,
                oldest_days = stored.oldest_days,
                "the artifact prefix, measured"
            );
            if let Err(error) = metrics.record(stored.samples(now)).await {
                tracing::warn!(%error, "the storage measurement could not be reported");
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aiwatcher_core::prompts::ObjectEntry;
    use time::Duration;

    const NOW: OffsetDateTime = OffsetDateTime::UNIX_EPOCH;

    fn entry(key: &str, size: u64, modified: Option<OffsetDateTime>) -> ObjectEntry {
        ObjectEntry {
            key: key.to_owned(),
            size,
            last_modified: modified,
        }
    }

    #[test]
    fn lateness_is_measured_to_the_tick_that_found_the_slot() {
        let samples = slot_samples(
            "curation/pii",
            "curation_pipeline",
            NOW,
            NOW + Duration::minutes(4),
            1,
        );
        assert_eq!(samples[0].name, "aiwatcher.schedule.slot.lateness");
        assert_eq!(samples[0].value, 240.0);
        assert_eq!(samples[0].unit.as_deref(), Some("s"));
    }

    #[test]
    fn a_slot_found_before_it_was_due_reports_no_lateness_rather_than_negative() {
        // Two clocks disagreeing is a fact about the host. A negative
        // observation would poison the histogram for the question it is for.
        let samples = slot_samples("d", "workflow", NOW, NOW - Duration::seconds(30), 1);
        assert_eq!(samples[0].value, 0.0);
    }

    #[test]
    fn the_backlog_is_a_depth_rather_than_a_total() {
        // One slot four minutes late is a busy tick; forty is an instance that
        // has fallen behind. Reading the lateness without the count cannot tell
        // those apart, so both come from the same tick.
        let samples = slot_samples("d", "workflow", NOW, NOW + Duration::minutes(4), 40);
        let due = &samples[1];
        assert_eq!(due.name, "aiwatcher.schedule.slots.due");
        assert_eq!(due.kind, MetricKind::Gauge);
        assert_eq!(due.value, 40.0);
    }

    #[test]
    fn a_walk_counts_what_is_there_and_how_old_the_oldest_is() {
        let stored = summarise(
            &[
                entry(
                    "artifacts/rows/ab/x/data",
                    100,
                    Some(NOW - Duration::days(9)),
                ),
                entry(
                    "artifacts/rows/cd/y/data",
                    20,
                    Some(NOW - Duration::days(2)),
                ),
            ],
            NOW,
        );
        assert_eq!(
            stored,
            Stored {
                objects: 2,
                bytes: 120,
                oldest_days: Some(9),
            }
        );
    }

    #[test]
    fn a_store_that_dates_nothing_reports_no_age_rather_than_a_zero() {
        // "Nothing here is older than today" and "no key reports a date" are
        // different states, and a zero says the first when the store meant the
        // second.
        let stored = summarise(&[entry("artifacts/rows/ab/x/data", 1, None)], NOW);
        assert_eq!(stored.oldest_days, None);
        assert!(
            !stored
                .samples(NOW)
                .iter()
                .any(|sample| sample.name.ends_with("oldest_days"))
        );
    }

    #[test]
    fn an_empty_prefix_is_a_measurement_and_not_an_absence() {
        // Zero objects is what a fresh installation reads, and the curve from
        // there is the whole point. It has to be reported.
        let stored = summarise(&[], NOW);
        let names: Vec<_> = stored
            .samples(NOW)
            .into_iter()
            .map(|sample| sample.name)
            .collect();
        assert_eq!(
            names,
            vec!["aiwatcher.artifacts.objects", "aiwatcher.artifacts.bytes"]
        );
    }
}
