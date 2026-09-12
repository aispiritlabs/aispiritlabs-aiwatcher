//! What a pod's log is, and where it goes (ADR_0029).
//!
//! **The last [`TAIL_BYTES`], in bytes rather than lines.** One line of a
//! structured logger can be megabytes, so a bound in lines bounds nothing —
//! and the end is the part somebody is reading: a traceback is printed last.
//! What came before is not thrown away silently, it is *counted*, and the
//! stored object says so on its first line.
//!
//! **It is never in the stream and never an output.** An attempt is immutable
//! once it is terminal, and a settlement never waits for its log: the launcher
//! ends the attempt first and reads the log after, so a pod that reported and
//! then printed is still read in full. What connects the two is the catalog
//! row's [`Provenance`], which names the execution, the step and the attempt.
//!
//! **The bytes before the row that indexes them** — [`aiwatcher_jobs::ORDERING`],
//! in the same place it applies to a prompt's head and an export's cursor. A
//! crash the right way round leaves an object nothing points at, which the next
//! pass overwrites identically, because an artifact is named by its own hash.

use std::sync::Arc;

use aiwatcher_core::ArtifactRef;
use aiwatcher_execution::{ArtifactCatalog, AttemptKey, CatalogedArtifact, Provenance};
use time::OffsetDateTime;

use crate::execution::artifacts::Artifacts;

/// How much of one pod's output is kept.
///
/// 256 KiB: enough for a traceback and the work that led to it, and small
/// enough that a fan-out of a hundred pods is not a bucket somebody notices.
/// The kubelet's own rotation (10 MiB by default) bounds what there is to read
/// in the first place.
pub const TAIL_BYTES: usize = 256 * 1024;

/// What a pod printed, bounded — and how much of it was not kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kept {
    pub bytes: Vec<u8>,
    /// The bytes that went past before the ones kept. Counted rather than
    /// forgotten: an object holding the end of something says so.
    pub skipped: u64,
}

/// The last `at_most` bytes of a stream nobody can rewind.
///
/// The cluster's log API can bound the *first* N bytes of a stream and not the
/// last, so the bound is kept here while the stream is read. This is the half
/// of the log path with a rule worth testing, and it compiles in every build
/// for that reason — only the streaming is behind `kube`.
#[derive(Debug)]
pub struct Tail {
    at_most: usize,
    kept: Vec<u8>,
    skipped: u64,
}

impl Tail {
    #[must_use]
    pub fn new(at_most: usize) -> Self {
        Self {
            at_most,
            kept: Vec::new(),
            skipped: 0,
        }
    }

    /// Take one chunk of the stream.
    pub fn push(&mut self, chunk: &[u8]) {
        // A chunk longer than the whole allowance makes everything before it
        // skipped, including what is already held.
        if chunk.len() >= self.at_most {
            let dropped = chunk.len() - self.at_most;
            self.skipped += self.kept.len() as u64 + dropped as u64;
            self.kept.clear();
            self.kept.extend_from_slice(&chunk[dropped..]);
            return;
        }
        self.kept.extend_from_slice(chunk);
        if self.kept.len() > self.at_most {
            let over = self.kept.len() - self.at_most;
            self.kept.drain(..over);
            self.skipped += over as u64;
        }
    }

    #[must_use]
    pub fn finish(self) -> Kept {
        Kept {
            bytes: self.kept,
            skipped: self.skipped,
        }
    }

    /// What has been kept so far, without ending the stream.
    ///
    /// A pod's log is readable while its pod still runs, and a backend holding
    /// the bytes itself has to answer that without giving them up.
    #[must_use]
    pub fn kept(&self) -> Kept {
        Kept {
            bytes: self.kept.clone(),
            skipped: self.skipped,
        }
    }
}

/// What the stored object says before the pod's own first byte.
///
/// Only when something was dropped. A header on a log that is complete would
/// be a line in every log claiming a bound that did not apply.
#[must_use]
pub fn header(kept: &Kept) -> Option<String> {
    (kept.skipped > 0).then(|| {
        format!(
            "aiwatcher: the first {} bytes of this pod's output are not kept; \
             what follows is its last {}.\n",
            kept.skipped,
            kept.bytes.len()
        )
    })
}

/// Where a pod's log is stored and recorded.
///
/// Both halves or neither: the bytes go to the object store the artifacts
/// prefix already uses, and the row that names them goes to the catalog beside
/// it. A deployment with no object store keeps no log, which is the same
/// absence that makes it keep no lineage — said by this being `None` rather
/// than by a branch at every call site.
#[derive(Clone, Debug)]
pub struct Keeper {
    artifacts: Artifacts,
    catalog: Arc<dyn ArtifactCatalog>,
}

impl Keeper {
    /// The keeper this deployment has, if it has one.
    #[must_use]
    pub fn of(
        artifacts: Option<&Artifacts>,
        catalog: Option<&Arc<dyn ArtifactCatalog>>,
    ) -> Option<Self> {
        Some(Self {
            artifacts: artifacts?.clone(),
            catalog: Arc::clone(catalog?),
        })
    }

    /// Store one pod's output and record it against the attempt that printed
    /// it.
    ///
    /// # Errors
    ///
    /// Whatever the object store or the catalog could not do. The caller keeps
    /// the Job on a failure, so the next pass reads it again — and the bytes
    /// are named by their own hash, so reading it twice stores it once.
    pub async fn keep(
        &self,
        key: &AttemptKey,
        kept: &Kept,
        now: OffsetDateTime,
    ) -> Result<ArtifactRef, LogError> {
        let mut body = Vec::with_capacity(kept.bytes.len() + 128);
        if let Some(header) = header(kept) {
            body.extend_from_slice(header.as_bytes());
        }
        body.extend_from_slice(&kept.bytes);

        let artifact = self
            .artifacts
            .put_log(&body)
            .await
            .map_err(|error| LogError::Store(error.to_string()))?;
        self.catalog
            .record(CatalogedArtifact {
                artifact: artifact.clone(),
                produced_by: Some(Provenance {
                    execution_id: key.execution_id.clone(),
                    step_id: key.step_id.clone(),
                    attempt: key.attempt,
                }),
                // A log is what the work said, not what it was made from.
                inputs: Vec::new(),
                created_at: now,
            })
            .await?;
        Ok(artifact)
    }
}

/// Why a log was not kept. Never why an attempt failed: the attempt has
/// already ended by the time this runs.
#[derive(Debug, thiserror::Error)]
pub enum LogError {
    #[error("the log could not be stored: {0}")]
    Store(String),
    #[error("the log was stored and not recorded: {0}")]
    Catalog(#[from] aiwatcher_execution::StoreError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tail_of(at_most: usize, chunks: &[&str]) -> Kept {
        let mut tail = Tail::new(at_most);
        for chunk in chunks {
            tail.push(chunk.as_bytes());
        }
        tail.finish()
    }

    #[test]
    fn a_log_that_fits_is_kept_whole_and_says_nothing_about_a_bound() {
        let kept = tail_of(64, &["starting\n", "done\n"]);
        assert_eq!(kept.bytes, b"starting\ndone\n");
        assert_eq!(kept.skipped, 0);
        assert_eq!(header(&kept), None, "no bound applied, so nothing to say");
    }

    #[test]
    fn a_log_that_outgrew_the_bound_keeps_its_end_and_counts_its_start() {
        // The end is the part somebody is reading: a traceback is printed
        // last.
        let kept = tail_of(10, &["0123456789", "abcde"]);
        assert_eq!(kept.bytes, b"56789abcde");
        assert_eq!(kept.skipped, 5);
        let header = header(&kept).expect("a bounded log says so");
        assert!(
            header.contains("first 5 bytes") && header.contains("last 10"),
            "{header}"
        );
    }

    #[test]
    fn one_chunk_longer_than_the_whole_bound_is_itself_cut_to_its_end() {
        // One line of a structured logger can be megabytes, which is why the
        // bound is in bytes: a chunk is not a unit this may keep whole.
        let kept = tail_of(4, &["ab", "0123456789"]);
        assert_eq!(kept.bytes, b"6789");
        assert_eq!(kept.skipped, 8, "the two held bytes and the six dropped");
    }

    #[test]
    fn the_bound_this_release_keeps_is_bytes_and_not_lines() {
        assert_eq!(TAIL_BYTES, 262_144);
    }
}
