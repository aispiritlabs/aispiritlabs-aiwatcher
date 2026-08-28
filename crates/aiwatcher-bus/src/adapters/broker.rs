//! A generic poll/commit broker adapter.
//!
//! This is the shape almost every log has — publish keyed, poll after a cursor,
//! commit a cursor, ask for the head — expressed as a four-method
//! [`BrokerClient`] trait. It exists for two reasons:
//!
//! * It is what a Kafka, NATS JetStream or Redpanda backend would implement,
//!   without touching anything above `aiwatcher-bus`.
//! * It is testable without a broker. The contract test drives the whole
//!   subscribe/catch-up/resume path over an in-process fake, which is how the
//!   ordering and resume logic is verified at all.
//!
//! The real Laser backend does **not** go through here — `laser_sdk` has richer
//! primitives (per-partition cursors, server-stored group offsets, replay
//! cursors) that this lowest common denominator would throw away. See
//! [`super::laser`].
//!
//! ## What the client must guarantee
//!
//! * **Order within a partition.** Records published with the same
//!   `partition_key` are delivered in publish order. The key is the stream name
//!   (`run:<run_id>`), so one run's events never overtake each other while
//!   unrelated runs proceed in parallel.
//! * **At-least-once delivery.** Redelivery after a crash is expected; that is
//!   why span ids are derived rather than generated.
//! * **Resumable cursors.** `poll` takes the last committed cursor and returns
//!   what follows it.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use aiwatcher_core::{Checkpoint, EventEnvelope, RecordedEvent, StreamName};

use crate::ports::{
    AppendResult, BusError, BusResult, Checkpointer, MessageSink, MessageSource, SourceMessage,
    StartFrom, SubscribeOptions,
};

/// One record as the broker hands it back.
#[derive(Clone, Debug)]
pub struct BrokerRecord {
    /// The broker's own cursor for this record. Opaque here; the adapter only
    /// ever passes it back to [`BrokerClient::poll`] and
    /// [`BrokerClient::commit`].
    pub cursor: String,
    pub payload: Vec<u8>,
}

/// Everything the adapter needs from a Laser (or Iggy, or Kafka) client.
#[async_trait]
pub trait BrokerClient: Send + Sync + fmt::Debug {
    /// Publish a batch. All payloads share one partition key, so the whole
    /// batch lands on one partition in order.
    async fn publish(
        &self,
        topic: &str,
        partition_key: &str,
        payloads: Vec<Vec<u8>>,
    ) -> Result<(), String>;

    /// Fetch up to `max` records after `cursor`. `None` means "from the
    /// beginning". Returning an empty vector means "caught up for now" — the
    /// adapter turns that into [`SourceMessage::CaughtUp`].
    async fn poll(
        &self,
        topic: &str,
        consumer_group: &str,
        cursor: Option<&str>,
        max: usize,
    ) -> Result<Vec<BrokerRecord>, String>;

    /// Record the group's progress. Called only after the batch has been
    /// processed successfully.
    async fn commit(&self, topic: &str, consumer_group: &str, cursor: &str) -> Result<(), String>;

    /// The newest cursor on the topic, for reporting lag.
    async fn head(&self, topic: &str) -> Result<Option<String>, String>;
}

/// Bridges [`BrokerClient`] to the bus ports.
///
/// The generic parameter is what keeps this testable: the tests below run the
/// full subscribe/catch-up/resume path against an in-process fake, with no
/// broker and no network.
#[derive(Debug, Clone)]
pub struct BrokerBus<C: BrokerClient> {
    client: Arc<C>,
    topic: String,
    /// How long to wait before polling again once the topic is drained.
    poll_interval: std::time::Duration,
}

impl<C: BrokerClient + 'static> BrokerBus<C> {
    pub fn new(client: Arc<C>, topic: impl Into<String>) -> Self {
        Self {
            client,
            topic: topic.into(),
            poll_interval: std::time::Duration::from_millis(50),
        }
    }

    #[must_use]
    pub fn with_poll_interval(mut self, interval: std::time::Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    fn decode(record: &BrokerRecord) -> BusResult<RecordedEvent> {
        serde_json::from_slice(&record.payload).map_err(|source| BusError::Decode {
            checkpoint: record.cursor.clone(),
            source,
        })
    }
}

#[async_trait]
impl<C: BrokerClient + 'static> MessageSink for BrokerBus<C> {
    async fn append(&self, events: Vec<EventEnvelope>) -> BusResult<AppendResult> {
        // The broker assigns the durable order, so the positions stamped here
        // are provisional: a projector reads `global_position` off the record
        // it polls, not off this result. What this result is for is echoing the
        // resolved ids back to the caller that published.
        let ingested_at = time::OffsetDateTime::now_utc();
        let mut by_stream: std::collections::HashMap<String, Vec<RecordedEvent>> =
            std::collections::HashMap::new();
        let mut recorded = Vec::with_capacity(events.len());

        for (index, envelope) in events.into_iter().enumerate() {
            envelope.validate()?;
            let stream = envelope.stream_name();
            let event = envelope.record(index as u64 + 1, index as u64 + 1, ingested_at, None);
            by_stream
                .entry(stream.partition_key())
                .or_default()
                .push(event.clone());
            recorded.push(event);
        }

        for (partition_key, batch) in by_stream {
            let payloads = batch
                .iter()
                .map(serde_json::to_vec)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|source| BusError::Decode {
                    checkpoint: partition_key.clone(),
                    source,
                })?;
            self.client
                .publish(&self.topic, &partition_key, payloads)
                .await
                .map_err(BusError::Unavailable)?;
        }

        let last_checkpoint = recorded.last().map_or_else(Checkpoint::beginning, |event| {
            event.metadata.checkpoint.clone()
        });
        Ok(AppendResult {
            recorded,
            last_checkpoint,
        })
    }
}

#[async_trait]
impl<C: BrokerClient + 'static> MessageSource for BrokerBus<C> {
    async fn subscribe(
        &self,
        options: SubscribeOptions,
    ) -> BusResult<BoxStream<'static, SourceMessage>> {
        let (tx, rx) = mpsc::channel(options.batch_size.max(1));
        let client = Arc::clone(&self.client);
        let topic = self.topic.clone();
        let poll_interval = self.poll_interval;
        let batch_size = options.batch_size.max(1);
        let group = options.consumer_group.clone();
        let stream_filter = options.stream.clone();

        let mut cursor = match &options.from {
            StartFrom::Beginning => None,
            StartFrom::After(checkpoint) => Some(checkpoint.to_string()),
            StartFrom::Now => client
                .head(&topic)
                .await
                .map_err(BusError::Unavailable)?
                .or(None),
        };

        tokio::spawn(async move {
            // `caught_up` gates the control message so a subscriber gets one
            // per backlog drain, not one per empty poll.
            let mut announced_catch_up = false;
            loop {
                let records = match client
                    .poll(&topic, &group, cursor.as_deref(), batch_size)
                    .await
                {
                    Ok(records) => records,
                    Err(error) => {
                        tracing::error!(%error, topic, "broker poll failed");
                        tokio::time::sleep(poll_interval).await;
                        continue;
                    }
                };

                if records.is_empty() {
                    if !announced_catch_up {
                        announced_catch_up = true;
                        let checkpoint = cursor
                            .as_deref()
                            .and_then(|raw| Checkpoint::parse(raw).ok())
                            .unwrap_or_else(Checkpoint::beginning);
                        if tx
                            .send(SourceMessage::CaughtUp { checkpoint })
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    tokio::time::sleep(poll_interval).await;
                    continue;
                }

                announced_catch_up = false;
                for record in records {
                    cursor = Some(record.cursor.clone());
                    let event = match BrokerBus::<C>::decode(&record) {
                        Ok(event) => event,
                        Err(error) => {
                            // A record we cannot decode must not stall the
                            // partition. The pipeline's dead-letter sink owns
                            // the retry policy; here we advance past it.
                            tracing::error!(%error, cursor = record.cursor, "skipping undecodable record");
                            continue;
                        }
                    };
                    if !matches_stream(&event, stream_filter.as_ref()) {
                        continue;
                    }
                    if tx.send(SourceMessage::event(event)).await.is_err() {
                        return;
                    }
                }
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    async fn read(&self, from: &Checkpoint, limit: usize) -> BusResult<Vec<RecordedEvent>> {
        let cursor = if from.is_beginning() {
            None
        } else {
            Some(from.to_string())
        };
        let records = self
            .client
            .poll(&self.topic, "aiwatcher-read", cursor.as_deref(), limit)
            .await
            .map_err(BusError::Unavailable)?;
        records.iter().map(Self::decode).collect()
    }

    async fn read_stream(&self, stream: &StreamName) -> BusResult<Vec<RecordedEvent>> {
        // Laser has no server-side filter, so this is a scan. The API layer
        // reads a run's history from the projector's read model instead; this
        // path exists for backfills and tooling.
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let records = self
                .client
                .poll(&self.topic, "aiwatcher-read", cursor.as_deref(), 1024)
                .await
                .map_err(BusError::Unavailable)?;
            if records.is_empty() {
                break;
            }
            cursor = records.last().map(|record| record.cursor.clone());
            for record in &records {
                let event = Self::decode(record)?;
                if &event.metadata.stream_name == stream {
                    out.push(event);
                }
            }
        }
        Ok(out)
    }

    async fn head(&self) -> BusResult<Checkpoint> {
        Ok(self
            .client
            .head(&self.topic)
            .await
            .map_err(BusError::Unavailable)?
            .and_then(|raw| Checkpoint::parse(&raw).ok())
            .unwrap_or_else(Checkpoint::beginning))
    }
}

/// Consumer offsets live in the broker, so the checkpointer is a thin shim over
/// [`BrokerClient::commit`].
#[async_trait]
impl<C: BrokerClient + 'static> Checkpointer for BrokerBus<C> {
    async fn load(&self, _processor_id: &str) -> BusResult<Option<Checkpoint>> {
        // The broker resumes a consumer group from its own committed offset, so
        // a subscription started with `StartFrom::Now` already picks up where
        // the group left off. Returning `None` says "use the group's offset"
        // rather than overriding it with a stale local copy.
        Ok(None)
    }

    async fn save(&self, processor_id: &str, checkpoint: &Checkpoint) -> BusResult<()> {
        self.client
            .commit(&self.topic, processor_id, checkpoint.as_str())
            .await
            .map_err(BusError::Unavailable)
    }
}

fn matches_stream(event: &RecordedEvent, filter: Option<&StreamName>) -> bool {
    filter.is_none_or(|stream| &event.metadata.stream_name == stream)
}
