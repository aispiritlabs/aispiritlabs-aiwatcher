//! Everything in a `Vec`, plus a broadcast for live tailing.
//!
//! Used by tests and by `just dev`. It implements the same ordering and
//! catch-up semantics as the durable adapters, so a test that passes here is
//! testing the real contract — the point of having it at all.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;

use aiwatcher_core::stream::StreamPosition;
use aiwatcher_core::{Checkpoint, EventEnvelope, RecordedEvent, StreamName};

use crate::ports::{
    AppendResult, BusResult, Checkpointer, MessageSink, MessageSource, SourceMessage, StartFrom,
    StreamPage, SubscribeOptions,
};

/// How many events a slow subscriber may fall behind before the broadcast
/// starts dropping. A drop is not silent: the subscription ends and the client
/// reconnects with its last checkpoint, which replays the gap.
const BROADCAST_CAPACITY: usize = 4096;

#[derive(Debug, Default)]
struct Log {
    events: Vec<RecordedEvent>,
    /// Next stream position per stream. Emmett's `streamPosition` is 1-based.
    stream_positions: HashMap<String, u64>,
}

/// An in-memory implementation of the whole bus.
#[derive(Debug, Clone)]
pub struct InMemoryBus {
    log: Arc<Mutex<Log>>,
    live: broadcast::Sender<RecordedEvent>,
    checkpoints: Arc<Mutex<HashMap<String, Checkpoint>>>,
}

impl Default for InMemoryBus {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryBus {
    #[must_use]
    pub fn new() -> Self {
        let (live, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            log: Arc::new(Mutex::new(Log::default())),
            live,
            checkpoints: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Everything written so far. Test helper.
    pub async fn all(&self) -> Vec<RecordedEvent> {
        self.log.lock().await.events.clone()
    }
}

#[async_trait]
impl MessageSink for InMemoryBus {
    async fn append(&self, events: Vec<EventEnvelope>) -> BusResult<AppendResult> {
        let ingested_at = time::OffsetDateTime::now_utc();
        let mut log = self.log.lock().await;
        let mut recorded = Vec::with_capacity(events.len());

        for envelope in events {
            envelope.validate()?;
            let stream = envelope.stream_name().to_string();
            let stream_position = {
                let next = log.stream_positions.entry(stream).or_insert(0);
                *next += 1;
                *next
            };
            let global_position = log.events.len() as u64 + 1;
            let event = envelope.record(stream_position, global_position, ingested_at, None);
            log.events.push(event.clone());
            recorded.push(event);
        }

        let last_checkpoint = recorded.last().map_or_else(Checkpoint::beginning, |event| {
            event.metadata.checkpoint.clone()
        });

        // Publish only after the log is consistent, so a subscriber that races
        // us cannot see an event the log does not yet contain.
        drop(log);
        for event in &recorded {
            // An error here means nobody is listening, which is not a failure.
            let _ = self.live.send(event.clone());
        }

        Ok(AppendResult {
            recorded,
            last_checkpoint,
        })
    }
}

#[async_trait]
impl MessageSource for InMemoryBus {
    async fn subscribe(
        &self,
        options: SubscribeOptions,
    ) -> BusResult<BoxStream<'static, SourceMessage>> {
        // Order matters: take the live receiver *before* snapshotting the
        // backlog. The other order loses anything appended in between. The
        // overlap this creates is removed by the position filter below.
        let mut live = self.live.subscribe();
        let log = self.log.lock().await;

        let from_position = match &options.from {
            StartFrom::Beginning => 0,
            StartFrom::Now => log.events.len() as u64,
            StartFrom::After(checkpoint) => checkpoint.global_position().unwrap_or(0),
        };
        let backlog: Vec<RecordedEvent> = log
            .events
            .iter()
            .filter(|event| event.metadata.global_position > from_position)
            .filter(|event| matches_stream(event, options.stream.as_ref()))
            .cloned()
            .collect();
        let head = log.events.len() as u64;
        drop(log);

        let (tx, rx) = mpsc::channel(options.batch_size.max(1));
        let stream_filter = options.stream.clone();

        tokio::spawn(async move {
            let mut delivered = from_position;
            for event in backlog {
                delivered = delivered.max(event.metadata.global_position);
                if tx.send(SourceMessage::event(event)).await.is_err() {
                    return;
                }
            }
            // The subscriber is now level with the log as it stood at
            // subscribe time.
            if tx
                .send(SourceMessage::CaughtUp {
                    checkpoint: Checkpoint::from_global_position(head.max(delivered)),
                })
                .await
                .is_err()
            {
                return;
            }

            loop {
                match live.recv().await {
                    Ok(event) => {
                        // Skip what the backlog already delivered.
                        if event.metadata.global_position <= delivered {
                            continue;
                        }
                        if !matches_stream(&event, stream_filter.as_ref()) {
                            continue;
                        }
                        delivered = event.metadata.global_position;
                        if tx.send(SourceMessage::event(event)).await.is_err() {
                            return;
                        }
                    }
                    // Lagged: end the subscription rather than skip silently.
                    // The client reconnects with its last checkpoint and the
                    // gap is replayed from the log.
                    Err(broadcast::error::RecvError::Lagged(_)) => return,
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    async fn read(&self, from: &Checkpoint, limit: usize) -> BusResult<Vec<RecordedEvent>> {
        let after = from.global_position().unwrap_or(0);
        Ok(self
            .log
            .lock()
            .await
            .events
            .iter()
            .filter(|event| event.metadata.global_position > after)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn read_stream(&self, stream: &StreamName) -> BusResult<Vec<RecordedEvent>> {
        Ok(self
            .log
            .lock()
            .await
            .events
            .iter()
            .filter(|event| &event.metadata.stream_name == stream)
            .cloned()
            .collect())
    }

    async fn read_stream_page(
        &self,
        stream: &StreamName,
        after: Option<StreamPosition>,
        limit: usize,
    ) -> BusResult<StreamPage> {
        // Take one more than asked for: whether it materialised is the answer
        // to `has_more`, without a second pass over the log.
        let mut events: Vec<RecordedEvent> = self
            .log
            .lock()
            .await
            .events
            .iter()
            .filter(|event| &event.metadata.stream_name == stream)
            .filter(|event| after.is_none_or(|cursor| event.metadata.stream_position > cursor))
            .take(limit.saturating_add(1))
            .cloned()
            .collect();
        let has_more = events.len() > limit;
        events.truncate(limit);
        Ok(StreamPage::new(events, has_more))
    }

    async fn head(&self) -> BusResult<Checkpoint> {
        let log = self.log.lock().await;
        Ok(log
            .events
            .last()
            .map_or_else(Checkpoint::beginning, |event| {
                event.metadata.checkpoint.clone()
            }))
    }
}

#[async_trait]
impl Checkpointer for InMemoryBus {
    async fn load(&self, processor_id: &str) -> BusResult<Option<Checkpoint>> {
        Ok(self.checkpoints.lock().await.get(processor_id).cloned())
    }

    async fn save(&self, processor_id: &str, checkpoint: &Checkpoint) -> BusResult<()> {
        self.checkpoints
            .lock()
            .await
            .insert(processor_id.to_owned(), checkpoint.clone());
        Ok(())
    }
}

fn matches_stream(event: &RecordedEvent, filter: Option<&StreamName>) -> bool {
    filter.is_none_or(|stream| &event.metadata.stream_name == stream)
}
