//! What witnesses saw asked, kept past the read model, a restart and the log.
//!
//! A traces step holds an answer to calls that asked its case's question in
//! other runs — while the measurement ran, or from a moment the run pinned
//! before it. The read model answers that only for the runs it still holds, and
//! nothing across a restart that did not replay them; so each model call a
//! witness relayed is written down here as the log passes it: when it ended,
//! the run it was in and the run it served, who published it, its model and
//! prompt, and the keyed digests of what it asked — as asked and normalised —
//! and never a word of it.
//!
//! A projector output like the observed periods: pages in the object store,
//! each created once under the positions it covers, written a minute at a
//! time, and a reach saved beside them — the position written through and the
//! moment the index reads back to. A page not yet written holds the projector's
//! resume back, so a restart reads its events again. The index reads back to
//! the log's first event where it read that event, to the first event after a
//! stretch of positions it never read otherwise, and never past its retention;
//! a reader told where that is names what it could not look at.
//!
//! Beside the reach it keeps what clients counted of a measurement's runs
//! ([`crate::measured`]), so a traces step tells a run lost in transport from
//! one nobody opened after a restart as well.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use aiwatcher_core::ports::PortError;
use aiwatcher_core::prompts::PromptRef;
use aiwatcher_core::storage::ObjectStore;
use aiwatcher_core::{EventType, Phase, RecordedEvent, Subject};

use crate::measured::{KeptMeasurement, MeasuredRuns, MeasuredState};

/// The object store prefix this output owns.
pub const PREFIX: &str = "asked-index/";
/// How long calls wait in memory before a page is written, at most.
const WRITE_EVERY: Duration = Duration::from_secs(60);
/// How many calls a page holds before it is written anyway.
const MOST_PENDING: usize = 5_000;
/// Runs whose caller is remembered until they end; the oldest goes past it.
const MOST_CALLERS: usize = 50_000;
/// Reaches kept per processor; older ones are removed.
const REACHES_KEPT: usize = 3;
/// How often retention is applied.
const PRUNE_EVERY: Duration = Duration::from_secs(3_600);
const HOUR_MS: i64 = 3_600_000;

/// One model call a witness relayed and said what it asked in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskedCall {
    /// The log position of the event that ended it: what a call written twice
    /// is recognised by.
    pub position: u64,
    /// When it ended, in Unix milliseconds.
    pub at_ms: i64,
    pub run_id: String,
    /// The run whose call it served, as its run's start said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_by: Option<String>,
    /// Which project the call's run belongs to (ADR_0033), from the event that
    /// ended it. Absent is the global side, and every call written before
    /// projects existed.
    ///
    /// On the row rather than in the key: a page is named by the positions it
    /// covers, and the log is one log, so two projects' calls share a page and
    /// are told apart by this. A reader deciding what a step may look at reads
    /// it; the index itself decides nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<aiwatcher_core::ProjectScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_verified: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub asked: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub asked_normalized: Vec<String>,
}

/// The calls asked from a moment on, and how far back the index reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AskedSince {
    pub calls: Vec<AskedCall>,
    /// The moment before which the index holds nothing it could have read —
    /// `None` where it read the log from its first event and kept every page.
    pub reaches_from_ms: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Page {
    first: u64,
    last: u64,
    calls: Vec<AskedCall>,
}

/// Where the index has written through, and how far back it reads.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Reach {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    through: Option<u64>,
    /// Whether it has read any event at all: a first one sets where it began.
    #[serde(default)]
    began: bool,
    /// The moment it reads back to, where that is not the log's first event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reads_from_ms: Option<i64>,
    /// What clients counted of measurements' runs, through the same position.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    measured: Vec<KeptMeasurement>,
}

#[derive(Debug, Default)]
struct State {
    reach: Reach,
    /// Positions applied and not yet written, and the calls among them.
    applied_through: Option<u64>,
    pending: Vec<AskedCall>,
    pending_since: Option<Instant>,
    reach_saved_at: Option<Instant>,
    callers: HashMap<String, String>,
    caller_order: VecDeque<String>,
    pruned_at: Option<Instant>,
    contiguous: bool,
    measured: MeasuredState,
}

fn hex(text: &str) -> String {
    text.bytes().map(|byte| format!("{byte:02x}")).collect()
}

fn digests(event: &RecordedEvent, key: &str) -> Vec<String> {
    event
        .data
        .get(key)
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .filter(|digest| digest.len() == 32 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .take(aiwatcher_core::witness::MOST_DIGESTS)
        .map(ToOwned::to_owned)
        .collect()
}

fn storage(key: &str, error: impl std::fmt::Display) -> PortError {
    PortError::Rejected {
        target: "asked-index",
        message: format!("{key}: {error}"),
    }
}

/// The index as a projector output.
#[derive(Debug)]
pub struct AskedIndex {
    store: Arc<dyn ObjectStore>,
    processor_id: String,
    retention: Option<Duration>,
    state: Mutex<State>,
}

impl AskedIndex {
    #[must_use]
    pub fn new(store: Arc<dyn ObjectStore>, processor_id: impl Into<String>) -> Self {
        Self {
            store,
            processor_id: processor_id.into(),
            retention: None,
            state: Mutex::new(State::default()),
        }
    }

    /// The same index, removing pages older than this.
    #[must_use]
    pub fn keeping(mut self, retention: Duration) -> Self {
        self.retention = Some(retention);
        self
    }

    fn reach_folder(&self) -> String {
        format!("{PREFIX}reach/{}/", hex(&self.processor_id))
    }

    /// Whether the log this reads numbers every event one after the last.
    pub async fn reads_contiguous_positions(&self, contiguous: bool) {
        self.state.lock().await.contiguous = contiguous;
    }

    /// Load the reach saved furthest along, and answer the position written
    /// through: where the projector resumes from when its checkpoint is ahead.
    pub async fn load(&self) -> Option<u64> {
        let loaded = match self.store.list(&self.reach_folder()).await {
            Ok(entries) => {
                let mut keys: Vec<String> = entries.into_iter().map(|entry| entry.key).collect();
                keys.sort_unstable_by(|one, other| other.cmp(one));
                let mut found = None;
                for key in keys {
                    match self.store.get(&key).await {
                        Ok(Some(bytes)) => {
                            if let Ok(reach) = serde_json::from_slice::<Reach>(&bytes) {
                                found = Some(reach);
                                break;
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            tracing::warn!(%error, "the asked index's reach could not be read; starting afresh");
                            break;
                        }
                    }
                }
                found
            }
            Err(error) => {
                tracing::warn!(%error, "the asked index's reach could not be listed; starting afresh");
                None
            }
        };
        let mut state = self.state.lock().await;
        if let Some(mut reach) = loaded {
            state.applied_through = reach.through;
            state.measured = MeasuredState::from_kept(std::mem::take(&mut reach.measured));
            state.reach = reach;
        }
        state.reach.through
    }

    /// Read one event: remember which run a serving run served, and keep a
    /// call a witness said what it asked in.
    pub async fn apply(&self, event: &RecordedEvent) {
        let position = event.metadata.global_position;
        let mut state = self.state.lock().await;
        if state
            .applied_through
            .is_some_and(|through| position <= through)
        {
            return;
        }
        let at_ms = i64::try_from(event.metadata.occurred_at.unix_timestamp_nanos() / 1_000_000)
            .unwrap_or(i64::MAX);
        let passed_over = state
            .applied_through
            .is_some_and(|through| state.contiguous && position > through + 1);
        if !state.reach.began || passed_over {
            // From the log's first event nothing came before; after a stretch
            // nobody read, what did is not here.
            let from_the_first = !state.reach.began && state.contiguous && position == 1;
            if !from_the_first {
                state.reach.reads_from_ms = Some(
                    state
                        .reach
                        .reads_from_ms
                        .map_or(at_ms, |from| from.max(at_ms)),
                );
            }
            state.reach.began = true;
        }
        state.applied_through = Some(position);
        state.measured.apply(event);
        let subject = event.event_type.subject();
        let run_id = &event.metadata.run_id;
        if event.event_type == EventType::RunStarted
            && let Some(caller) = event.data_str("caller_run_id")
        {
            if state.caller_order.len() >= MOST_CALLERS
                && let Some(oldest) = state.caller_order.pop_front()
            {
                state.callers.remove(&oldest);
            }
            state.callers.insert(run_id.clone(), caller.to_owned());
            state.caller_order.push_back(run_id.clone());
        }
        if subject == Subject::Llm && matches!(event.event_type.phase(), Some(Phase::End { .. })) {
            let asked = digests(event, "asked_digests");
            let asked_normalized = digests(event, "asked_normalized_digests");
            if !asked.is_empty() || !asked_normalized.is_empty() {
                let prompt = PromptRef::from_data(&event.data);
                let call = AskedCall {
                    position,
                    at_ms,
                    run_id: run_id.clone(),
                    caller_run_id: state.callers.get(run_id).cloned(),
                    published_by: event.metadata.published_by.clone(),
                    project: event.metadata.project,
                    model: event.data_str("model").map(ToOwned::to_owned),
                    prompt_name: prompt
                        .as_ref()
                        .and_then(|prompt| prompt.name.as_ref())
                        .map(ToString::to_string),
                    prompt_version: prompt.map(|prompt| prompt.version_id.to_string()),
                    prompt_verified: event
                        .data
                        .get("prompt_verified")
                        .and_then(serde_json::Value::as_bool),
                    asked,
                    asked_normalized,
                };
                state.pending_since.get_or_insert_with(Instant::now);
                state.pending.push(call);
            }
        }
        if subject == Subject::Run
            && matches!(event.event_type.phase(), Some(Phase::End { .. }))
            && state.callers.remove(run_id).is_some()
        {
            state.caller_order.retain(|held| held != run_id);
        }
    }

    /// Write the calls waiting once they are due or `force`d, then save where
    /// the index has written through; apply retention hourly. `false` when a
    /// page could not be written: the caller holds its checkpoint back.
    pub async fn flush(&self, force: bool) -> bool {
        let (page, reach) = {
            let mut state = self.state.lock().await;
            let Some(through) = state.applied_through else {
                return true;
            };
            if state.pending.is_empty() {
                // Nothing to write; the reach still moves a minute at a time,
                // so a restart does not read back to the last page written.
                let due = force
                    || state
                        .reach_saved_at
                        .is_none_or(|at| at.elapsed() >= WRITE_EVERY);
                if !due || state.reach.through == Some(through) {
                    return true;
                }
                let mut reach = state.reach.clone();
                reach.through = Some(through);
                reach.measured = state.measured.kept();
                (None, reach)
            } else {
                let due = force
                    || state.pending.len() >= MOST_PENDING
                    || state
                        .pending_since
                        .is_some_and(|since| since.elapsed() >= WRITE_EVERY);
                if !due {
                    return true;
                }
                let calls = std::mem::take(&mut state.pending);
                state.pending_since = None;
                let first = calls.first().map_or(through, |call| call.position);
                let mut reach = state.reach.clone();
                reach.through = Some(through);
                reach.measured = state.measured.kept();
                (
                    Some(Page {
                        first,
                        last: through,
                        calls,
                    }),
                    reach,
                )
            }
        };
        if let Some(page) = &page {
            let hour = page
                .calls
                .first()
                .map_or(0, |call| call.at_ms.div_euclid(HOUR_MS))
                * 3_600;
            let key = format!(
                "{PREFIX}pages/{hour:012}/{:020}-{:020}.json",
                page.first, page.last
            );
            let written = match serde_json::to_vec(page) {
                Ok(bytes) => self.store.create(&key, bytes).await.map(|_| ()),
                Err(error) => Err(storage(&key, error)),
            };
            if let Err(error) = written {
                tracing::warn!(%error, calls = page.calls.len(), "a page of the asked index could not be written; holding the checkpoint");
                let mut state = self.state.lock().await;
                let mut calls = page.calls.clone();
                calls.append(&mut state.pending);
                state.pending = calls;
                state.pending_since.get_or_insert_with(Instant::now);
                return false;
            }
        }
        if let Some(through) = reach.through {
            match self.save(through, &reach).await {
                Ok(()) => self.state.lock().await.reach_saved_at = Some(Instant::now()),
                Err(error) => {
                    tracing::warn!(%error, "the asked index's reach could not be saved");
                }
            }
        }
        {
            let mut state = self.state.lock().await;
            state.reach.through = reach.through;
        }
        self.prune().await;
        true
    }

    async fn save(&self, through: u64, reach: &Reach) -> Result<(), PortError> {
        let folder = self.reach_folder();
        let key = format!("{folder}{through:020}.json");
        let bytes = serde_json::to_vec(reach).map_err(|error| storage(&key, error))?;
        self.store.create(&key, bytes).await?;
        let mut saved: Vec<String> = self
            .store
            .list(&folder)
            .await?
            .into_iter()
            .map(|entry| entry.key)
            .filter(|saved| *saved <= key)
            .collect();
        saved.sort_unstable_by(|one, other| other.cmp(one));
        for old in saved.iter().skip(REACHES_KEPT) {
            self.store.delete(old).await?;
        }
        Ok(())
    }

    /// Remove the pages of hours past retention, once an hour.
    async fn prune(&self) {
        let Some(retention) = self.retention else {
            return;
        };
        {
            let mut state = self.state.lock().await;
            if state.pruned_at.is_some_and(|at| at.elapsed() < PRUNE_EVERY) {
                return;
            }
            state.pruned_at = Some(Instant::now());
        }
        let now_ms =
            i64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
                .unwrap_or(i64::MAX);
        let cutoff_ms = now_ms - i64::try_from(retention.as_millis()).unwrap_or(i64::MAX);
        // Only whole hours before the cutoff go, so what is kept reaches back
        // to the start of the hour it lies in.
        let cutoff_hour = cutoff_ms.div_euclid(HOUR_MS) * 3_600;
        let keys = match self.store.list(&format!("{PREFIX}pages/")).await {
            Ok(entries) => entries,
            Err(error) => {
                tracing::warn!(%error, "the asked index's pages could not be listed for retention");
                return;
            }
        };
        let mut removed = 0usize;
        for entry in keys {
            let hour = entry
                .key
                .strip_prefix(&format!("{PREFIX}pages/"))
                .and_then(|rest| rest.split('/').next())
                .and_then(|hour| hour.parse::<i64>().ok());
            if hour.is_some_and(|hour| hour < cutoff_hour) {
                if let Err(error) = self.store.delete(&entry.key).await {
                    tracing::warn!(%error, "a page of the asked index past retention could not be removed");
                    return;
                }
                removed += 1;
            }
        }
        let mut state = self.state.lock().await;
        let kept_from = cutoff_hour * 1_000;
        if state.reach.began {
            state.reach.reads_from_ms = Some(
                state
                    .reach
                    .reads_from_ms
                    .map_or(kept_from, |from| from.max(kept_from)),
            );
        }
        if removed > 0 {
            tracing::info!(removed, "pages of the asked index past retention removed");
        }
    }

    /// What each client counted of the runs it opened for one measurement,
    /// through the position the index has read.
    ///
    /// `project` is the project whose measurement is being asked about —
    /// `None` for the global side, which is every one a production caller asks
    /// about today (ADR_0033: no production caller constructs a bound store).
    pub async fn measured_runs(
        &self,
        project: Option<aiwatcher_core::ProjectScope>,
        evaluation_id: &str,
    ) -> Vec<MeasuredRuns> {
        self.state.lock().await.measured.of(project, evaluation_id)
    }

    /// Every call a witness said what it asked in that ended at or after
    /// `from_ms`, whichever page or memory holds it, each once — and how far
    /// back the index reads.
    ///
    /// # Errors
    ///
    /// The store's own failure, or a page that no longer reads.
    pub async fn since(&self, from_ms: i64) -> Result<AskedSince, PortError> {
        let (pending, reaches_from_ms) = {
            let state = self.state.lock().await;
            (state.pending.clone(), state.reach.reads_from_ms)
        };
        let now_ms =
            i64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
                .unwrap_or(i64::MAX);
        let start = from_ms.max(reaches_from_ms.unwrap_or(i64::MIN));
        let mut calls: BTreeMap<u64, AskedCall> = BTreeMap::new();
        // A page is filed under the hour its first call ended in, and runs on
        // at most a minute past it.
        let mut hour = start.div_euclid(HOUR_MS) - 1;
        while hour <= now_ms.div_euclid(HOUR_MS) {
            let folder = format!("{PREFIX}pages/{:012}/", hour * 3_600);
            for entry in self.store.list(&folder).await? {
                let Some(bytes) = self.store.get(&entry.key).await? else {
                    continue;
                };
                let page: Page =
                    serde_json::from_slice(&bytes).map_err(|error| storage(&entry.key, error))?;
                for call in page.calls {
                    if call.at_ms >= from_ms {
                        calls.entry(call.position).or_insert(call);
                    }
                }
            }
            hour += 1;
        }
        for call in pending {
            if call.at_ms >= from_ms {
                calls.entry(call.position).or_insert(call);
            }
        }
        Ok(AskedSince {
            calls: calls.into_values().collect(),
            reaches_from_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use aiwatcher_core::envelope::{EventEnvelope, Sdk, Source};
    use time::macros::datetime;

    use super::*;
    use crate::periods::tests::MemoryObjectStore;

    fn event(
        event_type: EventType,
        run_id: &str,
        position: u64,
        seconds: i64,
        data: serde_json::Value,
    ) -> RecordedEvent {
        let at = datetime!(2026-09-14 09:00:00 UTC) + time::Duration::seconds(seconds);
        let mut recorded =
            EventEnvelope::new(event_type, run_id, at, Source::new("gateway", Sdk::Python))
                .with_data(data)
                .record(position, position, at, None);
        recorded.metadata.published_by = Some("gateway".to_owned());
        recorded
    }

    fn relayed(
        run_id: &str,
        caller: &str,
        first: u64,
        seconds: i64,
        asked: &str,
    ) -> Vec<RecordedEvent> {
        let prompt = serde_json::json!({
            "model": "gpt-4o", "prompt_name": "capitals", "prompt_version": "f".repeat(64),
            "prompt_verified": true, "call_id": "c"
        });
        let mut completed = prompt.clone();
        completed["asked_digests"] = serde_json::json!([asked]);
        completed["asked_normalized_digests"] = serde_json::json!(["b".repeat(32)]);
        vec![
            event(
                EventType::RunStarted,
                run_id,
                first,
                seconds,
                serde_json::json!({"caller_run_id": caller}),
            ),
            event(EventType::LlmStarted, run_id, first + 1, seconds, prompt),
            event(
                EventType::LlmCompleted,
                run_id,
                first + 2,
                seconds,
                completed,
            ),
            event(
                EventType::RunCompleted,
                run_id,
                first + 3,
                seconds,
                serde_json::json!({}),
            ),
        ]
    }

    #[tokio::test]
    async fn a_call_a_witness_relayed_is_read_back_after_a_restart_from_the_moment_asked_for() {
        let store: Arc<dyn ObjectStore> = Arc::new(MemoryObjectStore::default());
        let index = AskedIndex::new(Arc::clone(&store), "projector");
        index.reads_contiguous_positions(true).await;
        for recorded in relayed("gateway-1", "case-1", 1, 0, &"a".repeat(32))
            .into_iter()
            .chain(relayed("gateway-2", "case-2", 5, 600, &"c".repeat(32)))
        {
            index.apply(&recorded).await;
        }
        assert!(
            index.flush(false).await,
            "nothing due is nothing written, and nothing held back"
        );
        let held = index.since(0).await.expect("reads");
        assert_eq!(
            held.calls.len(),
            2,
            "not yet written, still read from memory"
        );
        assert!(index.flush(true).await);

        let restarted = AskedIndex::new(Arc::clone(&store), "projector");
        restarted.reads_contiguous_positions(true).await;
        assert_eq!(restarted.load().await, Some(8));
        // A replay from the log's start reads nothing twice.
        for recorded in relayed("gateway-1", "case-1", 1, 0, &"a".repeat(32)) {
            restarted.apply(&recorded).await;
        }
        let from = datetime!(2026-09-14 09:05:00 UTC).unix_timestamp() * 1_000;
        let read = restarted.since(from).await.expect("reads");
        assert_eq!(
            read.reaches_from_ms, None,
            "it read the log from its first event"
        );
        assert_eq!(
            read.calls,
            [AskedCall {
                position: 7,
                at_ms: datetime!(2026-09-14 09:10:00 UTC).unix_timestamp() * 1_000,
                run_id: "gateway-2".to_owned(),
                caller_run_id: Some("case-2".to_owned()),
                published_by: Some("gateway".to_owned()),
                project: None,
                model: Some("gpt-4o".to_owned()),
                prompt_name: Some("capitals".to_owned()),
                prompt_version: Some("f".repeat(64)),
                prompt_verified: Some(true),
                asked: vec!["c".repeat(32)],
                asked_normalized: vec!["b".repeat(32)],
            }]
        );
        assert_eq!(restarted.since(0).await.expect("reads").calls.len(), 2);
    }

    #[tokio::test]
    async fn a_client_s_count_of_a_measurement_s_runs_is_read_back_after_a_restart() {
        let store: Arc<dyn ObjectStore> = Arc::new(MemoryObjectStore::default());
        let index = AskedIndex::new(Arc::clone(&store), "projector");
        index.reads_contiguous_positions(true).await;
        let measured = |event_type, position, data: serde_json::Value| {
            let started = event_type == EventType::RunStarted;
            let mut recorded = event(event_type, "case-run", position, 0, data);
            recorded.metadata.source.client = Some("worker".to_owned());
            if started {
                recorded.metadata.run_sequence = Some(position - 1);
            }
            recorded
        };
        for recorded in [
            measured(
                EventType::RunStarted,
                1,
                serde_json::json!({"evaluation_id": "e1", "generation_attempt": 1}),
            ),
            measured(
                EventType::ClientCounted,
                2,
                serde_json::json!({"evaluation_id": "e1", "generation_attempt": 1, "runs": 3}),
            ),
        ] {
            index.apply(&recorded).await;
        }
        assert!(index.flush(true).await);

        let restarted = AskedIndex::new(Arc::clone(&store), "projector");
        assert_eq!(restarted.load().await, Some(2));
        assert_eq!(
            restarted.measured_runs(None, "e1").await,
            [MeasuredRuns {
                client: "worker".to_owned(),
                attempt: Some(1),
                opened: 3,
                arrived: 1,
            }],
            "two of three runs never arrived, whether or not the read model replays"
        );
    }

    #[tokio::test]
    async fn pages_past_retention_go_and_the_index_says_it_reads_back_no_further() {
        let store: Arc<dyn ObjectStore> = Arc::new(MemoryObjectStore::default());
        let index =
            AskedIndex::new(Arc::clone(&store), "projector").keeping(Duration::from_secs(86_400));
        index.reads_contiguous_positions(true).await;
        // Written a year before these tests were, so past a day whenever they run.
        for recorded in relayed("gateway-1", "case-1", 1, -365 * 86_400, &"a".repeat(32)) {
            index.apply(&recorded).await;
        }
        assert!(index.flush(true).await);
        let read = index.since(0).await.expect("reads");
        assert!(read.calls.is_empty(), "the page past a day is gone");
        let now_ms =
            i64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
                .expect("a moment");
        assert!(
            read.reaches_from_ms.is_some_and(
                |from| from > now_ms - 2 * 86_400_000 && from <= now_ms - 86_400_000 + HOUR_MS
            ),
            "it reads back to the start of the hour a day ago: {:?}",
            read.reaches_from_ms
        );
    }

    #[tokio::test]
    async fn an_index_that_began_past_the_log_s_first_event_says_where_it_reads_back_to() {
        let store: Arc<dyn ObjectStore> = Arc::new(MemoryObjectStore::default());
        let index = AskedIndex::new(Arc::clone(&store), "projector");
        index.reads_contiguous_positions(true).await;
        for recorded in relayed("gateway-1", "case-1", 40, 60, &"a".repeat(32)) {
            index.apply(&recorded).await;
        }
        // Positions 44 to 99 were never read.
        for recorded in relayed("gateway-2", "case-2", 100, 900, &"c".repeat(32)) {
            index.apply(&recorded).await;
        }
        let read = index.since(0).await.expect("reads");
        assert_eq!(
            read.reaches_from_ms,
            Some(datetime!(2026-09-14 09:15:00 UTC).unix_timestamp() * 1_000),
            "after a stretch nobody read, it reads back only to the event after it"
        );
        assert_eq!(read.calls.len(), 2, "what it holds, it still answers");
    }
}
