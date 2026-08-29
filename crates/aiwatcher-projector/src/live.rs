//! The live channel, and how a reconnecting client closes its gap.
//!
//! Neither VictoriaTraces nor QuestDB should be the push path to a browser.
//! The projector already has every event in hand; it fans out here, and the
//! trace store stays what it is good at — history and search.
//!
//! ## Reconnect
//!
//! A browser that drops sends back the checkpoint of the last event it saw
//! (`Last-Event-ID` for SSE, a field in the hello frame for WebSocket). Three
//! outcomes:
//!
//! 1. The ring buffer still holds it → replay from the buffer, then go live.
//!    One round trip, no storage query.
//! 2. The checkpoint is older than the buffer → [`ReplayGap`]. The caller falls
//!    back to reading the durable log, which is why this returns an error
//!    rather than quietly skipping ahead.
//! 3. No checkpoint → live only.

use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{Mutex, broadcast};
use tokio_stream::wrappers::BroadcastStream;

use aiwatcher_core::Checkpoint;
use aiwatcher_core::ports::{LiveEvent, LivePublisher, PortResult};

/// The requested checkpoint has already scrolled out of the ring buffer.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("checkpoint {requested} is older than the live buffer (oldest is {oldest})")]
pub struct ReplayGap {
    pub requested: Checkpoint,
    pub oldest: Checkpoint,
}

#[derive(Clone, Debug)]
pub struct LiveHubConfig {
    /// Events kept for reconnect replay. A browser that drops for a few
    /// seconds should never need the durable log; sizing this to a few seconds
    /// of peak throughput is the goal.
    ///
    /// Each entry holds the event's payload, so this is a real memory cost —
    /// part of the 512 MB budget in `ReadModelConfig`.
    pub buffer_capacity: usize,
    /// Per-subscriber queue depth. A subscriber that falls this far behind is
    /// dropped and must reconnect — which is correct: a stalled tab should not
    /// hold memory for everyone else.
    pub broadcast_capacity: usize,
}

impl Default for LiveHubConfig {
    fn default() -> Self {
        Self {
            buffer_capacity: 4096,
            broadcast_capacity: 1024,
        }
    }
}

/// In-process fan-out to every SSE and WebSocket client.
#[derive(Debug)]
pub struct LiveHub {
    sender: broadcast::Sender<LiveEvent>,
    recent: Mutex<VecDeque<LiveEvent>>,
    config: LiveHubConfig,
}

impl Default for LiveHub {
    fn default() -> Self {
        Self::new(LiveHubConfig::default())
    }
}

impl LiveHub {
    #[must_use]
    pub fn new(config: LiveHubConfig) -> Self {
        let (sender, _) = broadcast::channel(config.broadcast_capacity.max(1));
        Self {
            sender,
            recent: Mutex::new(VecDeque::with_capacity(config.buffer_capacity)),
            config,
        }
    }

    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }

    /// A stream of everything published from now on.
    ///
    /// `Err` items mean the subscriber lagged past `broadcast_capacity`; the
    /// handler should close the connection so the client reconnects with its
    /// last checkpoint.
    #[must_use]
    pub fn stream(&self) -> BroadcastStream<LiveEvent> {
        BroadcastStream::new(self.sender.subscribe())
    }

    /// What the buffer holds after `from`, or a [`ReplayGap`].
    pub async fn replay_after(&self, from: &Checkpoint) -> Result<Vec<LiveEvent>, ReplayGap> {
        let recent = self.recent.lock().await;
        let Some(oldest) = recent.front() else {
            // Nothing buffered yet, so there is nothing to have missed.
            return Ok(Vec::new());
        };
        // A gap exists only when something sits *strictly between* the client's
        // cursor and the oldest event still buffered. `oldest > from` alone is
        // not enough: a client at position 3 with the buffer starting at 4 has
        // missed nothing, and a client that has seen nothing at all is fully
        // served by a buffer that starts at position 1.
        let oldest_position = oldest.checkpoint.global_position().unwrap_or(0);
        let from_position = from.global_position().unwrap_or(0);
        if oldest_position > from_position.saturating_add(1) {
            return Err(ReplayGap {
                requested: from.clone(),
                oldest: oldest.checkpoint.clone(),
            });
        }
        Ok(recent
            .iter()
            .filter(|event| &event.checkpoint > from)
            .cloned()
            .collect())
    }

    /// The checkpoint of the newest buffered event.
    pub async fn head(&self) -> Option<Checkpoint> {
        self.recent
            .lock()
            .await
            .back()
            .map(|event| event.checkpoint.clone())
    }
}

#[async_trait]
impl LivePublisher for LiveHub {
    async fn publish(&self, event: LiveEvent) -> PortResult<()> {
        {
            let mut recent = self.recent.lock().await;
            if recent.len() >= self.config.buffer_capacity {
                recent.pop_front();
            }
            recent.push_back(event.clone());
        }
        // No subscribers is the normal case, not an error.
        let _ = self.sender.send(event);
        Ok(())
    }
}

/// A hub shared by the projector and every HTTP handler.
pub type SharedLiveHub = Arc<LiveHub>;

#[cfg(test)]
mod tests {
    use time::macros::datetime;
    use tokio_stream::StreamExt;

    use aiwatcher_core::{EventType, SpanId, TraceId};

    use super::*;

    fn event(position: u64) -> LiveEvent {
        let trace_id = TraceId::derive("run-1");
        LiveEvent {
            checkpoint: Checkpoint::from_global_position(position),
            run_id: "run-1".to_owned(),
            conversation_id: None,
            workflow_id: None,
            workflow_run_id: None,
            trace_id,
            span_id: SpanId::derive(trace_id, "run"),
            event_type: EventType::LlmChunk,
            sequence: Some(position),
            occurred_at: datetime!(2026-08-27 18:20:11 UTC),
            data: serde_json::json!({ "text": "…" }),
        }
    }

    #[tokio::test]
    async fn a_subscriber_receives_what_is_published_after_it_subscribed() {
        let hub = LiveHub::default();
        let mut stream = hub.stream();

        hub.publish(event(1)).await.expect("publishes");
        hub.publish(event(2)).await.expect("publishes");

        let first = stream.next().await.expect("an item").expect("no lag");
        let second = stream.next().await.expect("an item").expect("no lag");
        assert_eq!(first.checkpoint, Checkpoint::from_global_position(1));
        assert_eq!(second.checkpoint, Checkpoint::from_global_position(2));
    }

    #[tokio::test]
    async fn a_reconnect_replays_only_what_the_client_missed() {
        let hub = LiveHub::default();
        for position in 1..=5 {
            hub.publish(event(position)).await.expect("publishes");
        }

        let missed = hub
            .replay_after(&Checkpoint::from_global_position(3))
            .await
            .expect("within the buffer");
        let positions: Vec<_> = missed
            .iter()
            .filter_map(|e| e.checkpoint.global_position())
            .collect();
        assert_eq!(positions, vec![4, 5]);
    }

    #[tokio::test]
    async fn a_checkpoint_older_than_the_buffer_reports_a_gap_rather_than_skipping() {
        let hub = LiveHub::new(LiveHubConfig {
            buffer_capacity: 3,
            ..LiveHubConfig::default()
        });
        for position in 1..=10 {
            hub.publish(event(position)).await.expect("publishes");
        }

        let gap = hub
            .replay_after(&Checkpoint::from_global_position(2))
            .await
            .expect_err("the buffer only holds 8, 9, 10");
        assert_eq!(gap.requested, Checkpoint::from_global_position(2));
        assert_eq!(gap.oldest, Checkpoint::from_global_position(8));
    }

    #[tokio::test]
    async fn the_buffer_stays_bounded() {
        let hub = LiveHub::new(LiveHubConfig {
            buffer_capacity: 4,
            ..LiveHubConfig::default()
        });
        for position in 1..=100 {
            hub.publish(event(position)).await.expect("publishes");
        }
        assert_eq!(hub.recent.lock().await.len(), 4);
        assert_eq!(
            hub.head().await,
            Some(Checkpoint::from_global_position(100))
        );
    }

    #[tokio::test]
    async fn a_fresh_client_is_served_from_the_start_of_the_buffer() {
        let hub = LiveHub::default();
        for position in 1..=3 {
            hub.publish(event(position)).await.expect("publishes");
        }

        let all = hub
            .replay_after(&Checkpoint::beginning())
            .await
            .expect("the buffer covers the whole log so far");
        assert_eq!(all.len(), 3, "having seen nothing is not a gap");
    }

    #[tokio::test]
    async fn a_cursor_immediately_before_the_buffer_is_not_a_gap() {
        let hub = LiveHub::new(LiveHubConfig {
            buffer_capacity: 3,
            ..LiveHubConfig::default()
        });
        for position in 1..=5 {
            hub.publish(event(position)).await.expect("publishes");
        }

        // Buffer holds 3, 4, 5. A client at 2 has missed nothing.
        let missed = hub
            .replay_after(&Checkpoint::from_global_position(2))
            .await
            .expect("no gap");
        let positions: Vec<_> = missed
            .iter()
            .filter_map(|e| e.checkpoint.global_position())
            .collect();
        assert_eq!(positions, vec![3, 4, 5]);
    }

    #[tokio::test]
    async fn replaying_against_an_empty_hub_is_not_a_gap() {
        let hub = LiveHub::default();
        assert_eq!(
            hub.replay_after(&Checkpoint::from_global_position(42))
                .await,
            Ok(Vec::new())
        );
    }
}
