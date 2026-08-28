//! What the pipeline expects a log to do.

use std::fmt;

use async_trait::async_trait;
use futures::stream::BoxStream;
use thiserror::Error;

use aiwatcher_core::stream::StreamPosition;
use aiwatcher_core::{Checkpoint, EventEnvelope, RecordedEvent, StreamName};

#[derive(Debug, Error)]
pub enum BusError {
    #[error("log is unavailable: {0}")]
    Unavailable(String),

    #[error("log rejected the write: {0}")]
    Rejected(String),

    #[error("could not decode a record at checkpoint {checkpoint}: {source}")]
    Decode {
        checkpoint: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("invalid envelope: {0}")]
    Invalid(#[from] aiwatcher_core::CoreError),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl BusError {
    /// Whether the caller should retry rather than give up on the batch.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::Unavailable(_) | Self::Io(_))
    }
}

pub type BusResult<T> = std::result::Result<T, BusError>;

/// What a subscriber receives.
///
/// [`SourceMessage::CaughtUp`] is Emmett's `__emt:MessageSourceCaughtUp`: the
/// source saying "as of this checkpoint I have nothing more for you". Emmett's
/// consumer strips it before any processor sees it; here it survives one layer
/// further, because it is exactly what an SSE handler needs to tell a client
/// "your replay is done, what follows is live" without a gap or a duplicate.
#[derive(Clone, Debug)]
pub enum SourceMessage {
    Event(Box<RecordedEvent>),
    CaughtUp { checkpoint: Checkpoint },
}

impl SourceMessage {
    #[must_use]
    pub fn event(event: RecordedEvent) -> Self {
        Self::Event(Box::new(event))
    }

    #[must_use]
    pub fn as_event(&self) -> Option<&RecordedEvent> {
        match self {
            Self::Event(event) => Some(event),
            Self::CaughtUp { .. } => None,
        }
    }

    #[must_use]
    pub fn checkpoint(&self) -> &Checkpoint {
        match self {
            Self::Event(event) => &event.metadata.checkpoint,
            Self::CaughtUp { checkpoint } => checkpoint,
        }
    }
}

/// Where a subscription starts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum StartFrom {
    /// Replay everything the log holds.
    Beginning,
    /// Only what arrives from now on.
    #[default]
    Now,
    /// Resume strictly *after* this checkpoint. This is what a reconnecting
    /// browser sends as `Last-Event-ID` and what a restarting projector reads
    /// from its [`Checkpointer`].
    After(Checkpoint),
}

#[derive(Clone, Debug)]
pub struct SubscribeOptions {
    pub from: StartFrom,
    /// Identifies the consumer group. Laser uses it for offset tracking; the
    /// local adapters use it for logging only.
    pub consumer_group: String,
    /// How many events to hand over per poll. Bounds memory during a replay of
    /// a large backlog.
    pub batch_size: usize,
    /// Restrict the subscription to one run's stream. `None` tails everything.
    pub stream: Option<StreamName>,
}

impl Default for SubscribeOptions {
    fn default() -> Self {
        Self {
            from: StartFrom::default(),
            consumer_group: "default".to_owned(),
            batch_size: 256,
            stream: None,
        }
    }
}

impl SubscribeOptions {
    #[must_use]
    pub fn from(start: StartFrom) -> Self {
        Self {
            from: start,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn in_group(mut self, group: impl Into<String>) -> Self {
        self.consumer_group = group.into();
        self
    }

    #[must_use]
    pub fn for_stream(mut self, stream: StreamName) -> Self {
        self.stream = Some(stream);
        self
    }

    #[must_use]
    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size.max(1);
        self
    }
}

/// What an append produced.
#[derive(Clone, Debug)]
pub struct AppendResult {
    /// The envelopes as recorded, with positions and resolved ids filled in.
    pub recorded: Vec<RecordedEvent>,
    /// The checkpoint of the last event written.
    pub last_checkpoint: Checkpoint,
}

/// One page of a stream.
#[derive(Clone, Debug)]
pub struct StreamPage {
    pub events: Vec<RecordedEvent>,
    /// The `stream_position` to pass as `after` for the next page. Absent when
    /// the page reached the end of the stream.
    pub next_cursor: Option<StreamPosition>,
    /// Whether the stream holds more after this page. Distinct from
    /// `next_cursor.is_some()` only in that it stays meaningful for a caller
    /// that discards the cursor.
    pub has_more: bool,
}

impl StreamPage {
    /// Take a page out of an already-read stream.
    ///
    /// The fallback for adapters that cannot seek, and the definition every
    /// override must match: `after` is exclusive, and the page is at most
    /// `limit` long.
    #[must_use]
    pub fn slice(events: Vec<RecordedEvent>, after: Option<StreamPosition>, limit: usize) -> Self {
        let mut remaining: Vec<RecordedEvent> = events
            .into_iter()
            .filter(|event| after.is_none_or(|cursor| event.metadata.stream_position > cursor))
            .collect();
        let has_more = remaining.len() > limit;
        remaining.truncate(limit);
        Self::new(remaining, has_more)
    }

    /// Build a page an adapter read directly, `has_more` decided by the
    /// adapter's own knowledge of what is left.
    #[must_use]
    pub fn new(events: Vec<RecordedEvent>, has_more: bool) -> Self {
        let next_cursor = has_more
            .then(|| events.last().map(|event| event.metadata.stream_position))
            .flatten();
        Self {
            events,
            next_cursor,
            has_more,
        }
    }
}

/// The write side. Also the place where a producer envelope becomes a record —
/// promotion happens once, at the log boundary, so no consumer has to guess.
#[async_trait]
pub trait MessageSink: Send + Sync + fmt::Debug {
    async fn append(&self, events: Vec<EventEnvelope>) -> BusResult<AppendResult>;
}

/// The read side.
#[async_trait]
pub trait MessageSource: Send + Sync + fmt::Debug {
    /// A live subscription. The stream yields events in log order and a
    /// [`SourceMessage::CaughtUp`] whenever it drains the backlog.
    async fn subscribe(
        &self,
        options: SubscribeOptions,
    ) -> BusResult<BoxStream<'static, SourceMessage>>;

    /// A bounded read, for the REST endpoints that serve history.
    async fn read(&self, from: &Checkpoint, limit: usize) -> BusResult<Vec<RecordedEvent>>;

    /// Every event on one stream, oldest first.
    async fn read_stream(&self, stream: &StreamName) -> BusResult<Vec<RecordedEvent>>;

    /// One page of a stream, oldest first, starting after `after`.
    ///
    /// The default reads the whole stream and slices it, which is correct but
    /// costs what [`Self::read_stream`] costs. An adapter that can seek should
    /// override it — the point of the method is that the *caller* is bounded
    /// even when the adapter is not, so the API never ships a whole run's
    /// history in one response.
    async fn read_stream_page(
        &self,
        stream: &StreamName,
        after: Option<StreamPosition>,
        limit: usize,
    ) -> BusResult<StreamPage> {
        let events = self.read_stream(stream).await?;
        Ok(StreamPage::slice(events, after, limit))
    }

    /// The checkpoint of the newest event, or [`Checkpoint::beginning`] on an
    /// empty log.
    async fn head(&self) -> BusResult<Checkpoint>;
}

/// Where a consumer's position is kept between restarts.
///
/// Committing *after* a successful write is what makes the pipeline
/// at-least-once rather than at-most-once. Never commit before the side effect.
#[async_trait]
pub trait Checkpointer: Send + Sync + fmt::Debug {
    /// Named `load`/`save` rather than `read`/`store` so a type can implement
    /// this and [`MessageSource`] without the two `read`s colliding.
    async fn load(&self, processor_id: &str) -> BusResult<Option<Checkpoint>>;
    async fn save(&self, processor_id: &str, checkpoint: &Checkpoint) -> BusResult<()>;
}
