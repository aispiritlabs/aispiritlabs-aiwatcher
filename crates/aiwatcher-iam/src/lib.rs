//! IAM-01's control plane. These stores authorize their own metadata operations.
//!
//! They do not yet isolate the application's datasets, events, artifacts or
//! workers. Do not activate a multi-organization UI until every data plane
//! adapter enforces the returned project scope. See this crate's README.

pub mod export;
pub mod memory;
mod model;
mod policy;
#[cfg(feature = "postgres")]
pub mod postgres;
pub mod retention;

pub use aiwatcher_jobs::{JobState, ShardRef};
use async_trait::async_trait;
pub use export::{
    AuditExportCounts, AuditExportJob, AuditExportRequest, AuditExportRowsPage, AuditExports,
};
pub use model::*;
pub use retention::{AuditBounds, AuditRetention, AuditWatermark, PruneReport};
use sha2::{Digest, Sha256};
use std::fmt::Debug;
use uuid::Uuid;

/// A new invitation token, and nothing that can show it again.
///
/// 244 bits from two version-4 UUIDs rather than a dependency on a random
/// number generator: `uuid` is already here and draws from the same system
/// source. The plaintext is returned once, to the caller that made the offer;
/// only [`digest_of`] is stored.
#[must_use]
pub fn mint_invitation_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// The one form of a token this crate keeps.
#[must_use]
pub fn digest_of(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the requested IAM resource is not available to this principal")]
    NotFound,
    #[error("the operation needs a higher IAM role")]
    Forbidden,
    #[error("an organization must retain at least one owner")]
    LastOwner,
    #[error("the invitation has expired")]
    Expired,
    #[error("the invitation has already been redeemed")]
    Redeemed,
    #[error("invalid IAM input: {0}")]
    Invalid(String),
    /// The request is well formed and the current state refuses it: cancelling
    /// an export that finished, exporting a trail that holds nothing.
    #[error("{0}")]
    Conflict(String),
    /// The object store an audit export is written to. Kept as the port error
    /// rather than flattened into a string, because it already carries whether
    /// the job should come back for it.
    #[error("the audit export store failed: {0}")]
    Storage(#[from] aiwatcher_core::ports::PortError),
    #[error("unsupported or inconsistent IAM document: {0}")]
    Incompatible(String),
    #[error("IAM storage failed: {0}")]
    Backend(String),
}

/// Trusted time. Adapters sample it after acquiring the mutation lock, never
/// accept a request's claimed timestamp as authorization time.
pub trait Clock: Debug + Send + Sync {
    fn now(&self) -> i64;
}

#[derive(Debug)]
pub struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> i64 {
        time::OffsetDateTime::now_utc().unix_timestamp()
    }
}

/// The actor must come from verified authentication, not from a request body.
/// Instance roles and IdP groups intentionally are not arguments to policy.
#[async_trait]
pub trait IamStore: Debug + Send + Sync {
    /// Bootstrap an organization with its first owner atomically. The
    /// API must separately authorize organization creation at instance level.
    async fn create_organization(&self, owner: &Principal, name: &str) -> Result<Organization>;
    /// Organization administrators only. Current membership is checked on every page.
    async fn audit(
        &self,
        organization: OrganizationId,
        actor: &Principal,
        after: i64,
        limit: usize,
    ) -> Result<Vec<AuditEntry>>;
    /// Whether this principal may read this organization's trail, now.
    ///
    /// The same check [`IamStore::audit`] makes, asked without a page — what an
    /// export's own routes need, since the bytes they serve are in an object
    /// store this crate does not authorize. A fresh decision every time it is
    /// asked, never a capability somebody holds afterwards.
    async fn authorize_audit(&self, organization: OrganizationId, actor: &Principal) -> Result<()>;
    /// What the trail currently holds, and where a sweep moved its beginning.
    ///
    /// An export pins its upper bound from `last_sequence`; a reader who finds
    /// the history starting at 4 312 reads `watermark` to learn why.
    async fn audit_bounds(
        &self,
        organization: OrganizationId,
        actor: &Principal,
    ) -> Result<AuditBounds>;
    /// Apply this deployment's audit retention to every organization.
    ///
    /// Takes no actor, and there is no route that reaches it. Retention is the
    /// deployment's clock rather than anybody's request: an administrator who
    /// could delete the record of their own administration is the one capability
    /// an audit trail must not grant, and a caller that could name the cutoff
    /// could name today.
    ///
    /// The delete and the watermark that explains it commit together, so there
    /// is no moment at which entries are gone and nothing says so.
    async fn prune_audit(&self, retention: &AuditRetention, now: i64) -> Result<PruneReport>;
    async fn organizations(&self, actor: &Principal) -> Result<Vec<Organization>>;
    /// Check current authorization and apply the change in one transaction.
    async fn apply(
        &self,
        organization: OrganizationId,
        actor: &Principal,
        command: Command,
    ) -> Result<Change>;
    /// Return only projects with an active direct or team grant. Membership,
    /// including owner/admin membership, confers no implicit project access.
    async fn projects(
        &self,
        organization: OrganizationId,
        actor: &Principal,
    ) -> Result<Vec<ProjectAccess>>;
    /// Who is in the organization, for an owner or admin of it. Never a
    /// decision: a role here is what somebody was given, not what they may do.
    async fn roster(&self, organization: OrganizationId, actor: &Principal) -> Result<Roster>;
    /// Every grant on one project, for whoever may issue one there. Windows
    /// come back as issued; filtering by the clock is `access`'s job, per
    /// person.
    async fn project_grants(&self, scope: ProjectScope, actor: &Principal) -> Result<Vec<Grant>>;
    /// Fresh policy decision; not a durable capability. Streams, jobs and
    /// later writes must recheck rather than treating it as a session grant.
    async fn access(&self, scope: ProjectScope, actor: &Principal) -> Result<ProjectAccess>;

    /// Offer a grant to whoever holds the returned token. The token is minted
    /// here, stored only as a digest, and returned exactly once.
    async fn invite(
        &self,
        scope: ProjectScope,
        actor: &Principal,
        offer: InvitationOffer,
    ) -> Result<IssuedInvitation>;
    /// The offers for projects this caller may administer.
    async fn invitations(
        &self,
        organization: OrganizationId,
        actor: &Principal,
    ) -> Result<Vec<Invitation>>;
    /// Withdraw an offer nobody has taken up. A redeemed one is refused.
    async fn revoke_invitation(
        &self,
        organization: OrganizationId,
        actor: &Principal,
        invitation: InvitationId,
    ) -> Result<()>;
    /// What this token offers, without spending it.
    ///
    /// The one read here whose caller may hold no identity at all: somebody
    /// with a link and no account has nothing else to ask with, and deciding
    /// whether to make one is a decision they are owed the terms of. It names
    /// no organization, because whoever holds a token does not know which one
    /// it belongs to and a parameter that made them say would confirm a guess —
    /// the same rule [`Self::redeem`] keeps.
    async fn offered(&self, token: &str) -> Result<Offered>;
    /// Membership and a grant for whoever presented this token, in one
    /// transaction, once. The redeemer's pair comes from a verified session and
    /// is learned here; the offer never named it.
    async fn redeem(&self, token: &str, redeemer: &Principal) -> Result<Redeemed>;
}
