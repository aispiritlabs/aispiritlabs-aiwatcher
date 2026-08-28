//! The consumer loop.
//!
//! ## Ordering, and why it is this order
//!
//! For each event:
//!
//! 1. **Deduplicate.** A redelivery is dropped before it can double-count
//!    tokens. It still advances the checkpoint — it was already processed.
//! 2. **Publish live.** First, because the panel's job is to be fast and a slow
//!    trace store must not delay it. A lost live event is recoverable: the
//!    client reconnects with its checkpoint.
//! 3. **Fold into the read model.** In-memory, cheap.
//! 4. **Assemble.** Produces zero or more finished spans and metric samples.
//! 5. **Flush.** Spans first, then metrics. Retries transient failures; a
//!    rejection is parked in the dead letter queue rather than retried forever.
//! 6. **Commit the checkpoint** — only now. Committing earlier converts a crash
//!    into silent loss; committing later converts it into a redelivery, which
//!    step 1 and the derived span ids absorb.
//!
//! ## Batching
//!
//! Spans and metrics accumulate and flush on whichever comes first: the batch
//! size, the flush interval, or a [`SourceMessage::CaughtUp`] — the source
//! telling us the backlog is drained, which is exactly when a partial batch
//! should go out rather than wait.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use time::OffsetDateTime;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use aiwatcher_bus::{Checkpointer, MessageSource, SourceMessage, StartFrom, SubscribeOptions};
use aiwatcher_core::attrs::aiwatcher as own;
use aiwatcher_core::ports::{
    CompletedSpan, DeadLetter, DeadLetterSink, LiveEvent, LivePublisher, MetricKind, MetricSample,
    MetricSink, TraceStore, attr,
};
use aiwatcher_core::{Checkpoint, RecordedEvent};
use aiwatcher_trace::{AssemblerConfig, SpanAssembler};

use crate::dedup::Deduplicator;
use crate::readmodel::ReadModel;
use crate::retry::{RetryPolicy, with_backoff};

#[derive(Clone, Debug)]
pub struct ProjectorConfig {
    /// Identifies this consumer for checkpointing. Two projectors with the same
    /// id share a position; two with different ids each see everything.
    pub processor_id: String,
    pub consumer_group: String,
    /// Spans buffered before a flush.
    pub flush_batch_size: usize,
    /// Longest a buffered span waits before being written.
    pub flush_interval: Duration,
    /// How often to close spans whose end event never arrived.
    pub sweep_interval: Duration,
    /// Message ids remembered for deduplication.
    pub dedup_capacity: usize,
    pub retry: RetryPolicy,
    pub assembler: AssemblerConfig,
    /// Where to start when no checkpoint is stored yet.
    pub cold_start: StartFrom,
    /// Ignore the stored checkpoint and replay the whole log on startup.
    ///
    /// The read model and the live buffer live in memory, so a restart that
    /// resumes from its checkpoint comes back with an empty runs list and an
    /// empty metrics view — the data is all still in the log, and nothing is
    /// reading it. Replaying rebuilds them.
    ///
    /// Safe because span ids are derived: re-exporting a span overwrites the
    /// one already in the trace store rather than duplicating it. That
    /// determinism is what pays for this.
    ///
    /// Right for a bounded local log; wrong for a Laser topic with months of
    /// history behind it, which is why the Laser wiring turns it off.
    pub rebuild_on_start: bool,
}

impl Default for ProjectorConfig {
    fn default() -> Self {
        Self {
            processor_id: "aiwatcher-projector".to_owned(),
            consumer_group: "aiwatcher".to_owned(),
            flush_batch_size: 128,
            flush_interval: Duration::from_millis(500),
            sweep_interval: Duration::from_secs(30),
            dedup_capacity: 50_000,
            retry: RetryPolicy::default(),
            assembler: AssemblerConfig::default(),
            cold_start: StartFrom::Beginning,
            rebuild_on_start: true,
        }
    }
}

/// Everything the pipeline writes to.
#[derive(Clone)]
pub struct Outputs {
    pub live: Arc<dyn LivePublisher>,
    pub traces: Arc<dyn TraceStore>,
    pub metrics: Arc<dyn MetricSink>,
    pub dead_letters: Arc<dyn DeadLetterSink>,
    pub read_model: Arc<ReadModel>,
}

impl std::fmt::Debug for Outputs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Outputs")
            .field("live", &self.live)
            .field("traces", &self.traces)
            .field("metrics", &self.metrics)
            .field("dead_letters", &self.dead_letters)
            .finish_non_exhaustive()
    }
}

/// Buffered output waiting for a flush.
#[derive(Debug, Default)]
struct Pending {
    spans: Vec<CompletedSpan>,
    metrics: Vec<MetricSample>,
    checkpoint: Option<Checkpoint>,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.spans.is_empty() && self.metrics.is_empty() && self.checkpoint.is_none()
    }
}

/// Consumes the log and keeps every downstream view current.
#[derive(Debug)]
pub struct Projector<S, C> {
    source: Arc<S>,
    checkpointer: Arc<C>,
    outputs: Outputs,
    config: ProjectorConfig,
    assembler: Mutex<SpanAssembler>,
    dedup: Mutex<Deduplicator>,
}

impl<S, C> Projector<S, C>
where
    S: MessageSource + 'static,
    C: Checkpointer + 'static,
{
    pub fn new(
        source: Arc<S>,
        checkpointer: Arc<C>,
        outputs: Outputs,
        config: ProjectorConfig,
    ) -> Self {
        let assembler = SpanAssembler::new(config.assembler.clone());
        let dedup = Deduplicator::new(config.dedup_capacity);
        Self {
            source,
            checkpointer,
            outputs,
            config,
            assembler: Mutex::new(assembler),
            dedup: Mutex::new(dedup),
        }
    }

    /// Run until `shutdown` fires or the source ends.
    ///
    /// On shutdown the assembler is drained so spans that were open at the time
    /// are written rather than silently lost.
    pub async fn run(self: Arc<Self>, shutdown: CancellationToken) -> Result<(), ProjectorError> {
        let stored = self
            .checkpointer
            .load(&self.config.processor_id)
            .await
            .map_err(ProjectorError::Bus)?;
        let from = if self.config.rebuild_on_start {
            StartFrom::Beginning
        } else {
            stored.map_or_else(|| self.config.cold_start.clone(), StartFrom::After)
        };
        tracing::info!(
            processor_id = self.config.processor_id,
            ?from,
            rebuild_on_start = self.config.rebuild_on_start,
            "projector starting"
        );

        let mut stream = self
            .source
            .subscribe(
                SubscribeOptions::from(from)
                    .in_group(self.config.consumer_group.clone())
                    .with_batch_size(self.config.flush_batch_size),
            )
            .await
            .map_err(ProjectorError::Bus)?;

        let mut pending = Pending::default();
        let mut flush_timer = tokio::time::interval(self.config.flush_interval);
        flush_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut sweep_timer = tokio::time::interval(self.config.sweep_interval);
        sweep_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                biased;

                () = shutdown.cancelled() => {
                    tracing::info!("projector shutting down; draining open spans");
                    let drained = self.assembler.lock().await.drain(OffsetDateTime::now_utc());
                    pending.spans.extend(drained.spans);
                    pending.metrics.extend(drained.metrics);
                    self.flush(&mut pending).await;
                    return Ok(());
                }

                message = stream.next() => {
                    let Some(message) = message else {
                        tracing::warn!("message source ended; flushing and stopping");
                        self.flush(&mut pending).await;
                        return Ok(());
                    };
                    match message {
                        SourceMessage::Event(event) => {
                            self.handle(&event, &mut pending).await;
                            if pending.spans.len() >= self.config.flush_batch_size {
                                self.flush(&mut pending).await;
                            }
                        }
                        SourceMessage::CaughtUp { checkpoint } => {
                            // The backlog is drained. Anything buffered should
                            // go out now rather than sit until the timer.
                            tracing::debug!(%checkpoint, "caught up with the log");
                            if pending.checkpoint.is_none() && !checkpoint.is_beginning() {
                                pending.checkpoint = Some(checkpoint);
                            }
                            self.flush(&mut pending).await;
                        }
                    }
                }

                _ = flush_timer.tick() => {
                    if !pending.is_empty() {
                        self.flush(&mut pending).await;
                    }
                }

                _ = sweep_timer.tick() => {
                    let swept = self.assembler.lock().await.sweep(OffsetDateTime::now_utc());
                    if !swept.is_empty() {
                        tracing::warn!(
                            spans = swept.spans.len(),
                            "closing spans whose end event never arrived"
                        );
                        pending.spans.extend(swept.spans);
                        pending.metrics.extend(swept.metrics);
                    }
                    pending.metrics.push(MetricSample {
                        name: own::metrics::OPEN_SPANS.to_owned(),
                        kind: MetricKind::Gauge,
                        value: self.assembler.lock().await.open_span_count() as f64,
                        unit: None,
                        at: OffsetDateTime::now_utc(),
                        attributes: vec![
                            attr(own::processor::ID, self.config.processor_id.clone()),
                        ],
                    });
                }
            }
        }
    }

    async fn handle(&self, event: &RecordedEvent, pending: &mut Pending) {
        // A redelivery still advances the checkpoint: it was processed, just
        // not now.
        pending.checkpoint = Some(event.metadata.checkpoint.clone());

        if !self.dedup.lock().await.admit(&event.metadata.message_id) {
            tracing::debug!(
                message_id = %event.metadata.message_id,
                "dropping a redelivered event"
            );
            pending.metrics.push(MetricSample {
                name: own::metrics::EVENTS_DEDUPLICATED.to_owned(),
                kind: MetricKind::Counter,
                value: 1.0,
                unit: None,
                at: OffsetDateTime::now_utc(),
                attributes: Vec::new(),
            });
            return;
        }

        // Live first: the panel should not wait on storage.
        if let Err(error) = self.outputs.live.publish(LiveEvent::from(event)).await {
            tracing::warn!(%error, "live publish failed; the client will resync on reconnect");
        }

        self.outputs.read_model.apply(event).await;

        let assembled = self.assembler.lock().await.ingest(event);
        pending.spans.extend(assembled.spans);
        pending.metrics.extend(assembled.metrics);
        pending.metrics.push(MetricSample {
            name: own::metrics::EVENTS_INGESTED.to_owned(),
            kind: MetricKind::Counter,
            value: 1.0,
            unit: None,
            at: OffsetDateTime::now_utc(),
            attributes: vec![attr(own::event::TYPE, event.event_type.as_str())],
        });
    }

    /// Write what is buffered, then commit.
    async fn flush(&self, pending: &mut Pending) {
        let spans = std::mem::take(&mut pending.spans);
        let metrics = std::mem::take(&mut pending.metrics);
        let checkpoint = pending.checkpoint.take();

        let mut span_write_failed = false;
        if !spans.is_empty() {
            self.outputs.read_model.record_spans(&spans).await;
            let seed = checkpoint
                .as_ref()
                .and_then(Checkpoint::global_position)
                .unwrap_or(1);
            let attempt_spans = spans.clone();
            let result = with_backoff(self.config.retry, seed, "trace-store", || {
                let batch = attempt_spans.clone();
                async move { self.outputs.traces.write_spans(batch).await }
            })
            .await;

            if let Err(error) = result {
                span_write_failed = true;
                tracing::error!(%error, spans = spans.len(), "failed to write spans");
                self.park_spans(&spans, &checkpoint, &error).await;
            }
        }

        if !metrics.is_empty() {
            let seed = checkpoint
                .as_ref()
                .and_then(Checkpoint::global_position)
                .unwrap_or(1);
            let attempt_metrics = metrics.clone();
            // Metrics are best-effort: a lost aggregate is a gap in a graph,
            // not lost data, and stalling the pipeline over one is a worse
            // trade than the gap.
            if let Err(error) = with_backoff(self.config.retry, seed, "metric-sink", || {
                let batch = attempt_metrics.clone();
                async move { self.outputs.metrics.record(batch).await }
            })
            .await
            {
                tracing::warn!(%error, samples = metrics.len(), "failed to record metrics");
            }
        }

        // Commit last, and only if the durable write went through. A failed
        // span write leaves the checkpoint where it was, so a restart replays
        // those events — which the derived span ids make safe.
        if let Some(checkpoint) = checkpoint {
            if span_write_failed {
                tracing::warn!(
                    %checkpoint,
                    "holding the checkpoint back so the failed batch is replayed"
                );
            } else if let Err(error) = self
                .checkpointer
                .save(&self.config.processor_id, &checkpoint)
                .await
            {
                tracing::error!(%error, %checkpoint, "failed to commit the checkpoint");
            }
        }
    }

    async fn park_spans(
        &self,
        spans: &[CompletedSpan],
        checkpoint: &Option<Checkpoint>,
        error: &aiwatcher_core::ports::PortError,
    ) {
        // Only a permanent rejection is parked. A transient failure keeps the
        // checkpoint back instead, so the batch is retried on the next pass
        // rather than quarantined.
        if error.is_retryable() {
            return;
        }
        let raw = serde_json::to_string(spans).unwrap_or_else(|_| "<unserialisable>".to_owned());
        let letter = DeadLetter {
            checkpoint: checkpoint.clone().unwrap_or_else(Checkpoint::beginning),
            raw,
            reason: error.to_string(),
            attempts: self.config.retry.max_attempts,
            parked_at: OffsetDateTime::now_utc(),
        };
        if let Err(park_error) = self.outputs.dead_letters.park(letter).await {
            tracing::error!(%park_error, "could not park a rejected span batch");
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectorError {
    #[error(transparent)]
    Bus(#[from] aiwatcher_bus::BusError),
}
