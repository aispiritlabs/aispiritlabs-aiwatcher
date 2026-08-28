//! An append-only file. One JSON record per line, position = line number.
//!
//! This exists so aiwatcher is useful before anyone commits to Laser. It is a
//! real durable log — a restart replays from disk, a projector resumes from its
//! stored checkpoint — with none of the operational surface of a broker. What
//! it does not do is scale past one writer, and that is fine for the stage it
//! is meant for.
//!
//! The line-number-as-position choice is what keeps it simple: no index file to
//! keep in sync, and a corrupted tail truncates cleanly because a half-written
//! line fails to parse and everything before it is still valid.

use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use tokio::fs::{self, File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncSeekExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;

use aiwatcher_core::stream::StreamPosition;
use aiwatcher_core::{Checkpoint, EventEnvelope, RecordedEvent, StreamName};

use crate::ports::{
    AppendResult, BusError, BusResult, Checkpointer, MessageSink, MessageSource, SourceMessage,
    StartFrom, StreamPage, SubscribeOptions,
};

const BROADCAST_CAPACITY: usize = 4096;
const EVENTS_FILE: &str = "events.jsonl";
const CHECKPOINTS_DIR: &str = "checkpoints";

#[derive(Debug)]
struct Writer {
    file: File,
    /// Byte offset of the start of each record, indexed by global position - 1.
    /// Built once at open, appended on every write, so a resume seeks straight
    /// to the right byte instead of scanning.
    offsets: Vec<u64>,
    /// The same offsets, grouped by stream and indexed by stream position - 1.
    ///
    /// Without it, reading one run's events means scanning the whole log — a
    /// cost that grows with everything else recorded, not with the run. Costs
    /// one more `u64` per event, the same order as `offsets`, which is the
    /// trade this makes on purpose.
    stream_offsets: HashMap<String, Vec<u64>>,
    stream_positions: HashMap<String, u64>,
}

/// A single-node durable log backed by one append-only file.
#[derive(Debug, Clone)]
pub struct FileWal {
    root: PathBuf,
    events_path: PathBuf,
    writer: Arc<Mutex<Writer>>,
    live: broadcast::Sender<RecordedEvent>,
    /// Serialises checkpoint writes; they are tiny and rare.
    checkpoint_lock: Arc<Mutex<()>>,
}

impl FileWal {
    /// Open (or create) a log rooted at `root`, replaying what is already
    /// there to rebuild the offset index and per-stream positions.
    ///
    /// A truncated final line — a crash mid-write — is dropped and the file is
    /// truncated back to the last complete record.
    pub async fn open(root: impl AsRef<Path>) -> BusResult<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root).await?;
        fs::create_dir_all(root.join(CHECKPOINTS_DIR)).await?;
        let events_path = root.join(EVENTS_FILE);

        let (offsets, stream_offsets, stream_positions, valid_len) =
            Self::scan(&events_path).await?;

        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&events_path)
            .await?;
        if file.metadata().await?.len() != valid_len {
            // Drop a partially written trailing record.
            file.set_len(valid_len).await?;
            tracing::warn!(
                path = %events_path.display(),
                truncated_to = valid_len,
                "dropped an incomplete trailing record from the write-ahead log"
            );
        }
        file.seek(SeekFrom::End(0)).await?;

        let (live, _) = broadcast::channel(BROADCAST_CAPACITY);
        Ok(Self {
            root,
            events_path,
            writer: Arc::new(Mutex::new(Writer {
                file,
                offsets,
                stream_offsets,
                stream_positions,
            })),
            live,
            checkpoint_lock: Arc::new(Mutex::new(())),
        })
    }

    /// Returns the offset index, the per-stream offset index, the per-stream
    /// next positions, and the byte length up to which the file parses cleanly.
    #[allow(clippy::type_complexity)]
    async fn scan(
        path: &Path,
    ) -> BusResult<(
        Vec<u64>,
        HashMap<String, Vec<u64>>,
        HashMap<String, u64>,
        u64,
    )> {
        let mut offsets = Vec::new();
        let mut stream_offsets: HashMap<String, Vec<u64>> = HashMap::new();
        let mut stream_positions = HashMap::new();
        let mut valid_len = 0u64;

        let Ok(file) = File::open(path).await else {
            return Ok((offsets, stream_offsets, stream_positions, 0));
        };

        let mut reader = BufReader::new(file);
        let mut line = String::new();
        let mut offset = 0u64;
        loop {
            line.clear();
            let read = reader.read_line(&mut line).await?;
            if read == 0 {
                break;
            }
            // A final line with no newline is a crash artefact, not a record.
            if !line.ends_with('\n') {
                break;
            }
            match serde_json::from_str::<RecordedEvent>(line.trim_end()) {
                Ok(event) => {
                    offsets.push(offset);
                    let stream = event.metadata.stream_name.to_string();
                    stream_offsets
                        .entry(stream.clone())
                        .or_default()
                        .push(offset);
                    stream_positions.insert(stream, event.metadata.stream_position);
                    offset += read as u64;
                    valid_len = offset;
                }
                Err(source) => {
                    return Err(BusError::Decode {
                        checkpoint: Checkpoint::from_global_position(offsets.len() as u64 + 1)
                            .to_string(),
                        source,
                    });
                }
            }
        }
        Ok((offsets, stream_offsets, stream_positions, valid_len))
    }

    /// How many events one stream holds.
    async fn stream_length(&self, stream: &StreamName) -> usize {
        self.writer
            .lock()
            .await
            .stream_offsets
            .get(&stream.to_string())
            .map_or(0, Vec::len)
    }

    /// Byte offsets of one stream's records, skipping `skip` and taking at
    /// most `limit`.
    async fn stream_offsets(&self, stream: &StreamName, skip: usize, limit: usize) -> Vec<u64> {
        self.writer
            .lock()
            .await
            .stream_offsets
            .get(&stream.to_string())
            .map(|offsets| offsets.iter().skip(skip).take(limit).copied().collect())
            .unwrap_or_default()
    }

    /// Read the records sitting at the given byte offsets.
    ///
    /// One file handle for the whole batch. A stream's offsets are ascending,
    /// so the seeks move forward through the file even though the records are
    /// interleaved with every other stream's.
    async fn read_at_offsets(&self, offsets: &[u64]) -> BusResult<Vec<RecordedEvent>> {
        if offsets.is_empty() {
            return Ok(Vec::new());
        }
        let file = File::open(&self.events_path).await?;
        let mut reader = BufReader::new(file);
        let mut events = Vec::with_capacity(offsets.len());
        let mut line = String::new();
        for offset in offsets {
            reader.seek(SeekFrom::Start(*offset)).await?;
            line.clear();
            if reader.read_line(&mut line).await? == 0 || !line.ends_with('\n') {
                break;
            }
            let event: RecordedEvent =
                serde_json::from_str(line.trim_end()).map_err(|source| BusError::Decode {
                    checkpoint: format!("offset {offset}"),
                    source,
                })?;
            events.push(event);
        }
        Ok(events)
    }

    /// Read records with `global_position > after`, at most `limit` of them.
    async fn read_after(&self, after: u64, limit: usize) -> BusResult<Vec<RecordedEvent>> {
        let start_offset = {
            let writer = self.writer.lock().await;
            match writer.offsets.get(after as usize) {
                Some(offset) => *offset,
                // Nothing past `after` yet.
                None => return Ok(Vec::new()),
            }
        };

        let mut file = File::open(&self.events_path).await?;
        file.seek(SeekFrom::Start(start_offset)).await?;
        let mut reader = BufReader::new(file);
        let mut events = Vec::new();
        let mut line = String::new();
        while events.len() < limit {
            line.clear();
            if reader.read_line(&mut line).await? == 0 || !line.ends_with('\n') {
                break;
            }
            let event: RecordedEvent =
                serde_json::from_str(line.trim_end()).map_err(|source| BusError::Decode {
                    checkpoint: Checkpoint::from_global_position(after + events.len() as u64 + 1)
                        .to_string(),
                    source,
                })?;
            events.push(event);
        }
        Ok(events)
    }

    fn checkpoint_path(&self, processor_id: &str) -> PathBuf {
        // Processor ids come from configuration, not from user input, but a
        // path separator would still escape the directory — replace rather
        // than trust.
        let safe: String = processor_id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.root.join(CHECKPOINTS_DIR).join(safe)
    }
}

#[async_trait]
impl MessageSink for FileWal {
    async fn append(&self, events: Vec<EventEnvelope>) -> BusResult<AppendResult> {
        let ingested_at = time::OffsetDateTime::now_utc();
        let mut writer = self.writer.lock().await;
        let base_position = writer.offsets.len() as u64;
        let mut recorded = Vec::with_capacity(events.len());
        let mut buffer = Vec::new();
        // Offset of each record relative to the start of this batch, with the
        // stream it belongs to; turned into absolute offsets once we know where
        // the batch landed.
        let mut relative_offsets: Vec<(String, u64)> = Vec::with_capacity(events.len());

        for envelope in events {
            envelope.validate()?;
            let stream = envelope.stream_name().to_string();
            let stream_position = {
                let next = writer.stream_positions.entry(stream.clone()).or_insert(0);
                *next += 1;
                *next
            };
            let global_position = base_position + recorded.len() as u64 + 1;
            let event = envelope.record(stream_position, global_position, ingested_at, None);

            let line = serde_json::to_string(&event).map_err(|source| BusError::Decode {
                checkpoint: event.metadata.checkpoint.to_string(),
                source,
            })?;
            relative_offsets.push((stream, buffer.len() as u64));
            buffer.extend_from_slice(line.as_bytes());
            buffer.push(b'\n');
            recorded.push(event);
        }

        // One write and one fsync for the whole batch. Durability is per batch,
        // which is the unit a producer already retries.
        let base_offset = writer.file.metadata().await?.len();
        writer.file.write_all(&buffer).await?;
        writer.file.flush().await?;
        writer.file.sync_data().await?;
        for (stream, relative) in relative_offsets {
            let offset = base_offset + relative;
            writer.offsets.push(offset);
            writer
                .stream_offsets
                .entry(stream)
                .or_default()
                .push(offset);
        }

        let last_checkpoint = recorded.last().map_or_else(Checkpoint::beginning, |event| {
            event.metadata.checkpoint.clone()
        });

        // Publish only once the record is on disk: a live subscriber must never
        // see an event a restart would lose.
        drop(writer);
        for event in &recorded {
            let _ = self.live.send(event.clone());
        }

        Ok(AppendResult {
            recorded,
            last_checkpoint,
        })
    }
}

#[async_trait]
impl MessageSource for FileWal {
    async fn subscribe(
        &self,
        options: SubscribeOptions,
    ) -> BusResult<BoxStream<'static, SourceMessage>> {
        let mut live = self.live.subscribe();
        let head = self.writer.lock().await.offsets.len() as u64;

        let from_position = match &options.from {
            StartFrom::Beginning => 0,
            StartFrom::Now => head,
            StartFrom::After(checkpoint) => checkpoint.global_position().unwrap_or(0),
        };

        let (tx, rx) = mpsc::channel(options.batch_size.max(1));
        let this = self.clone();
        let stream_filter = options.stream.clone();
        let batch_size = options.batch_size.max(1);

        tokio::spawn(async move {
            let mut delivered = from_position;
            // Replay in bounded batches so a large backlog does not have to fit
            // in memory at once.
            loop {
                let batch = match this.read_after(delivered, batch_size).await {
                    Ok(batch) => batch,
                    Err(error) => {
                        tracing::error!(%error, "write-ahead log replay failed");
                        return;
                    }
                };
                if batch.is_empty() {
                    break;
                }
                for event in batch {
                    delivered = event.metadata.global_position;
                    if !matches_stream(&event, stream_filter.as_ref()) {
                        continue;
                    }
                    if tx.send(SourceMessage::event(event)).await.is_err() {
                        return;
                    }
                }
            }

            if tx
                .send(SourceMessage::CaughtUp {
                    checkpoint: Checkpoint::from_global_position(delivered),
                })
                .await
                .is_err()
            {
                return;
            }

            loop {
                match live.recv().await {
                    Ok(event) => {
                        if event.metadata.global_position <= delivered {
                            continue;
                        }
                        delivered = event.metadata.global_position;
                        if !matches_stream(&event, stream_filter.as_ref()) {
                            continue;
                        }
                        if tx.send(SourceMessage::event(event)).await.is_err() {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(dropped)) => {
                        tracing::warn!(
                            dropped,
                            "subscriber lagged; ending subscription to force a checkpointed resume"
                        );
                        return;
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    async fn read(&self, from: &Checkpoint, limit: usize) -> BusResult<Vec<RecordedEvent>> {
        self.read_after(from.global_position().unwrap_or(0), limit)
            .await
    }

    async fn read_stream(&self, stream: &StreamName) -> BusResult<Vec<RecordedEvent>> {
        let offsets = self.stream_offsets(stream, 0, usize::MAX).await;
        self.read_at_offsets(&offsets).await
    }

    async fn read_stream_page(
        &self,
        stream: &StreamName,
        after: Option<StreamPosition>,
        limit: usize,
    ) -> BusResult<StreamPage> {
        // `stream_position` is 1-based and contiguous within a stream, so the
        // event after position `p` is the one at index `p`.
        let skip = after.unwrap_or(0) as usize;
        let total = self.stream_length(stream).await;
        let offsets = self.stream_offsets(stream, skip, limit).await;
        let events = self.read_at_offsets(&offsets).await?;
        let has_more = total > skip.saturating_add(events.len());
        Ok(StreamPage::new(events, has_more))
    }

    async fn head(&self) -> BusResult<Checkpoint> {
        let count = self.writer.lock().await.offsets.len() as u64;
        Ok(if count == 0 {
            Checkpoint::beginning()
        } else {
            Checkpoint::from_global_position(count)
        })
    }
}

#[async_trait]
impl Checkpointer for FileWal {
    async fn load(&self, processor_id: &str) -> BusResult<Option<Checkpoint>> {
        let path = self.checkpoint_path(processor_id);
        match fs::read_to_string(&path).await {
            Ok(raw) => Ok(Some(Checkpoint::parse(raw.trim())?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    async fn save(&self, processor_id: &str, checkpoint: &Checkpoint) -> BusResult<()> {
        let _guard = self.checkpoint_lock.lock().await;
        let path = self.checkpoint_path(processor_id);
        // Write-then-rename: a crash mid-write leaves the previous checkpoint
        // intact rather than a truncated one. Resuming from an older checkpoint
        // replays events; resuming from a corrupt one loses them.
        let temporary = path.with_extension("tmp");
        fs::write(&temporary, checkpoint.as_str()).await?;
        fs::rename(&temporary, &path).await?;
        Ok(())
    }
}

fn matches_stream(event: &RecordedEvent, filter: Option<&StreamName>) -> bool {
    filter.is_none_or(|stream| &event.metadata.stream_name == stream)
}
