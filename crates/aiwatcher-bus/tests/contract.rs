// `clippy.toml`'s `allow-expect-in-tests` only reaches `#[cfg(test)]` modules,
// not files under `tests/`. An assertion that panics is the point here.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! One contract, run against every adapter.
//!
//! The adapters are interchangeable only if they behave identically, and the
//! only way to keep that true is to test them with the same code. Each test
//! below runs against the in-memory bus, the write-ahead log and the Laser
//! adapter over a fake client.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use tokio::sync::Mutex;

use aiwatcher_bus::adapters::broker::{BrokerBus, BrokerClient, BrokerRecord};
use aiwatcher_bus::adapters::memory::InMemoryBus;
use aiwatcher_bus::adapters::wal::FileWal;
use aiwatcher_bus::{
    Checkpointer, MessageSink, MessageSource, SourceMessage, StartFrom, SubscribeOptions,
};
use aiwatcher_core::{Checkpoint, EventEnvelope, EventType, Sdk, Source, StreamName};

fn envelope(event_type: EventType, run_id: &str) -> EventEnvelope {
    EventEnvelope::new(
        event_type,
        run_id,
        time::OffsetDateTime::now_utc(),
        Source::new("test-service", Sdk::Rust),
    )
}

/// An in-process stand-in for a broker.
///
/// Deliberately not clever: an append-only `Vec` with string cursors. It exists
/// to prove the adapter's ordering and resume logic, not to simulate any real
/// broker's internals.
#[derive(Debug, Default)]
struct FakeBroker {
    partitions: Mutex<HashMap<String, Vec<Vec<u8>>>>,
    /// Global publish order, which is what `poll` walks.
    log: Mutex<Vec<Vec<u8>>>,
    commits: Mutex<HashMap<String, String>>,
}

#[async_trait]
impl BrokerClient for FakeBroker {
    async fn publish(
        &self,
        _topic: &str,
        partition_key: &str,
        payloads: Vec<Vec<u8>>,
    ) -> Result<(), String> {
        let mut partitions = self.partitions.lock().await;
        let mut log = self.log.lock().await;
        for payload in payloads {
            partitions
                .entry(partition_key.to_owned())
                .or_default()
                .push(payload.clone());
            log.push(payload);
        }
        Ok(())
    }

    async fn poll(
        &self,
        _topic: &str,
        _group: &str,
        cursor: Option<&str>,
        max: usize,
    ) -> Result<Vec<BrokerRecord>, String> {
        let after: usize = cursor
            .and_then(|raw| raw.trim_start_matches('0').parse().ok())
            .unwrap_or(0);
        let log = self.log.lock().await;
        Ok(log
            .iter()
            .enumerate()
            .skip(after)
            .take(max)
            .map(|(index, payload)| BrokerRecord {
                cursor: Checkpoint::from_global_position(index as u64 + 1).to_string(),
                payload: payload.clone(),
            })
            .collect())
    }

    async fn commit(&self, _topic: &str, group: &str, cursor: &str) -> Result<(), String> {
        self.commits
            .lock()
            .await
            .insert(group.to_owned(), cursor.to_owned());
        Ok(())
    }

    async fn head(&self, _topic: &str) -> Result<Option<String>, String> {
        let log = self.log.lock().await;
        Ok((!log.is_empty())
            .then(|| Checkpoint::from_global_position(log.len() as u64).to_string()))
    }
}

/// Collect messages until a `CaughtUp` arrives, then return what came before it.
async fn drain_until_caught_up(
    stream: &mut futures::stream::BoxStream<'static, SourceMessage>,
) -> Vec<SourceMessage> {
    let mut out = Vec::new();
    while let Some(message) = stream.next().await {
        let done = matches!(message, SourceMessage::CaughtUp { .. });
        out.push(message);
        if done {
            break;
        }
    }
    out
}

async fn assert_appends_assign_ascending_positions(bus: &dyn MessageSink) {
    let result = bus
        .append(vec![
            envelope(EventType::RunStarted, "run-a"),
            envelope(EventType::LlmStarted, "run-a"),
            envelope(EventType::RunStarted, "run-b"),
        ])
        .await
        .expect("append succeeds");

    let positions: Vec<u64> = result
        .recorded
        .iter()
        .map(|event| event.metadata.global_position)
        .collect();
    assert_eq!(positions, vec![1, 2, 3], "global positions are dense");

    // Stream positions restart per run — that is what makes them useful.
    assert_eq!(result.recorded[0].metadata.stream_position, 1);
    assert_eq!(result.recorded[1].metadata.stream_position, 2);
    assert_eq!(result.recorded[2].metadata.stream_position, 1);
    assert_eq!(
        result.recorded[2].metadata.stream_name,
        StreamName::for_run("run-b")
    );
}

async fn assert_replay_then_catch_up_then_live<B>(bus: &B)
where
    B: MessageSink + MessageSource,
{
    bus.append(vec![
        envelope(EventType::RunStarted, "run-1"),
        envelope(EventType::LlmStarted, "run-1"),
    ])
    .await
    .expect("backlog written");

    let mut stream = bus
        .subscribe(SubscribeOptions::from(StartFrom::Beginning))
        .await
        .expect("subscribes");

    let replayed = drain_until_caught_up(&mut stream).await;
    assert_eq!(replayed.len(), 3, "two events then one catch-up marker");
    assert!(matches!(replayed[2], SourceMessage::CaughtUp { .. }));
    assert_eq!(
        replayed[0].as_event().expect("event").event_type.as_str(),
        "run.started"
    );

    // Anything appended after the catch-up marker arrives live.
    bus.append(vec![envelope(EventType::RunCompleted, "run-1")])
        .await
        .expect("live event written");

    let live = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
        .await
        .expect("live event arrives in time")
        .expect("stream is open");
    assert_eq!(
        live.as_event().expect("event").event_type.as_str(),
        "run.completed"
    );
}

async fn assert_resume_after_a_checkpoint_skips_what_was_seen<B>(bus: &B)
where
    B: MessageSink + MessageSource,
{
    bus.append(vec![
        envelope(EventType::RunStarted, "run-1"),
        envelope(EventType::LlmStarted, "run-1"),
        envelope(EventType::LlmCompleted, "run-1"),
    ])
    .await
    .expect("backlog written");

    let mut stream = bus
        .subscribe(SubscribeOptions::from(StartFrom::After(
            Checkpoint::from_global_position(2),
        )))
        .await
        .expect("subscribes");

    let replayed = drain_until_caught_up(&mut stream).await;
    let events: Vec<_> = replayed
        .iter()
        .filter_map(SourceMessage::as_event)
        .collect();
    assert_eq!(events.len(), 1, "only what follows position 2");
    assert_eq!(events[0].metadata.global_position, 3);
}

async fn assert_a_stream_filter_isolates_one_run<B>(bus: &B)
where
    B: MessageSink + MessageSource,
{
    bus.append(vec![
        envelope(EventType::RunStarted, "run-a"),
        envelope(EventType::RunStarted, "run-b"),
        envelope(EventType::RunCompleted, "run-a"),
    ])
    .await
    .expect("backlog written");

    let mut stream = bus
        .subscribe(
            SubscribeOptions::from(StartFrom::Beginning).for_stream(StreamName::for_run("run-a")),
        )
        .await
        .expect("subscribes");

    let replayed = drain_until_caught_up(&mut stream).await;
    let events: Vec<_> = replayed
        .iter()
        .filter_map(SourceMessage::as_event)
        .collect();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.metadata.run_id == "run-a"));
}

/// Paging is a *contract*, not an adapter's private optimisation: the default
/// implementation slices a whole read, the WAL seeks by offset, and the two
/// must be indistinguishable from outside.
async fn assert_a_page_resumes_from_its_cursor_without_repeating_an_event<B>(bus: &B)
where
    B: MessageSink + MessageSource,
{
    let events: Vec<_> = (0..5)
        .map(|_| envelope(EventType::LlmStarted, "run-paged"))
        .chain(std::iter::once(envelope(
            EventType::RunStarted,
            "other-run",
        )))
        .collect();
    bus.append(events).await.expect("backlog written");
    let stream = StreamName::for_run("run-paged");

    let first = bus
        .read_stream_page(&stream, None, 2)
        .await
        .expect("reads a page");
    assert_eq!(first.events.len(), 2);
    assert!(first.has_more);
    assert_eq!(first.events[0].metadata.stream_position, 1);

    let cursor = first.next_cursor.expect("more events remain");
    let second = bus
        .read_stream_page(&stream, Some(cursor), 2)
        .await
        .expect("reads the next page");
    assert_eq!(second.events.len(), 2);
    assert_eq!(second.events[0].metadata.stream_position, 3);

    let third = bus
        .read_stream_page(&stream, second.next_cursor, 2)
        .await
        .expect("reads the last page");
    // Five events on this stream, not six: the other run's event never appears.
    assert_eq!(third.events.len(), 1);
    assert!(!third.has_more);
    assert!(third.next_cursor.is_none());
    assert!(
        third
            .events
            .iter()
            .all(|event| event.metadata.run_id == "run-paged")
    );
}

/// A stream read must cost what the *stream* holds, not what the log holds.
///
/// The WAL used to answer this by scanning every record ever written and
/// filtering. This asserts the index instead: one run's page is unaffected by
/// how much unrelated traffic sits around it.
async fn assert_a_stream_page_ignores_unrelated_traffic<B>(bus: &B)
where
    B: MessageSink + MessageSource,
{
    for index in 0..20 {
        bus.append(vec![envelope(
            EventType::LlmStarted,
            &format!("noise-{index}"),
        )])
        .await
        .expect("noise written");
        if index == 10 {
            bus.append(vec![envelope(EventType::RunStarted, "needle")])
                .await
                .expect("needle written");
        }
    }

    let page = bus
        .read_stream_page(&StreamName::for_run("needle"), None, 50)
        .await
        .expect("reads");

    assert_eq!(page.events.len(), 1);
    assert_eq!(page.events[0].metadata.run_id, "needle");
    assert!(!page.has_more);
}

async fn assert_checkpoints_round_trip(store: &dyn Checkpointer) {
    assert_eq!(store.load("projector-1").await.expect("reads"), None);
    let checkpoint = Checkpoint::from_global_position(17);
    store
        .save("projector-1", &checkpoint)
        .await
        .expect("stores");
    assert_eq!(
        store.load("projector-1").await.expect("reads"),
        Some(checkpoint)
    );
}

#[tokio::test]
async fn in_memory_bus_satisfies_the_contract() {
    assert_appends_assign_ascending_positions(&InMemoryBus::new()).await;
    assert_replay_then_catch_up_then_live(&InMemoryBus::new()).await;
    assert_resume_after_a_checkpoint_skips_what_was_seen(&InMemoryBus::new()).await;
    assert_a_stream_filter_isolates_one_run(&InMemoryBus::new()).await;
    assert_a_page_resumes_from_its_cursor_without_repeating_an_event(&InMemoryBus::new()).await;
    assert_a_stream_page_ignores_unrelated_traffic(&InMemoryBus::new()).await;
    assert_checkpoints_round_trip(&InMemoryBus::new()).await;
}

#[tokio::test]
async fn file_wal_satisfies_the_contract() {
    for case in [
        "positions",
        "replay",
        "resume",
        "filter",
        "paging",
        "isolation",
        "checkpoints",
    ] {
        let dir = tempdir(case);
        let wal = FileWal::open(&dir).await.expect("opens");
        match case {
            "positions" => assert_appends_assign_ascending_positions(&wal).await,
            "replay" => assert_replay_then_catch_up_then_live(&wal).await,
            "resume" => assert_resume_after_a_checkpoint_skips_what_was_seen(&wal).await,
            "filter" => assert_a_stream_filter_isolates_one_run(&wal).await,
            "paging" => {
                assert_a_page_resumes_from_its_cursor_without_repeating_an_event(&wal).await;
            }
            "isolation" => assert_a_stream_page_ignores_unrelated_traffic(&wal).await,
            _ => assert_checkpoints_round_trip(&wal).await,
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[tokio::test]
async fn broker_adapter_satisfies_the_read_contract() {
    let bus = BrokerBus::new(Arc::new(FakeBroker::default()), "aiwatcher.events")
        .with_poll_interval(std::time::Duration::from_millis(5));
    assert_replay_then_catch_up_then_live(&bus).await;

    let bus = BrokerBus::new(Arc::new(FakeBroker::default()), "aiwatcher.events")
        .with_poll_interval(std::time::Duration::from_millis(5));
    assert_resume_after_a_checkpoint_skips_what_was_seen(&bus).await;

    let bus = BrokerBus::new(Arc::new(FakeBroker::default()), "aiwatcher.events")
        .with_poll_interval(std::time::Duration::from_millis(5));
    assert_a_stream_filter_isolates_one_run(&bus).await;

    // The broker adapter does not override paging; this proves the default
    // implementation satisfies the same contract.
    let bus = BrokerBus::new(Arc::new(FakeBroker::default()), "aiwatcher.events")
        .with_poll_interval(std::time::Duration::from_millis(5));
    assert_a_page_resumes_from_its_cursor_without_repeating_an_event(&bus).await;
}

#[tokio::test]
async fn a_wal_survives_a_restart() {
    let dir = tempdir("restart");
    {
        let wal = FileWal::open(&dir).await.expect("opens");
        wal.append(vec![
            envelope(EventType::RunStarted, "run-1"),
            envelope(EventType::RunCompleted, "run-1"),
        ])
        .await
        .expect("writes");
        wal.save("projector", &Checkpoint::from_global_position(2))
            .await
            .expect("checkpoints");
    }

    let reopened = FileWal::open(&dir).await.expect("reopens");
    assert_eq!(
        reopened.head().await.expect("head"),
        Checkpoint::from_global_position(2)
    );
    assert_eq!(
        reopened.load("projector").await.expect("checkpoint"),
        Some(Checkpoint::from_global_position(2))
    );

    // A new append continues the numbering rather than restarting it.
    let result = reopened
        .append(vec![envelope(EventType::RunStarted, "run-2")])
        .await
        .expect("writes");
    assert_eq!(result.recorded[0].metadata.global_position, 3);
    assert_eq!(
        result.recorded[0].metadata.stream_position, 1,
        "a new run starts at stream position 1"
    );

    let events = reopened
        .read_stream(&StreamName::for_run("run-1"))
        .await
        .expect("reads a stream");
    assert_eq!(events.len(), 2);
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_wal_drops_a_torn_trailing_record_instead_of_refusing_to_open() {
    let dir = tempdir("torn");
    {
        let wal = FileWal::open(&dir).await.expect("opens");
        wal.append(vec![envelope(EventType::RunStarted, "run-1")])
            .await
            .expect("writes");
    }

    // Simulate a crash part-way through the second record.
    let path = std::path::Path::new(&dir).join("events.jsonl");
    let mut content = std::fs::read_to_string(&path).expect("reads");
    content.push_str("{\"kind\":\"Event\",\"event_ty");
    std::fs::write(&path, content).expect("writes");

    let reopened = FileWal::open(&dir)
        .await
        .expect("reopens past the torn record");
    assert_eq!(
        reopened.head().await.expect("head"),
        Checkpoint::from_global_position(1)
    );
    let result = reopened
        .append(vec![envelope(EventType::RunCompleted, "run-1")])
        .await
        .expect("writes over the truncated tail");
    assert_eq!(result.recorded[0].metadata.global_position, 2);

    let all = reopened
        .read(&Checkpoint::beginning(), 10)
        .await
        .expect("reads");
    assert_eq!(
        all.len(),
        2,
        "the torn record is gone, both good ones remain"
    );
    std::fs::remove_dir_all(&dir).ok();
}

fn tempdir(label: &str) -> String {
    let path = std::env::temp_dir().join(format!(
        "aiwatcher-wal-{label}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&path).expect("creates a temp dir");
    path.to_string_lossy().into_owned()
}
