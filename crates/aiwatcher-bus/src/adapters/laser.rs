//! The real Laser backend, over `laser_sdk` and Apache Iggy.
//!
//! ```no_run
//! # use aiwatcher_bus::adapters::laser::{LaserBus, LaserConfig};
//! # async fn wire() -> Result<(), Box<dyn std::error::Error>> {
//! let bus = LaserBus::connect(LaserConfig::default()).await?;
//! # let _ = bus; Ok(()) }
//! ```
//!
//! ## What lands where
//!
//! | aiwatcher | Laser |
//! |---|---|
//! | one run | a partition key (`run:<run_id>`), so a run's events keep their order |
//! | one event | one record, payload = the producer's [`EventEnvelope`] as JSON |
//! | `global_position` | the record's Iggy offset, plus one |
//! | projector resume | the consumer group's server-stored offset |
//! | checkpoint commit | [`Consumer::store_offset`], only after the durable write |
//!
//! ## The envelope on the wire, not the record
//!
//! The other adapters promote an [`EventEnvelope`] into a [`RecordedEvent`] at
//! append time, because they *are* the store and they assign the position. Here
//! the broker assigns it, and a producer has no way to know it in advance. So
//! the topic carries the envelope and the **consumer** promotes, stamping the
//! position from the offset Iggy actually gave the record.
//!
//! That also means a producer never has to be able to compute a position, which
//! is what lets a Python agent publish to Laser directly rather than through
//! this process.
//!
//! ## One partition, deliberately
//!
//! A [`Checkpoint`] is a single ordered scalar: that is what makes
//! `Last-Event-ID` resume work with no client-side bookkeeping, and what lets
//! the live tail drop what a client already saw with one comparison. A
//! multi-partition log has no total order — partition 0 offset 100 and
//! partition 1 offset 5 are not comparable — so a scalar cursor would silently
//! skip events on a lagging partition.
//!
//! [`LaserConfig::partitions`] therefore defaults to 1, and the records are
//! still keyed by run so that raising it preserves per-run ordering. Raising it
//! above 1 **requires** replacing the scalar checkpoint with a per-partition
//! vector; the constructor warns rather than letting that pass quietly. One
//! Iggy partition carries far more than agent telemetry produces, so this is a
//! ceiling worth hitting before designing around.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::BoxStream;
use laser_sdk::prelude::{
    CommitPolicy, Consumer, ConsumerStart, Laser, LaserError, Producer, ProducerMessage, Routing,
};
use tokio::sync::{Mutex, mpsc};
use tokio_stream::wrappers::ReceiverStream;

use aiwatcher_core::{Checkpoint, EventEnvelope, RecordedEvent, StreamName};

use crate::ports::{
    AppendResult, BusError, BusResult, Checkpointer, MessageSink, MessageSource, SourceMessage,
    StartFrom, SubscribeOptions,
};

/// How long a consumer may sit idle before the subscription reports itself
/// caught up. Laser has no "you are at the tail" signal, so silence is the
/// signal — see [`SourceMessage::CaughtUp`].
const CAUGHT_UP_AFTER: Duration = Duration::from_millis(250);

#[derive(Clone, Debug)]
pub struct LaserConfig {
    /// `user:password@host:port`. `Laser::connect` supplies the TCP scheme.
    pub connection_string: String,
    /// The Iggy stream. One per deployment, not per run.
    pub stream: String,
    /// The topic every agent event is published to.
    pub topic: String,
    /// Kept at 1 on purpose. See the module docs.
    pub partitions: u32,
    /// Records requested per poll.
    pub batch_length: u32,
    /// How long to wait for the broker before giving up at startup.
    ///
    /// The Iggy client retries internally and will otherwise sit there
    /// indefinitely. A process that hangs on startup is worse than one that
    /// exits: it never goes unready, so nothing restarts it and no probe fires.
    pub connect_timeout: Duration,
}

impl Default for LaserConfig {
    fn default() -> Self {
        Self {
            connection_string: "iggy:iggy@127.0.0.1:8090".to_owned(),
            stream: "aiwatcher".to_owned(),
            topic: "events".to_owned(),
            partitions: 1,
            batch_length: 256,
            connect_timeout: Duration::from_secs(15),
        }
    }
}

/// What a commit needs: the broker position of the last event that was
/// durably written downstream.
#[derive(Clone, Copy, Debug)]
struct CommitRequest {
    partition_id: u32,
    offset: u64,
}

/// The Laser-backed bus.
#[derive(Clone)]
pub struct LaserBus {
    laser: Laser,
    config: LaserConfig,
    producer: Arc<Producer>,
    /// Set when a subscription starts. Commits are forwarded to the task that
    /// owns the `Consumer`, because offsets can only be stored through it.
    commits: Arc<Mutex<Option<mpsc::Sender<CommitRequest>>>>,
    /// The highest position handed out, so `head` can answer without a poll.
    head: Arc<AtomicU64>,
}

impl std::fmt::Debug for LaserBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaserBus")
            .field("stream", &self.config.stream)
            .field("topic", &self.config.topic)
            .field("partitions", &self.config.partitions)
            .finish_non_exhaustive()
    }
}

impl LaserBus {
    /// Connect, and make sure the stream and topic exist.
    pub async fn connect(config: LaserConfig) -> BusResult<Self> {
        if config.partitions != 1 {
            tracing::warn!(
                partitions = config.partitions,
                "more than one partition: a scalar checkpoint cannot order across \
                 partitions, so live-stream resume may skip events on a lagging one. \
                 See the module docs on aiwatcher_bus::adapters::laser."
            );
        }

        let laser = tokio::time::timeout(
            config.connect_timeout,
            Laser::connect(&config.connection_string),
        )
        .await
        .map_err(|_| {
            BusError::Unavailable(format!(
                "no response from the Laser broker within {:?}",
                config.connect_timeout
            ))
        })?
        .map_err(to_bus_error)?;

        let topic = laser.stream(&config.stream).topic(&config.topic);
        // `create_stream`/`create_topic` make this idempotent: the first process
        // to start creates them, the rest find them.
        let producer = topic
            .producer()
            .create_stream(true)
            .create_topic(true)
            .partitions(config.partitions)
            .routing(Routing::Balanced)
            .build()
            .await
            .map_err(to_bus_error)?;

        tracing::info!(
            stream = config.stream,
            topic = config.topic,
            partitions = config.partitions,
            "connected to Laser"
        );

        Ok(Self {
            laser,
            config,
            producer: Arc::new(producer),
            commits: Arc::new(Mutex::new(None)),
            head: Arc::new(AtomicU64::new(0)),
        })
    }

    fn topic(&self) -> laser_sdk::prelude::Topic {
        self.laser
            .stream(&self.config.stream)
            .topic(&self.config.topic)
    }

    /// Iggy offsets are 0-based; aiwatcher positions are 1-based, matching the
    /// other adapters. Keeping the two one apart rather than equal means
    /// `Checkpoint::beginning()` (position 0) is genuinely "before the first
    /// record" instead of aliasing it.
    fn position_of(offset: u64) -> u64 {
        offset.saturating_add(1)
    }

    fn offset_of(position: u64) -> u64 {
        position.saturating_sub(1)
    }

    /// Promote a producer envelope using the position the broker assigned.
    fn record(
        payload: &[u8],
        partition_id: u32,
        offset: u64,
        stream_positions: &mut std::collections::HashMap<String, u64>,
    ) -> BusResult<RecordedEvent> {
        let envelope: EventEnvelope =
            serde_json::from_slice(payload).map_err(|source| BusError::Decode {
                checkpoint: format!("{partition_id}:{offset}"),
                source,
            })?;
        envelope.validate()?;

        let global_position = Self::position_of(offset);
        // Per-run position. Rebuilt as the consumer reads, which is exact for a
        // consumer that started at the beginning and approximate for one that
        // resumed — the field is for display, and `global_position` is what
        // anything correctness-bearing uses.
        let stream_position = {
            let next = stream_positions
                .entry(envelope.stream_name().to_string())
                .or_insert(0);
            *next += 1;
            *next
        };
        Ok(envelope.record(
            stream_position,
            global_position,
            time::OffsetDateTime::now_utc(),
            None,
        ))
    }

    /// Drain the topic from the beginning through a replay cursor.
    ///
    /// Client-owned offsets, so it neither joins nor disturbs the projector's
    /// consumer group. Used by the REST reads.
    async fn drain(&self, after_position: u64, limit: usize) -> BusResult<Vec<RecordedEvent>> {
        let mut cursor = self
            .topic()
            .replay()
            .map_err(to_bus_error)?
            .batch(self.config.batch_length);

        let mut stream_positions = std::collections::HashMap::new();
        let mut out = Vec::new();
        loop {
            let batch = cursor.poll().await.map_err(to_bus_error)?;
            if batch.is_empty() {
                break;
            }
            for message in batch {
                let event = Self::record(
                    &message.payload,
                    message.id.partition_id,
                    message.id.offset,
                    &mut stream_positions,
                )?;
                if event.metadata.global_position > after_position {
                    out.push(event);
                }
                if out.len() >= limit {
                    return Ok(out);
                }
            }
        }
        Ok(out)
    }
}

#[async_trait]
impl MessageSink for LaserBus {
    async fn append(&self, events: Vec<EventEnvelope>) -> BusResult<AppendResult> {
        for envelope in &events {
            envelope.validate()?;
        }

        // Group by run so each batch goes out under one partition key. With one
        // partition this changes nothing; with more it is what keeps a run's
        // events in order.
        let mut by_key: std::collections::HashMap<String, Vec<ProducerMessage>> =
            std::collections::HashMap::new();
        for envelope in &events {
            let payload = serde_json::to_vec(envelope).map_err(|source| BusError::Decode {
                checkpoint: envelope.run_id.clone(),
                source,
            })?;
            by_key
                .entry(envelope.stream_name().partition_key())
                .or_default()
                .push(ProducerMessage::new(bytes::Bytes::from(payload)));
        }
        for (key, batch) in by_key {
            self.producer
                .send_batch_with_routing(batch, Some(Routing::key(key.into_bytes())))
                .await
                .map_err(to_bus_error)?;
        }

        // Iggy's send response does not carry per-record offsets, and the
        // position that matters is the one the *consumer* stamps. What comes
        // back here is the echo a publisher needs — the resolved correlation
        // ids — with positions left at zero rather than invented.
        let ingested_at = time::OffsetDateTime::now_utc();
        let recorded: Vec<RecordedEvent> = events
            .into_iter()
            .map(|envelope| envelope.record(0, 0, ingested_at, None))
            .collect();

        Ok(AppendResult {
            last_checkpoint: Checkpoint::from_global_position(self.head.load(Ordering::Acquire)),
            recorded,
        })
    }
}

#[async_trait]
impl MessageSource for LaserBus {
    async fn subscribe(
        &self,
        options: SubscribeOptions,
    ) -> BusResult<BoxStream<'static, SourceMessage>> {
        let start = match &options.from {
            StartFrom::Beginning => ConsumerStart::First,
            // The broker owns the group's position: resuming after the stored
            // offset is what makes a projector restart pick up where it left
            // off without any local state.
            StartFrom::Now => ConsumerStart::Next,
            StartFrom::After(checkpoint) => {
                ConsumerStart::Offset(Self::offset_of(checkpoint.global_position().unwrap_or(0)))
            }
        };

        let consumer = self
            .topic()
            .consumer_group(options.consumer_group.clone())
            .create_group(true)
            .auto_join_group(true)
            .start_at(start)
            // Manual: the pipeline commits only after the durable write, and an
            // automatic policy would move the offset past events that were
            // never stored.
            .commit_policy(CommitPolicy::Disabled)
            .batch_length(options.batch_size.max(1) as u32)
            .allow_replay()
            // Bounded. The Iggy client's default is to retry initialisation
            // forever, so a group that cannot be joined presents as a hang with
            // a reconnect loop in the server log rather than as an error.
            .init_retries(3, Duration::from_secs(1))
            .build();
        let mut consumer: Consumer = tokio::time::timeout(self.config.connect_timeout, consumer)
            .await
            .map_err(|_| {
                BusError::Unavailable(format!(
                    "consumer group {:?} on {}/{} did not initialise within {:?}",
                    options.consumer_group,
                    self.config.stream,
                    self.config.topic,
                    self.config.connect_timeout
                ))
            })?
            .map_err(to_bus_error)?;

        let (events_tx, events_rx) = mpsc::channel(options.batch_size.max(1));
        let (commit_tx, mut commit_rx) = mpsc::channel::<CommitRequest>(64);
        *self.commits.lock().await = Some(commit_tx);

        let stream_filter = options.stream.clone();
        let head = Arc::clone(&self.head);

        let start_position = match &options.from {
            StartFrom::After(checkpoint) => checkpoint.global_position().unwrap_or(0),
            _ => 0,
        };

        tokio::spawn(async move {
            let mut stream_positions = std::collections::HashMap::new();
            let mut announced_catch_up = false;
            // The highest position handed to the subscriber.
            //
            // Under `CommitPolicy::Disabled` every poll starts from the offset
            // last *stored* on the server, and the pipeline stores one only
            // after a durable write. Between those two points the broker keeps
            // returning the same records — measured at ~300k redeliveries of
            // the same two events in one test — so the adapter has to remember
            // how far it has read and drop what it has already emitted.
            //
            // This is the local read position; the committed offset is
            // deliberately behind it, and that gap is what makes a crash replay
            // rather than lose events.
            let mut delivered = start_position;

            loop {
                tokio::select! {
                    // The caller dropped the stream. Leave the consumer group
                    // rather than sitting in `next_within` forever: a lingering
                    // member keeps its partition assignment, so the next
                    // subscription to the same group would join and receive
                    // nothing.
                    () = events_tx.closed() => {
                        if let Err(error) = consumer.shutdown().await {
                            tracing::warn!(%error, "failed to leave the consumer group cleanly");
                        }
                        return;
                    }

                    // Commits are served on the same task because offsets can
                    // only be stored through the `Consumer` that owns them.
                    Some(request) = commit_rx.recv() => {
                        if let Err(error) = consumer
                            .store_offset(request.offset, Some(request.partition_id))
                            .await
                        {
                            tracing::error!(%error, offset = request.offset, "failed to store the consumer offset");
                        }
                    }

                    received = consumer.next_within(CAUGHT_UP_AFTER) => {
                        match received {
                            Ok(message) => {
                                let position = LaserBus::position_of(message.position.offset);
                                head.fetch_max(position, Ordering::AcqRel);
                                if position <= delivered {
                                    // Already emitted; the broker is replaying
                                    // because the committed offset has not
                                    // caught up yet. That replay is itself the
                                    // tail signal — the broker has nothing
                                    // newer — so it announces the catch-up that
                                    // silence would otherwise have to.
                                    if !announced_catch_up {
                                        announced_catch_up = true;
                                        let checkpoint = Checkpoint::from_global_position(
                                            head.load(Ordering::Acquire),
                                        );
                                        if events_tx
                                            .send(SourceMessage::CaughtUp { checkpoint })
                                            .await
                                            .is_err()
                                        {
                                            return;
                                        }
                                    }
                                    continue;
                                }
                                delivered = position;
                                announced_catch_up = false;

                                let event = match LaserBus::record(
                                    &message.payload,
                                    message.partition_id,
                                    message.position.offset,
                                    &mut stream_positions,
                                ) {
                                    Ok(event) => event,
                                    Err(error) => {
                                        // A record we cannot decode must not
                                        // stall the partition. Skipping it here
                                        // holds the offset back, so the
                                        // projector's dead-letter path still
                                        // sees it on the next pass.
                                        tracing::error!(
                                            %error,
                                            offset = message.position.offset,
                                            "skipping an undecodable record"
                                        );
                                        continue;
                                    }
                                };
                                if !matches_stream(&event, stream_filter.as_ref()) {
                                    continue;
                                }
                                if events_tx.send(SourceMessage::event(event)).await.is_err() {
                                    return;
                                }
                            }
                            Err(LaserError::Timeout(_)) => {
                                // Silence means the tail. Announce it once, so a
                                // subscriber gets one marker per backlog drain
                                // rather than one per idle poll.
                                if !announced_catch_up {
                                    announced_catch_up = true;
                                    let checkpoint = Checkpoint::from_global_position(
                                        head.load(Ordering::Acquire),
                                    );
                                    if events_tx
                                        .send(SourceMessage::CaughtUp { checkpoint })
                                        .await
                                        .is_err()
                                    {
                                        return;
                                    }
                                }
                            }
                            Err(error) => {
                                tracing::error!(%error, "laser consumer failed; ending the subscription");
                                return;
                            }
                        }
                    }

                    else => return,
                }
            }
        });

        Ok(Box::pin(ReceiverStream::new(events_rx)))
    }

    async fn read(&self, from: &Checkpoint, limit: usize) -> BusResult<Vec<RecordedEvent>> {
        self.drain(from.global_position().unwrap_or(0), limit).await
    }

    async fn read_stream(&self, stream: &StreamName) -> BusResult<Vec<RecordedEvent>> {
        // Iggy has no server-side content filter, so this is a scan bounded by
        // the topic. The API layer serves a run's history from the projector's
        // read model; this path is for backfills and audit.
        Ok(self
            .drain(0, usize::MAX)
            .await?
            .into_iter()
            .filter(|event| &event.metadata.stream_name == stream)
            .collect())
    }

    async fn head(&self) -> BusResult<Checkpoint> {
        Ok(Checkpoint::from_global_position(
            self.head.load(Ordering::Acquire),
        ))
    }
}

#[async_trait]
impl Checkpointer for LaserBus {
    async fn load(&self, _processor_id: &str) -> BusResult<Option<Checkpoint>> {
        // The broker resumes a consumer group from its own committed offset, so
        // a subscription started with `StartFrom::Now` already picks up where
        // the group left off. Returning `None` says "use the group's offset"
        // rather than overriding it with a stale local copy.
        Ok(None)
    }

    async fn save(&self, _processor_id: &str, checkpoint: &Checkpoint) -> BusResult<()> {
        let Some(position) = checkpoint.global_position() else {
            return Ok(());
        };
        let guard = self.commits.lock().await;
        let Some(sender) = guard.as_ref() else {
            // No subscription is running, so there is no offset to advance.
            return Ok(());
        };
        sender
            .send(CommitRequest {
                // Single partition — see the module docs. With more, the
                // checkpoint would have to carry the partition too.
                partition_id: 0,
                offset: Self::offset_of(position),
            })
            .await
            .map_err(|_| BusError::Unavailable("the laser consumer task has stopped".to_owned()))
    }
}

fn matches_stream(event: &RecordedEvent, filter: Option<&StreamName>) -> bool {
    filter.is_none_or(|stream| &event.metadata.stream_name == stream)
}

/// Map a Laser failure onto the retryable/permanent split the pipeline reads.
///
/// Getting this backwards either spins forever on a bad payload or discards
/// good data during an outage, so the mapping is explicit rather than a
/// catch-all.
fn to_bus_error(error: LaserError) -> BusError {
    match error {
        LaserError::Codec(message) => BusError::Rejected(message),
        LaserError::Invalid(message) => BusError::Rejected(message),
        LaserError::Config(message) => BusError::Rejected(message.to_owned()),
        other => BusError::Unavailable(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_are_one_ahead_of_iggy_offsets() {
        // Offset 0 is the first record; position 0 must stay "before anything".
        assert_eq!(LaserBus::position_of(0), 1);
        assert_eq!(LaserBus::offset_of(1), 0);
        assert_eq!(
            LaserBus::offset_of(0),
            0,
            "beginning clamps rather than wraps"
        );
        assert_eq!(
            Checkpoint::from_global_position(LaserBus::position_of(0)).global_position(),
            Some(1)
        );
        for offset in [0u64, 1, 42, 9_999] {
            assert_eq!(LaserBus::offset_of(LaserBus::position_of(offset)), offset);
        }
    }

    #[test]
    fn a_codec_failure_is_permanent_and_an_outage_is_retryable() {
        assert!(!to_bus_error(LaserError::Codec("bad json".to_owned())).is_retryable());
        assert!(!to_bus_error(LaserError::Invalid("no such topic".to_owned())).is_retryable());
        assert!(to_bus_error(LaserError::Timeout("the broker")).is_retryable());
    }

    #[test]
    fn connecting_gives_up_rather_than_hanging_forever() {
        // The Iggy client retries internally with no ceiling; a bounded wait is
        // what lets a readiness probe see a broken deployment.
        assert_eq!(
            LaserConfig::default().connect_timeout,
            Duration::from_secs(15)
        );
    }

    #[test]
    fn the_default_config_uses_a_single_partition() {
        // Not a preference: a scalar checkpoint has no total order across
        // partitions. See the module docs.
        assert_eq!(LaserConfig::default().partitions, 1);
        assert_eq!(
            LaserConfig::default().connection_string,
            "iggy:iggy@127.0.0.1:8090"
        );
    }
}
