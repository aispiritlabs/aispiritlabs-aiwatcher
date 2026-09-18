//! Moving an authored registry into one explicitly named IAM project.
//!
//! Everything in the authored object store was written before projects
//! existed, under keys with no owner in them. IAM-01 gives each registry a
//! scoped namespace — `<prefix>/scopes/<organization>/<project>/registry/` —
//! and the legacy routes keep serving legacy data until somebody moves it.
//! This crate is what an operator runs to move it, and what they read before
//! deciding to.
//!
//! Four ideas carry it.
//!
//! **The owner supplies the keys.** Each registry answers in
//! [`aiwatcher_core::migration`]'s vocabulary: what it holds, where each
//! object goes in a bound scope, in what order they may be written, and what
//! each one points at. This crate composes those answers. It computes no
//! object key, so there is no second implementation of a key layout to drift
//! from the first.
//!
//! **A manifest is a pure function of the snapshot.** [`plan::plan`] reads and
//! hashes; [`manifest::Manifest::manifest_id`] is the digest of everything it
//! concluded. What the *target* holds is a [`manifest::Survey`], kept out of
//! that identity on purpose — a half-finished copy changes the target and must
//! not change the plan, or no resume could bind to it.
//!
//! **Nothing is trusted out of the manifest file.** [`execute::execute`] plans
//! again from the live store and refuses unless it reaches the same id. That
//! one step revalidates every key, detects a source that moved, and detects an
//! object nobody's adapter recognises.
//!
//! **Absence is an answer.** A registry with no adapter is reported as
//! unsupported, a registry that must not be copied as blocked, and a prefix
//! nobody owns as unknown — never as a count of zero. A run that copies every
//! object it planned is [`manifest::Receipt::complete`]; only a run with no
//! blocker at all is [`manifest::Receipt::cutover_ready`].
//!
//! What this crate does **not** do: stop a deployment, change a route, delete
//! a source, create an organization, a team, a membership or a grant, read an
//! identity provider's groups, rewrite a stored byte, or decide that a cutover
//! has happened.

pub mod authority;
pub mod execute;
pub mod family;
pub mod manifest;
pub mod plan;

use aiwatcher_core::migration::InventoryError;
use aiwatcher_core::ports::PortError;

pub type Result<T> = std::result::Result<T, MigrationError>;

#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    /// The source moved under the migration. Nothing is written after this.
    #[error("the snapshot is not the one this plan was taken from: {0}")]
    SnapshotChanged(String),

    #[error("{0}")]
    Refused(String),

    #[error(
        "this manifest no longer matches the id it was reviewed under; re-plan it and review \
         the difference"
    )]
    ManifestEdited,

    #[error("the {family} registry could not describe itself: {source}")]
    Inventory {
        family: String,
        #[source]
        source: InventoryError,
    },

    /// Nothing was overwritten, and nothing will be.
    #[error(
        "{target_key} already holds different bytes (sha256 {target_sha256}); {source_key} was \
         not copied and nothing was overwritten"
    )]
    Conflict {
        source_key: String,
        target_key: String,
        target_sha256: String,
    },

    #[error("{key} read back as bytes other than the ones just written to it")]
    ReadBack { key: String },

    #[error(
        "this store will not publish {key} only when the key is absent ({message}). Either use \
         a store that can, or state that this run has the store to itself — read-then-write is \
         safe under that statement and under nothing else"
    )]
    NoAtomicCreate { key: String, message: String },

    #[error("{0}")]
    Authority(String),

    #[error("nothing was written: {0}")]
    Blocked(String),

    #[error("the checkpoint at {path} is not this run's: {detail}")]
    CheckpointMismatch { path: String, detail: String },

    #[error(transparent)]
    Store(#[from] PortError),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}
