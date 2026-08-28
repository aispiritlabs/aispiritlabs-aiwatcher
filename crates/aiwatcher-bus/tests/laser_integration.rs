// See the note in contract.rs.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
#![cfg(feature = "laser")]

//! The Laser adapter against a real Apache Iggy broker.
//!
//! Ignored by default — it needs a broker. Run it with:
//!
//! ```text
//! just iggy-up
//! just test-laser
//! ```
//!
//! `AIWATCHER_LASER_CONNECTION_STRING` overrides the address, so the same test
//! runs against the Tilt cluster's Iggy via a port-forward.
//!
//! These are the assertions the in-process fake cannot make: that the offsets
//! Iggy hands back really do line up with the positions the adapter stamps,
//! that a consumer group really does resume where it committed, and that a
//! partition key really does keep one run's events in order.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;

use aiwatcher_bus::adapters::laser::{LaserBus, LaserConfig};
use aiwatcher_bus::{
    Checkpointer, MessageSink, MessageSource, SourceMessage, StartFrom, SubscribeOptions,
};
use aiwatcher_core::{Checkpoint, EventEnvelope, EventType, MessageId, Sdk, Source, StreamName};

fn connection_string() -> String {
    std::env::var("AIWATCHER_LASER_CONNECTION_STRING")
        .unwrap_or_else(|_| "iggy:iggy@127.0.0.1:8090".to_owned())
}

/// A fresh topic per test, so runs do not see each other's records.
async fn bus(label: &str) -> Arc<LaserBus> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    Arc::new(
        LaserBus::connect(LaserConfig {
            connection_string: connection_string(),
            stream: "aiwatcher-test".to_owned(),
            topic: format!("{label}-{nanos}"),
            partitions: 1,
            batch_length: 64,
            connect_timeout: Duration::from_secs(10),
        })
        .await
        .expect("connects to Iggy — is one running? `just iggy-up`"),
    )
}

fn envelope(event_id: &str, event_type: EventType, run_id: &str) -> EventEnvelope {
    let mut envelope = EventEnvelope::new(
        event_type,
        run_id,
        time::OffsetDateTime::now_utc(),
        Source::new("laser-integration-test", Sdk::Rust),
    );
    envelope.event_id = Some(MessageId::new(event_id));
    envelope.agent_id = Some("researcher".to_owned());
    envelope
}

/// Read until the catch-up marker, with a deadline so a hang fails loudly.
async fn drain_until_caught_up(
    stream: &mut futures::stream::BoxStream<'static, SourceMessage>,
) -> Vec<SourceMessage> {
    let mut out = Vec::new();
    loop {
        let message = tokio::time::timeout(Duration::from_secs(15), stream.next())
            .await
            .expect("the subscription produced a message in time")
            .expect("the subscription is open");
        let done = matches!(message, SourceMessage::CaughtUp { .. });
        out.push(message);
        if done {
            return out;
        }
    }
}

#[tokio::test]
#[ignore = "needs a running Apache Iggy; `just iggy-up` then `just test-laser`"]
async fn a_published_run_comes_back_with_broker_assigned_positions() {
    let bus = bus("positions").await;
    bus.append(vec![
        envelope("e1", EventType::RunStarted, "run-1"),
        envelope("e2", EventType::LlmStarted, "run-1"),
        envelope("e3", EventType::RunCompleted, "run-1"),
    ])
    .await
    .expect("publishes");

    let mut stream = bus
        .subscribe(SubscribeOptions::from(StartFrom::Beginning))
        .await
        .expect("subscribes");
    let messages = drain_until_caught_up(&mut stream).await;
    let events: Vec<_> = messages
        .iter()
        .filter_map(SourceMessage::as_event)
        .collect();

    assert_eq!(events.len(), 3, "all three records came back");
    assert_eq!(
        events
            .iter()
            .map(|event| event.metadata.global_position)
            .collect::<Vec<_>>(),
        vec![1, 2, 3],
        "positions are the Iggy offsets, one-based"
    );
    assert_eq!(
        events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec!["run.started", "llm.started", "run.completed"],
        "the partition key kept one run's events in publish order"
    );
    assert_eq!(events[0].metadata.stream_name, StreamName::for_run("run-1"));
    assert_eq!(events[0].metadata.stream_position, 1);
    assert_eq!(events[1].metadata.stream_position, 2);
}

#[tokio::test]
#[ignore = "needs a running Apache Iggy; `just iggy-up` then `just test-laser`"]
async fn the_envelope_is_promoted_by_the_consumer_not_the_producer() {
    let bus = bus("promotion").await;
    bus.append(vec![envelope("e1", EventType::LlmCompleted, "run-x")])
        .await
        .expect("publishes");

    let mut stream = bus
        .subscribe(SubscribeOptions::from(StartFrom::Beginning))
        .await
        .expect("subscribes");
    let messages = drain_until_caught_up(&mut stream).await;
    let event = messages
        .iter()
        .find_map(SourceMessage::as_event)
        .expect("one event");

    // The producer sent an envelope with no ids at all; everything below was
    // resolved on the way through.
    assert_eq!(event.metadata.message_id.as_str(), "e1");
    assert_eq!(
        event.metadata.correlation_id.as_str(),
        "e1",
        "an unseeded correlation roots on the message id"
    );
    assert_eq!(
        event.metadata.causation_id.as_str(),
        event.metadata.correlation_id.as_str(),
        "and an unseeded causation roots on the correlation"
    );
    assert_eq!(
        event.metadata.trace_id,
        aiwatcher_core::TraceId::derive("run-x"),
        "the trace id is derived, so a redelivery lands on the same trace"
    );
    assert!(event.metadata.global_position > 0);
}

// Multi-threaded on purpose: this test keeps two subscriptions alive at once,
// each with a live Iggy client underneath. On the default current-thread
// runtime a client that blocks starves the timer, and even the assertions'
// timeouts stop firing — the test hangs instead of failing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a running Apache Iggy; `just iggy-up` then `just test-laser`"]
async fn a_consumer_group_resumes_from_its_committed_offset() {
    let bus = bus("resume").await;
    bus.append(vec![
        envelope("e1", EventType::RunStarted, "run-1"),
        envelope("e2", EventType::LlmStarted, "run-1"),
        envelope("e3", EventType::LlmCompleted, "run-1"),
        envelope("e4", EventType::RunCompleted, "run-1"),
    ])
    .await
    .expect("publishes");

    let group = "resume-test";

    // First pass: read everything, commit through position 2 only.
    {
        let mut stream = bus
            .subscribe(SubscribeOptions::from(StartFrom::Beginning).in_group(group))
            .await
            .expect("subscribes");
        let messages = drain_until_caught_up(&mut stream).await;
        assert_eq!(
            messages.iter().filter_map(SourceMessage::as_event).count(),
            4
        );
        bus.save(group, &Checkpoint::from_global_position(2))
            .await
            .expect("commits");
        // The commit is served by the subscription task; give it a moment to
        // reach the broker before the subscription is dropped.
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // Second pass: the same group, resuming from what it committed.
    let mut stream = bus
        .subscribe(SubscribeOptions::from(StartFrom::Now).in_group(group))
        .await
        .expect("resubscribes");
    let messages = drain_until_caught_up(&mut stream).await;
    let positions: Vec<_> = messages
        .iter()
        .filter_map(SourceMessage::as_event)
        .map(|event| event.metadata.global_position)
        .collect();

    assert_eq!(
        positions,
        vec![3, 4],
        "the group picked up after the offset it stored, not from the start"
    );
}

#[tokio::test]
#[ignore = "needs a running Apache Iggy; `just iggy-up` then `just test-laser`"]
async fn a_stream_filter_isolates_one_run() {
    let bus = bus("filter").await;
    bus.append(vec![
        envelope("a1", EventType::RunStarted, "run-a"),
        envelope("b1", EventType::RunStarted, "run-b"),
        envelope("a2", EventType::RunCompleted, "run-a"),
    ])
    .await
    .expect("publishes");

    let mut stream = bus
        .subscribe(
            SubscribeOptions::from(StartFrom::Beginning).for_stream(StreamName::for_run("run-a")),
        )
        .await
        .expect("subscribes");
    let messages = drain_until_caught_up(&mut stream).await;
    let events: Vec<_> = messages
        .iter()
        .filter_map(SourceMessage::as_event)
        .collect();

    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.metadata.run_id == "run-a"));
}

#[tokio::test]
#[ignore = "needs a running Apache Iggy; `just iggy-up` then `just test-laser`"]
async fn a_bounded_read_serves_history_without_joining_the_group() {
    let bus = bus("read").await;
    bus.append(vec![
        envelope("e1", EventType::RunStarted, "run-1"),
        envelope("e2", EventType::RunCompleted, "run-1"),
        envelope("e3", EventType::RunStarted, "run-2"),
    ])
    .await
    .expect("publishes");

    let all = bus
        .read(&Checkpoint::beginning(), 10)
        .await
        .expect("reads through a replay cursor");
    assert_eq!(all.len(), 3);

    let after_first = bus
        .read(&Checkpoint::from_global_position(1), 10)
        .await
        .expect("reads");
    assert_eq!(after_first.len(), 2, "the cursor skipped the first record");

    let one_run = bus
        .read_stream(&StreamName::for_run("run-1"))
        .await
        .expect("reads one stream");
    assert_eq!(one_run.len(), 2);
    assert!(one_run.iter().all(|event| event.metadata.run_id == "run-1"));
}

#[tokio::test]
#[ignore = "needs a running Apache Iggy; `just iggy-up` then `just test-laser`"]
async fn events_published_after_the_catch_up_marker_arrive_live() {
    let bus = bus("live").await;
    bus.append(vec![envelope("e1", EventType::RunStarted, "run-1")])
        .await
        .expect("publishes");

    let mut stream = bus
        .subscribe(SubscribeOptions::from(StartFrom::Beginning))
        .await
        .expect("subscribes");
    let replayed = drain_until_caught_up(&mut stream).await;
    assert_eq!(
        replayed.iter().filter_map(SourceMessage::as_event).count(),
        1
    );

    bus.append(vec![envelope("e2", EventType::RunCompleted, "run-1")])
        .await
        .expect("publishes");

    let live = tokio::time::timeout(Duration::from_secs(15), stream.next())
        .await
        .expect("the live event arrived in time")
        .expect("the stream is open");
    assert_eq!(
        live.as_event().expect("an event").event_type.as_str(),
        "run.completed"
    );
}
