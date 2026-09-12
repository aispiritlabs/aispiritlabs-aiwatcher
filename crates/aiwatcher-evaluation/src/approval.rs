//! What an operator admitted, as a resource rather than a directory.
//!
//! An approval names one pinned pair — a variant and the context it was
//! measured in — and nothing about a single run. Every repetition of that pair
//! publishes under the same approval, so admitting a second variant cannot
//! disturb the first and a producer never needs a step on the server's host.
//!
//! Its ID is derived from the pair, so two operators who admit the same
//! declaration reach the same approval instead of two rows meaning one thing.

use serde::{Deserialize, Serialize};

use crate::{Result, SCHEMA_VERSION, digest};

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
    /// the whole declaration. `None` where an adapter has nothing to add.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_digest: Option<String>,
    pub approved_by: String,
    pub approved_at: i64,
}

/// Withdrawal is final for its approval ID. Hiding evidence is what it does;
/// it never shortens or extends the retention of what was already published.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
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

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ApprovalPage {
    pub approvals: Vec<Approval>,
}
