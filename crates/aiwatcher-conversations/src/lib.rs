//! Conversation content, kept on purpose: the one thing here that is
//! **encrypted, separately retained, and erasable**.
//!
//! Everything else authored in this workspace — prompts, curation recipes,
//! annotations, training runs — is kept forever because keeping it is the
//! point. A conversation turn is not like that. It is somebody's words, and it
//! is only in this system because a person decided a model should be trained
//! on them. So the three properties that make the other registries simple are
//! all reversed here: content is encrypted at rest, it expires on a clock that
//! has nothing to do with the event log's retention, and a deletion has to
//! actually delete.
//!
//! **It is not on the event log, and that is the decision.** ADR_0021. Putting
//! `input` and `output` on `llm.completed` — which is what this replaced —
//! writes the bodies into the durable log, into every projector's memory and
//! into whatever the log's retention happens to be, and offers no place to put
//! the consent record that made the capture lawful. The lesson is ADR_0018's,
//! one turn further: a design whose last step is an exception in somebody
//! else's retention policy is a design in the wrong place.
//!
//! ```text
//! producer ── redacts, attaches consent ──► POST /api/v1/conversation-turns
//!                                                     │
//!                    ┌────────────────────────────────┤
//!                    ▼                                ▼
//!            head (plaintext)                 content (encrypted)
//!            role, ordering, policy,          parts and tool results,
//!            findings, review state,          AES-256-GCM under a key
//!            digests — no content             derived per object
//!                    │                                │
//!                    │   review: approve or reject    │
//!                    ▼                                ▼
//!            export job ──► shards (encrypted) + manifest ──► name@sha256
//!                             resumable, ordered, every exclusion named
//! ```
//!
//! # Layout
//!
//! Sliced by noun, the way `aiwatcher-annotations` is, so a change to what one
//! thing *is* touches one directory:
//!
//! ```text
//! turn         the contract: roles, ordering, content parts, tool results
//! policy       consent, retention and what a deployment demands of both
//! redaction    what a producer says it removed, and what the server finds anyway
//! review       the human gate: findings, approval, rejection
//! archive/     SLICE — the encrypted store and its retention clock
//!   crypt        envelope encryption over `ring`, and the keyring rotation
//! export/      SLICE — the asynchronous job that freezes a selection
//!   format       chat, prompt/response, SFT and DPO shapes
//! registry     the facade, and the only public door
//! store        (private) the key layout every slice reads through
//! ```

use aiwatcher_core::ports::{PortError, PortResult};
use aiwatcher_core::prompts::ObjectStore;
use sha2::{Digest, Sha256};

pub mod archive;
pub mod export;
pub mod policy;
pub mod redaction;
pub mod registry;
pub mod review;
mod store;
pub mod turn;

pub use archive::crypt::{Keyring, KeyringError, SealedObject};
pub use archive::{
    ArchivedTurn, ConversationHead, ConversationPage, Erasure, ErasureReason, ErasureReport,
    TurnFilter, TurnPage,
};
pub use export::format::{ExportFormat, PreferenceLabel};
pub use export::{
    ExclusionReason, ExportCounts, ExportExclusion, ExportIndex, ExportJob, ExportJobPage,
    ExportJobSummary, ExportManifest, ExportPage, ExportRequest, ExportRowsPage, ExportSelection,
    ExportVersionSummary, JobState, LEASE_SECONDS, ShardRef, Withdrawal,
};
pub use policy::{
    ArchivePolicy, ConsentRecord, ContentPolicy, LawfulBasis, PolicyMode, RetentionPolicy,
    TrainingScope,
};
pub use redaction::{Finding, FindingKind, RedactionRecord, scan};
pub use registry::Registry;
pub use review::{HumanFinding, ReviewRequest, ReviewState, TurnReview};
pub use turn::{
    ContentPart, PartSummary, Provenance, RecordTurnRequest, RecordedTurn, Role, ToolResult,
    TurnContent, TurnState, turn_id,
};

/// A conversation id, a message id, a subject, a policy reference.
pub const MAX_NAME_BYTES: usize = 256;
/// One turn's content, before encryption. Generous next to a prompt version
/// because a turn can carry a pasted document; small enough that the archive
/// stays a store of messages rather than of files.
pub const MAX_CONTENT_BYTES: usize = 1024 * 1024;
/// Content parts in one turn.
pub const MAX_PARTS: usize = 256;
/// Tool results attached to one turn.
pub const MAX_TOOL_RESULTS: usize = 64;
/// Turns one write may carry. A producer batches a whole exchange; a producer
/// sending ten thousand is sending a corpus and wants an import, not a write.
pub const MAX_TURNS_PER_WRITE: usize = 256;
/// Entries in one index shard. The last shard is the only mutable object in a
/// conversation, which is what keeps appending a turn a bounded write however
/// long the conversation gets.
pub const INDEX_SHARD_ENTRIES: usize = 1_000;
/// Rows in one export shard, and the unit of resume.
pub const EXPORT_SHARD_ROWS: usize = 500;
/// Turns one list request returns.
pub const MAX_TURN_PAGE: usize = 200;
/// Rows one export-rows request returns.
pub const MAX_ROW_PAGE: usize = 200;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),

    /// Content this archive will not take, with every reason rather than the
    /// first — the annotation registry's rule, for the same reason. A producer
    /// fixing one policy problem per round trip gives up and starts sending
    /// `basis: "unknown"`.
    #[error("the turn was refused: {}", .0.join("; "))]
    Rejected(Vec<String>),

    /// A decision this crate made and will make identically again: a promotion
    /// gate, a cancelled job, an export that selected nothing.
    #[error("{0}")]
    Refused(String),

    #[error("{0} was not found")]
    NotFound(String),

    /// The content was there and is not any more. Distinct from
    /// [`Self::NotFound`] on purpose: a tombstone is an answer, and one an
    /// auditor asked for.
    #[error("{0} was erased on {1}")]
    Erased(String, String),

    #[error("{what} is {size}; the limit is {limit}")]
    TooLarge {
        what: &'static str,
        size: usize,
        limit: usize,
    },

    /// The archive has no key that can read this object, or none to write with.
    #[error("{0}")]
    Crypto(#[from] KeyringError),

    #[error("the conversation archive could not use its object store: {0}")]
    Store(#[from] PortError),

    #[error("stored object {key} is not a conversation archive document: {message}")]
    Corrupt { key: String, message: String },
}

pub type Result<T> = std::result::Result<T, Error>;

/// A conversation id, a message id, a job name.
///
/// Slashes are allowed — an export name is `training/agent-turns`, and a
/// conversation id comes from a producer that never heard of this crate — and
/// every use of one in a key is hashed rather than interpolated, so a `..`
/// cannot reach another prefix. The check is still here, because an id that
/// arrives with a newline in it is a log-injection waiting to be printed.
///
/// # Errors
///
/// [`Error::Invalid`] when the value is empty, too long, or holds a control
/// character.
pub fn validate_name(value: &str, what: &str) -> Result<()> {
    if value.is_empty() || value.len() > MAX_NAME_BYTES {
        return Err(Error::Invalid(format!(
            "{what} must be between 1 and {MAX_NAME_BYTES} bytes"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(Error::Invalid(format!(
            "{what} must not contain control characters"
        )));
    }
    Ok(())
}

/// A lowercase SHA-256 in hex: a turn id, a content digest, an export version.
///
/// Checked before it is interpolated into an object key, for the same reason
/// `aiwatcher_annotations::validate_digest` is: a `..` in an identifier is a
/// path traversal into somebody else's data.
///
/// # Errors
///
/// [`Error::Invalid`] when the value is not 64 lowercase hex characters.
pub fn validate_digest(value: &str, what: &str) -> Result<()> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Ok(());
    }
    Err(Error::Invalid(format!(
        "{what} must be a 64-character lowercase SHA-256"
    )))
}

#[must_use]
pub fn digest(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

/// Keep the object-store failure vocabulary consistent with the other
/// registries.
impl From<Error> for PortError {
    fn from(error: Error) -> Self {
        match error {
            Error::Store(error) => error,
            other => Self::Rejected {
                target: "conversation-archive",
                message: other.to_string(),
            },
        }
    }
}

/// A small probe useful to wiring and health checks.
///
/// # Errors
///
/// Whatever the object store says when it cannot be listed.
pub async fn probe(store: &dyn ObjectStore, prefix: &str) -> PortResult<()> {
    store.list(prefix).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_digest_that_could_be_mistaken_for_a_path_is_refused() {
        assert!(validate_digest(&digest(b"anything"), "turn id").is_ok());
        for invalid in [
            "../../etc/passwd",
            "",
            "ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
            "short",
        ] {
            assert!(validate_digest(invalid, "turn id").is_err(), "{invalid}");
        }
    }

    #[test]
    fn a_name_holding_a_control_character_is_refused() {
        assert!(validate_name("training/agent-turns", "name").is_ok());
        assert!(validate_name("conversation 17", "name").is_ok());
        assert!(validate_name("line\nbreak", "name").is_err());
        assert!(validate_name("", "name").is_err());
    }
}
