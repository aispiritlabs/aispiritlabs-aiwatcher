//! Live streaming, and the reconnect that makes it trustworthy.
//!
//! ## The gap problem
//!
//! A browser tab loses its connection for four seconds. Three things could
//! happen when it comes back:
//!
//! * it resumes and silently misses those four seconds — the failure mode that
//!   makes a live view untrustworthy, because nothing looks wrong;
//! * it reloads the whole run — correct but slow, and it loses scroll position;
//! * it says where it got to and is handed exactly what it missed.
//!
//! The third is what this implements. SSE has the mechanism built in: the
//! browser resends the last `id:` it saw as `Last-Event-ID`, with no
//! application code. So every SSE frame carries the event's checkpoint as its
//! id, and a reconnect resumes from it.
//!
//! ## Where the missed events come from
//!
//! The live hub's ring buffer first, because that is a memory read. If the
//! client was away long enough that the buffer has scrolled past — a laptop
//! that slept — the hub reports a gap and the handler falls back to the durable
//! log. Either way the client gets a contiguous stream.

use std::convert::Infallible;

use axum::response::sse::Event as SseEvent;
use futures::stream::{Stream, StreamExt};

use aiwatcher_core::Checkpoint;
use aiwatcher_core::ports::LiveEvent;
use aiwatcher_projector::LiveHub;

use crate::error::ApiResult;
use crate::state::AppState;

/// A frame in the live stream.
///
/// `Caught` is not decoration: it is what lets the panel switch from "loading
/// history" to "live" at the right moment, instead of guessing from a timeout.
/// It mirrors Emmett's `MessageSourceCaughtUp` control message.
#[derive(Clone, Debug, serde::Serialize, utoipa::ToSchema)]
#[serde(tag = "frame", rename_all = "snake_case")]
pub enum LiveFrame {
    /// A replayed or live event.
    Event(Box<LiveEvent>),
    /// The replay is finished; everything after this is live.
    Caught { checkpoint: Checkpoint },
    /// The requested cursor was too old to replay from memory and the log was
    /// read instead. Surfaced so the panel can say so rather than pretend the
    /// stream was continuous.
    Resynced { from: Checkpoint },
}

impl LiveFrame {
    fn checkpoint(&self) -> Option<&Checkpoint> {
        match self {
            Self::Event(event) => Some(&event.checkpoint),
            Self::Caught { checkpoint } => Some(checkpoint),
            Self::Resynced { .. } => None,
        }
    }

    /// Render as an SSE frame, tagging it with the checkpoint so the browser
    /// sends it back as `Last-Event-ID` after a drop.
    pub fn to_sse(&self) -> Result<SseEvent, axum::Error> {
        let event = SseEvent::default()
            .event(match self {
                Self::Event(_) => "event",
                Self::Caught { .. } => "caught_up",
                Self::Resynced { .. } => "resynced",
            })
            .json_data(self)?;
        Ok(match self.checkpoint() {
            Some(checkpoint) if !checkpoint.is_beginning() => event.id(checkpoint.to_string()),
            _ => event,
        })
    }
}

/// Everything the client missed, in order, ready to be followed by the live
/// tail.
///
/// Returns the frames plus the checkpoint the caller should treat as the
/// boundary — live events at or below it were already delivered here.
pub async fn catch_up(
    state: &AppState,
    from: Option<&Checkpoint>,
    run_filter: Option<&str>,
) -> ApiResult<(Vec<LiveFrame>, Checkpoint)> {
    let Some(from) = from else {
        // No cursor means live only. The panel fetches history with
        // `GET /api/v1/runs/{id}` and opens the stream at the
        // `summary.last_checkpoint` that response carried, so replaying here
        // would send everything twice. A client that really does want the
        // whole run streamed sends `Checkpoint::beginning`, which is a
        // cursor and takes the branch below.
        let head = state
            .live
            .head()
            .await
            .unwrap_or_else(Checkpoint::beginning);
        return Ok((
            vec![LiveFrame::Caught {
                checkpoint: head.clone(),
            }],
            head,
        ));
    };

    let mut frames = Vec::new();
    let missed = match state.live.replay_after(from).await {
        Ok(events) => events,
        Err(gap) => {
            // The buffer scrolled past. Read the durable log instead of
            // pretending the stream was continuous.
            tracing::info!(
                requested = %gap.requested,
                oldest = %gap.oldest,
                "live buffer gap; resyncing from the log"
            );
            frames.push(LiveFrame::Resynced { from: from.clone() });
            replay_from_log(state, from).await?
        }
    };

    let mut boundary = from.clone();
    for event in missed {
        if !matches_run(&event, run_filter) {
            continue;
        }
        boundary = event.checkpoint.clone();
        frames.push(LiveFrame::Event(Box::new(event)));
    }
    frames.push(LiveFrame::Caught {
        checkpoint: boundary.clone(),
    });
    Ok((frames, boundary))
}

/// How many events a resync will read out of the log before giving up. A
/// client that is further behind than this is better served by reloading the
/// run than by streaming a huge backlog through a WebSocket.
const MAX_RESYNC_EVENTS: usize = 10_000;

async fn replay_from_log(state: &AppState, from: &Checkpoint) -> ApiResult<Vec<LiveEvent>> {
    let events = state.source.read(from, MAX_RESYNC_EVENTS).await?;
    if events.len() == MAX_RESYNC_EVENTS {
        tracing::warn!(
            %from,
            limit = MAX_RESYNC_EVENTS,
            "resync hit its cap; the client is further behind than the stream can carry"
        );
    }
    Ok(events.iter().map(LiveEvent::from).collect())
}

/// The live tail, filtered and de-overlapped against what catch-up delivered.
pub fn live_tail(
    live: &LiveHub,
    after: Checkpoint,
    run_filter: Option<String>,
) -> impl Stream<Item = LiveFrame> + Send + use<> {
    live.stream().filter_map(move |result| {
        let after = after.clone();
        let run_filter = run_filter.clone();
        async move {
            let event = match result {
                Ok(event) => event,
                // Lagged past the broadcast capacity. Ending the stream is the
                // honest move: the client reconnects with its last id and the
                // gap is filled properly.
                Err(error) => {
                    tracing::warn!(%error, "live subscriber lagged; closing the stream");
                    return None;
                }
            };
            if event.checkpoint <= after {
                // Already delivered during catch-up.
                return None;
            }
            if !matches_run(&event, run_filter.as_deref()) {
                return None;
            }
            Some(LiveFrame::Event(Box::new(event)))
        }
    })
}

/// Convert a frame stream into SSE, dropping anything that fails to serialise
/// rather than tearing the connection down over one bad frame.
pub fn as_sse(
    frames: impl Stream<Item = LiveFrame> + Send + 'static,
) -> impl Stream<Item = Result<SseEvent, Infallible>> + Send + 'static {
    frames.filter_map(|frame| async move {
        match frame.to_sse() {
            Ok(event) => Some(Ok(event)),
            Err(error) => {
                tracing::error!(%error, "dropping an unserialisable live frame");
                None
            }
        }
    })
}

fn matches_run(event: &LiveEvent, run_filter: Option<&str>) -> bool {
    run_filter.is_none_or(|run_id| event.run_id == run_id)
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use aiwatcher_core::{EventType, SpanId, TraceId};

    use super::*;

    /// Render one frame the way the wire sees it, by driving it through the
    /// real SSE response body. Asserting on `Debug` output would test axum's
    /// formatting rather than ours.
    async fn render(frame: &LiveFrame) -> String {
        use axum::response::IntoResponse;
        use http_body_util::BodyExt;

        let event = frame.to_sse().expect("serialises");
        let response = axum::response::sse::Sse::new(futures::stream::once(async move {
            Ok::<_, Infallible>(event)
        }))
        .keep_alive(
            axum::response::sse::KeepAlive::new().interval(std::time::Duration::from_secs(3600)),
        )
        .into_response();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("collects")
            .to_bytes();
        String::from_utf8(bytes.to_vec()).expect("utf-8")
    }

    fn live_event(position: u64, run_id: &str) -> LiveEvent {
        let trace_id = TraceId::derive(run_id);
        LiveEvent {
            checkpoint: Checkpoint::from_global_position(position),
            run_id: run_id.to_owned(),
            conversation_id: None,
            trace_id,
            span_id: SpanId::derive(trace_id, "run"),
            event_type: EventType::LlmChunk,
            sequence: Some(position),
            occurred_at: datetime!(2026-08-27 18:20:11 UTC),
            data: serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn an_sse_frame_carries_the_checkpoint_as_its_id() {
        let frame = LiveFrame::Event(Box::new(live_event(42, "run-1")));
        let rendered = render(&frame).await;

        assert!(
            rendered.contains(&format!("id: {}", Checkpoint::from_global_position(42))),
            "the browser resends this as Last-Event-ID: {rendered}"
        );
        assert!(rendered.contains("event: event"));
    }

    #[tokio::test]
    async fn the_catch_up_marker_is_its_own_sse_event_type() {
        let frame = LiveFrame::Caught {
            checkpoint: Checkpoint::from_global_position(7),
        };
        let rendered = render(&frame).await;
        assert!(rendered.contains("event: caught_up"), "{rendered}");
        assert!(rendered.contains("\"frame\":\"caught\""), "{rendered}");
    }

    #[tokio::test]
    async fn a_frame_at_the_beginning_carries_no_id() {
        let frame = LiveFrame::Caught {
            checkpoint: Checkpoint::beginning(),
        };
        let rendered = render(&frame).await;
        assert!(
            !rendered.contains("id:"),
            "an empty id would confuse the browser: {rendered}"
        );
    }

    #[tokio::test]
    async fn the_live_tail_skips_what_catch_up_already_delivered() {
        let hub = LiveHub::default();
        let tail = live_tail(&hub, Checkpoint::from_global_position(2), None);
        futures::pin_mut!(tail);

        for position in 1..=4 {
            aiwatcher_core::ports::LivePublisher::publish(&hub, live_event(position, "run-1"))
                .await
                .expect("publishes");
        }

        let first = tail.next().await.expect("a frame");
        let LiveFrame::Event(event) = first else {
            panic!("expected an event frame");
        };
        assert_eq!(
            event.checkpoint,
            Checkpoint::from_global_position(3),
            "1 and 2 were already sent during catch-up"
        );
    }

    #[tokio::test]
    async fn a_run_filter_keeps_other_runs_out_of_the_stream() {
        let hub = LiveHub::default();
        let tail = live_tail(&hub, Checkpoint::beginning(), Some("run-a".to_owned()));
        futures::pin_mut!(tail);

        for (position, run) in [(1, "run-b"), (2, "run-a"), (3, "run-b")] {
            aiwatcher_core::ports::LivePublisher::publish(&hub, live_event(position, run))
                .await
                .expect("publishes");
        }

        let LiveFrame::Event(event) = tail.next().await.expect("a frame") else {
            panic!("expected an event frame");
        };
        assert_eq!(event.run_id, "run-a");
    }
}
