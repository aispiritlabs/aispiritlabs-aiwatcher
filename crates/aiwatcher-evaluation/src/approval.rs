//! What an operator admitted, as a resource rather than a directory.
//!
//! An approval names one pinned pair — a variant and the context it was
//! measured in — and nothing about a single run. Every repetition of that pair
//! publishes under the same approval, so admitting a second variant cannot
//! disturb the first and a producer never needs a step on the server's host.
//!
//! Its ID is derived from the pair, so two operators who admit the same
//! declaration reach the same approval instead of two rows meaning one thing.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{Result, SCHEMA_VERSION, digest};

/// Where an operator stages the bytes an adapter admits a pair by.
///
/// The registry never reads them: it records the digest the adapter computed
/// and refuses a publication whose bundle has moved since. The port exists so
/// the route that *accepts* them belongs to whoever owns the prefix they land
/// in — the adapter — while the route itself sits beside the approval it is
/// for. Absent, a new pair still needs its bytes on the host's disk.
#[async_trait]
pub trait ApprovalBundles: Send + Sync + std::fmt::Debug {
    /// Replace one member. Staging after an approval does not widen it: the
    /// recorded digest no longer matches, and every read of that pair is
    /// refused until the bytes are what was admitted.
    async fn stage(&self, approval_id: &str, name: &str, bytes: Vec<u8>) -> Result<StagedFile>;
    async fn staged(&self, approval_id: &str) -> Result<Vec<StagedFile>>;
    /// Remove every member. Answers how many there were.
    async fn discard(&self, approval_id: &str) -> Result<usize>;
}

/// One member of a staged bundle. Never its content, and never a digest: the
/// declaration pins one for every member and the approval pins one for the
/// bundle, so a third copy of that fact could only disagree with them.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct StagedFile {
    pub name: String,
    pub size_bytes: u64,
}

/// What an adapter records for the bytes a bundle adds beyond the manifest's
/// own pins, as a content address over their parsed form.
///
/// Over what they *say*, so re-staging the same declaration in other whitespace
/// is not a change. And never over the bundle's `manifest.json`: the pair it
/// declares is already the approval's address, and the rest of it — which run
/// wrote it — differs for every run of that pair, so digesting it made staging
/// the second run's manifest hide every result the pair had published.
pub fn bundle_digest<T: Serialize>(added: &T) -> Result<String> {
    digest(&(SCHEMA_VERSION, "evaluation.bundle", added))
}

/// The content address of a pinned pair. Derived, never generated: a redelivered
/// approval of one declaration lands on the approval it already made.
pub fn approval_id(variant_id: &str, context_id: &str) -> Result<String> {
    digest(&(
        SCHEMA_VERSION,
        "evaluation.approval",
        variant_id,
        context_id,
    ))
}

/// Immutable once written. What it records is the act, not the bytes: the
/// declaration's own digests are the variant and context IDs it is addressed by.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRecord {
    pub approval_id: String,
    pub variant_id: String,
    pub context_id: String,
    /// What the deployment adapter verified *beyond* the manifest's own pinned
    /// digests — a model package, whose historical ID binds artifacts and not
    /// the whole declaration. `None` where an adapter has nothing to add. A
    /// record written before [`bundle_digest`] holds the digest of the whole
    /// staged declaration instead, and still admits while those bytes stand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_digest: Option<String>,
    pub approved_by: String,
    pub approved_at: i64,
}

/// Withdrawal is final for its approval ID. Hiding evidence is what it does;
/// it never shortens or extends the retention of what was already published.
///
/// Named `ApprovalWithdrawal` in the contract: an OpenAPI components block is
/// one global namespace and a conversation corpus has a `Withdrawal` of its
/// own, so the plain name silently described this one with that one's fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = ApprovalWithdrawal)]
#[serde(deny_unknown_fields)]
pub struct Withdrawal {
    pub withdrawn_by: String,
    pub withdrawn_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Approval {
    pub record: ApprovalRecord,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withdrawn: Option<Withdrawal>,
}

impl Approval {
    #[must_use]
    pub fn admits(&self) -> bool {
        self.withdrawn.is_none()
    }
}

/// An operator's admission of a whole line of variants: every variant of one
/// experiment, measured in one context.
///
/// A regression gate measures a new variant on every commit — new code, new
/// prompt — against a context that does not move. Admitting each of those
/// pairs by hand is a person in every pipeline; admitting none is a producer
/// admitting its own evidence. A line is the operator deciding once what may
/// be measured and how — the cohort, the card, the scorer, all in the context
/// — and leaving which variant to the pipeline. It covers only evidence this
/// deployment measures, whose numbers the server computes rather than a
/// producer sends. Each variant it admits still gets its own approval, made
/// when a run of it starts, from its bytes, and naming the line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ApprovalLineRecord {
    pub line_id: String,
    pub context_id: String,
    pub experiment_id: String,
    pub admitted_by: String,
    pub admitted_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ApprovalLine {
    pub record: ApprovalLineRecord,
    /// A withdrawn line admits no further variant. What it admitted stays
    /// admitted, each withdrawable by its own approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withdrawn: Option<Withdrawal>,
}

impl ApprovalLine {
    #[must_use]
    pub fn admits(&self) -> bool {
        self.withdrawn.is_none()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ApprovalLinePage {
    pub lines: Vec<ApprovalLine>,
}

/// The address of a line: derived from what it admits, so admitting it twice
/// lands on one.
pub fn line_id(context_id: &str, experiment_id: &str) -> Result<String> {
    digest(&(
        SCHEMA_VERSION,
        "evaluation.approval_line",
        context_id,
        experiment_id,
    ))
}

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ApprovalPage {
    pub approvals: Vec<Approval>,
}
