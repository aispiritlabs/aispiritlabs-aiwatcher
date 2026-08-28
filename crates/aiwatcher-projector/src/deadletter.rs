//! Where events that cannot be processed go.
//!
//! A dead letter queue exists so one bad event cannot stall a partition. The
//! rule the pipeline follows: retry while the failure is transient, park once
//! it is not, and never block the stream on a payload that will fail the same
//! way forever.
//!
//! Parked events keep their raw bytes. A parsed copy would have lost exactly
//! the thing that needs looking at.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use aiwatcher_core::ports::{DeadLetter, DeadLetterSink, PortError, PortResult};

/// Keeps parked events in memory. Tests, and a deployment that would rather
/// alert than persist.
#[derive(Debug, Default)]
pub struct InMemoryDeadLetters {
    parked: Mutex<Vec<DeadLetter>>,
}

impl InMemoryDeadLetters {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn parked(&self) -> Vec<DeadLetter> {
        self.parked.lock().await.clone()
    }

    pub async fn len(&self) -> usize {
        self.parked.lock().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.parked.lock().await.is_empty()
    }
}

#[async_trait]
impl DeadLetterSink for InMemoryDeadLetters {
    async fn park(&self, letter: DeadLetter) -> PortResult<()> {
        tracing::error!(
            checkpoint = %letter.checkpoint,
            reason = %letter.reason,
            attempts = letter.attempts,
            "parking an event in the dead letter queue"
        );
        self.parked.lock().await.push(letter);
        Ok(())
    }
}

/// Appends parked events to a JSONL file, one per line.
#[derive(Debug, Clone)]
pub struct FileDeadLetters {
    path: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl FileDeadLetters {
    pub async fn open(path: impl AsRef<Path>) -> PortResult<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|source| PortError::Other {
                    target: "dead-letters",
                    source: Box::new(source),
                })?;
        }
        Ok(Self {
            path,
            lock: Arc::new(Mutex::new(())),
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[async_trait]
impl DeadLetterSink for FileDeadLetters {
    async fn park(&self, letter: DeadLetter) -> PortResult<()> {
        tracing::error!(
            checkpoint = %letter.checkpoint,
            reason = %letter.reason,
            attempts = letter.attempts,
            path = %self.path.display(),
            "parking an event in the dead letter queue"
        );
        let line = serde_json::to_string(&letter).map_err(|source| PortError::Other {
            target: "dead-letters",
            source: Box::new(source),
        })?;

        let _guard = self.lock.lock().await;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await
            .map_err(|source| PortError::Other {
                target: "dead-letters",
                source: Box::new(source),
            })?;
        file.write_all(line.as_bytes())
            .await
            .and(file.write_all(b"\n").await)
            .map_err(|source| PortError::Other {
                target: "dead-letters",
                source: Box::new(source),
            })?;
        // Parked events are the ones worth not losing to a crash.
        file.sync_data().await.map_err(|source| PortError::Other {
            target: "dead-letters",
            source: Box::new(source),
        })
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use aiwatcher_core::Checkpoint;

    use super::*;

    fn letter() -> DeadLetter {
        DeadLetter {
            checkpoint: Checkpoint::from_global_position(7),
            raw: "{\"event_type\":\"llm.completed\",".to_owned(),
            reason: "unexpected end of input".to_owned(),
            attempts: 3,
            parked_at: datetime!(2026-08-27 18:20:11 UTC),
        }
    }

    #[tokio::test]
    async fn parked_events_keep_their_raw_bytes() {
        let sink = InMemoryDeadLetters::new();
        assert!(sink.is_empty().await);
        sink.park(letter()).await.expect("parks");

        let parked = sink.parked().await;
        assert_eq!(parked.len(), 1);
        assert_eq!(
            parked[0].raw, "{\"event_type\":\"llm.completed\",",
            "the truncated payload is what needs inspecting"
        );
        assert_eq!(parked[0].attempts, 3);
    }

    #[tokio::test]
    async fn the_file_sink_appends_one_json_line_per_letter() {
        let dir = std::env::temp_dir().join(format!(
            "aiwatcher-dlq-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let path = dir.join("dead-letters.jsonl");
        let sink = FileDeadLetters::open(&path).await.expect("opens");
        sink.park(letter()).await.expect("parks");
        sink.park(letter()).await.expect("parks");

        let content = std::fs::read_to_string(&path).expect("reads");
        assert_eq!(content.lines().count(), 2);
        let first: serde_json::Value =
            serde_json::from_str(content.lines().next().expect("a line")).expect("valid json");
        assert_eq!(first["reason"], "unexpected end of input");
        std::fs::remove_dir_all(&dir).ok();
    }
}
