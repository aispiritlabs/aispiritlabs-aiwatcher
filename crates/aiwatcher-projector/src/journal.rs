//! What the period fold reads, kept past the log's retention.
//!
//! A gap the fold finds is events the log no longer held when it came to them
//! ([`crate::period_fold`]), and nothing the fold holds can refill it. This can:
//! a consumer of its own reads the same log — cheap, since it folds nothing —
//! and keeps each stretch of positions it read as a page in the object store,
//! created once, holding the events the fold reads and only what the fold
//! reads of each. When the fold meets a gap it looks here first and folds the
//! pages covering it, in order, as it would have folded them from the log; only
//! what no page covers is written down as missing.
//!
//! A page covers every position from its first to its last — each one read,
//! whether kept or of no use to the fold — and never one this did not read, so
//! a stretch the journal itself found missing is missing from it too. A page is
//! written before the position after it is committed, the checkpoint's own
//! rule, and kept for as many days as the deployment says.
//!
//! Where it runs decides what it survives. Beside the projector it refills a
//! fold that fell behind the log, or started again from further back than the
//! log reaches; it runs in every role, under one group name, so in a split
//! deployment it also outlives either half being down. What no journal read
//! before the log evicted it is a gap for both, and the journal says so when it
//! comes to a position past the one after its last.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use tokio_util::sync::CancellationToken;

use aiwatcher_bus::{Checkpointer, MessageSource, SourceMessage, StartFrom, SubscribeOptions};
use aiwatcher_core::{Checkpoint, RecordedEvent};

use crate::period_fold::PeriodFold;
use crate::periods::{JournalPage, PeriodStore};

/// Positions one page holds at most.
const PAGE_POSITIONS: u64 = 4_096;
/// How long a page waits for more positions before it is kept anyway.
const PAGE_EVERY: Duration = Duration::from_secs(5);
/// How often pages past their days are removed.
const PRUNE_EVERY: Duration = Duration::from_secs(3_600);
/// Positions pages may hold while the store refuses them, before the oldest is
/// let go and the journal says it holds nothing of those.
const MOST_HELD: u64 = 64 * PAGE_POSITIONS;
const DAY: i64 = 86_400;

/// The journal as a consumer of the log.
#[derive(Debug)]
pub struct Journal<S, C> {
    source: Arc<S>,
    checkpointer: Arc<C>,
    store: PeriodStore,
    processor_id: String,
    keep_days: u64,
    cold_start: StartFrom,
}

/// A page still being read.
#[derive(Debug)]
struct Reading {
    page: JournalPage,
    opened: Instant,
}

/// What has been read and is not yet kept: pages the store has not taken, in
/// order, and the one still being read.
#[derive(Debug, Default)]
struct Held {
    waiting: std::collections::VecDeque<JournalPage>,
    reading: Option<Reading>,
    /// The last position a kept or waiting page holds: a redelivery at or
    /// before it is already paged.
    through: Option<u64>,
}

impl Held {
    fn positions(&self) -> u64 {
        self.waiting
            .iter()
            .chain(self.reading.as_ref().map(|open| &open.page))
            .map(|page| page.last - page.first + 1)
            .sum()
    }
}

impl<S, C> Journal<S, C>
where
    S: MessageSource + 'static,
    C: Checkpointer + 'static,
{
    /// A journal reading `source` as `processor_id`, keeping pages for
    /// `keep_days`, and starting at `cold_start` when it has read nothing yet.
    pub fn new(
        source: Arc<S>,
        checkpointer: Arc<C>,
        store: PeriodStore,
        processor_id: impl Into<String>,
        keep_days: u64,
        cold_start: StartFrom,
    ) -> Self {
        Self {
            source,
            checkpointer,
            store,
            processor_id: processor_id.into(),
            keep_days: keep_days.max(1),
            cold_start,
        }
    }

    /// Read the log and keep what the fold reads of it until `shutdown`.
    ///
    /// # Errors
    ///
    /// The log's refusal to be read. A store that refuses a page is waited
    /// out: the page is kept once it takes it, and nothing past it is committed
    /// before then.
    pub async fn run(
        self: Arc<Self>,
        shutdown: CancellationToken,
    ) -> Result<(), aiwatcher_bus::BusError> {
        if !self.source.positions_are_contiguous() {
            tracing::warn!(
                "the observation journal needs a log that numbers every event one after the last; this one does not, so it keeps nothing"
            );
            return Ok(());
        }
        let mut held = Held::default();
        let from = match self.checkpointer.load(&self.processor_id).await? {
            Some(checkpoint) => {
                // Paged through there already: a redelivery before it is
                // nothing, and a first position past the one after it is
                // what the log evicted while no journal read it.
                held.through = checkpoint.global_position();
                StartFrom::After(checkpoint)
            }
            None => self.cold_start.clone(),
        };
        tracing::info!(
            processor_id = self.processor_id,
            ?from,
            "observation journal starting"
        );
        let mut stream = self
            .source
            .subscribe(
                SubscribeOptions::from(from)
                    .in_group(self.processor_id.clone())
                    .with_batch_size(512),
            )
            .await?;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut pruned: Option<Instant> = None;
        loop {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => {
                    self.close(&mut held);
                    self.keep(&mut held).await;
                    return Ok(());
                }
                message = stream.next() => match message {
                    Some(SourceMessage::Event(event)) => {
                        if self.read(&mut held, &event) {
                            self.keep(&mut held).await;
                        }
                    }
                    Some(SourceMessage::CaughtUp { .. }) => {
                        self.close(&mut held);
                        self.keep(&mut held).await;
                    }
                    None => {
                        self.close(&mut held);
                        self.keep(&mut held).await;
                        return Ok(());
                    }
                },
                _ = tick.tick() => {
                    if held.reading.as_ref().is_some_and(|open| open.opened.elapsed() >= PAGE_EVERY) {
                        self.close(&mut held);
                    }
                    self.keep(&mut held).await;
                    if pruned.is_none_or(|at| at.elapsed() >= PRUNE_EVERY) {
                        pruned = Some(Instant::now());
                        self.prune().await;
                    }
                }
            }
        }
    }

    /// One position read: on the page being read when it follows its last,
    /// on a new one when it does not — a page never claims a position it did
    /// not read. `true` when a page closed and is waiting to be kept.
    fn read(&self, held: &mut Held, event: &RecordedEvent) -> bool {
        let position = event.metadata.global_position;
        let bound = event
            .metadata
            .occurred_at
            .min(event.metadata.ingested_at)
            .unix_timestamp();
        if held.through.is_some_and(|through| position <= through) {
            return false;
        }
        let last_read = held
            .reading
            .as_ref()
            .map(|open| open.page.last)
            .or(held.through);
        if let Some(last) = last_read.filter(|last| position > last + 1) {
            tracing::warn!(
                processor_id = self.processor_id,
                first = last + 1,
                last = position - 1,
                "the log no longer held these positions when the observation journal came to them, so no page holds them"
            );
        }
        let mut closed = false;
        if let Some(open) = held.reading.as_ref() {
            if position <= open.page.last {
                return false;
            }
            if position != open.page.last + 1 {
                self.close(held);
                closed = true;
            }
        }
        let open = held.reading.get_or_insert_with(|| Reading {
            page: JournalPage {
                first: position,
                last: position.saturating_sub(1),
                from: bound,
                clock: bound,
                events: Vec::new(),
            },
            opened: Instant::now(),
        });
        open.page.last = position;
        open.page.clock = open.page.clock.max(bound);
        if PeriodFold::reads(event) {
            open.page.events.push(PeriodFold::kept(event));
        }
        if open.page.last - open.page.first + 1 >= PAGE_POSITIONS {
            self.close(held);
            closed = true;
        }
        closed
    }

    /// Stop reading into the open page; it waits to be kept.
    fn close(&self, held: &mut Held) {
        if let Some(open) = held.reading.take() {
            held.through = Some(open.page.last);
            held.waiting.push_back(open.page);
        }
        // A store refusing pages for long enough lets the oldest go rather
        // than hold the log in memory; the journal then says nothing of them.
        while held.positions() > MOST_HELD {
            let Some(dropped) = held.waiting.pop_front() else {
                break;
            };
            tracing::error!(
                processor_id = self.processor_id,
                first = dropped.first,
                last = dropped.last,
                "the observation journal could not keep a page for so long that it let it go; it holds nothing of those positions"
            );
        }
    }

    /// Keep the waiting pages in order, committing through each one kept, and
    /// stop at the first the store refuses.
    async fn keep(&self, held: &mut Held) {
        while let Some(page) = held.waiting.front() {
            if let Err(error) = self.store.write_page(page).await {
                tracing::warn!(%error, first = page.first, last = page.last, "the observation journal could not keep a page; trying again");
                return;
            }
            let last = page.last;
            held.waiting.pop_front();
            if let Err(error) = self
                .checkpointer
                .save(&self.processor_id, &Checkpoint::from_global_position(last))
                .await
            {
                tracing::warn!(%error, last, "the observation journal kept a page and could not commit past it");
            }
        }
    }

    async fn prune(&self) {
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let before = now - i64::try_from(self.keep_days).unwrap_or(i64::MAX / DAY) * DAY;
        match self.store.prune_journal(before).await {
            Ok(0) => {}
            Ok(removed) => {
                tracing::info!(
                    removed,
                    "the observation journal removed pages past their days"
                );
            }
            Err(error) => {
                tracing::warn!(%error, "the observation journal could not remove pages past their days");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use aiwatcher_bus::MessageSink;
    use aiwatcher_bus::adapters::memory::InMemoryBus;
    use aiwatcher_core::{EventEnvelope, EventType, Sdk, Source};
    use time::macros::datetime;

    use super::*;
    use crate::periods::tests::MemoryObjectStore;

    fn envelope(run_id: &str, event_type: EventType, seconds: i64, variant: bool) -> EventEnvelope {
        let at = datetime!(2026-09-13 09:00:00 UTC) + time::Duration::seconds(seconds);
        let mut envelope =
            EventEnvelope::new(event_type, run_id, at, Source::new("app", Sdk::Python))
                .with_data(serde_json::json!({"model": "m", "prompt_tokens": 3, "said": "words"}));
        if variant {
            envelope.variant_id = Some("v1".to_owned());
        }
        envelope
    }

    #[tokio::test]
    async fn a_journal_pages_every_position_it_reads_and_keeps_only_what_the_fold_reads() {
        let bus = Arc::new(InMemoryBus::new());
        bus.append(vec![
            envelope("r1", EventType::RunStarted, 1, true),
            envelope("r1", EventType::LlmStarted, 2, true),
            envelope("r1", EventType::LlmChunk, 3, true),
            envelope("other", EventType::RunStarted, 3, false),
            envelope("r1", EventType::LlmCompleted, 4, true),
            envelope("r1", EventType::RunCompleted, 5, true),
        ])
        .await
        .unwrap();
        let objects = Arc::new(MemoryObjectStore::default());
        let store = PeriodStore::new(objects);
        let journal = Arc::new(Journal::new(
            Arc::clone(&bus),
            Arc::clone(&bus),
            store.clone(),
            "projector-journal",
            7,
            StartFrom::Beginning,
        ));
        let shutdown = CancellationToken::new();
        let running = tokio::spawn(Arc::clone(&journal).run(shutdown.clone()));
        let deadline = Instant::now() + Duration::from_secs(5);
        while bus.load("projector-journal").await.unwrap().is_none() {
            assert!(Instant::now() < deadline, "the journal kept nothing");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        shutdown.cancel();
        running.await.unwrap().unwrap();

        let pages = store.pages(1, 6).await.unwrap();
        assert_eq!(pages.len(), 1);
        let page = &pages[0];
        assert_eq!((page.first, page.last), (1, 6), "every position read");
        let kept: Vec<&str> = page
            .events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect();
        assert_eq!(
            kept,
            [
                "run.started",
                "llm.started",
                "llm.completed",
                "run.completed"
            ],
            "no chunk, and nothing of a run naming no variant"
        );
        assert_eq!(
            page.events[1].data,
            serde_json::json!({"model": "m", "prompt_tokens": 3}),
            "nothing said in the run"
        );
        assert_eq!(
            bus.load("projector-journal").await.unwrap(),
            Some(Checkpoint::from_global_position(6))
        );
    }
}
