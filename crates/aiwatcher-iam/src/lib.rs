//! IAM-01's control plane. These stores authorize their own metadata operations.
//!
//! They do not yet isolate the application's datasets, events, artifacts or
//! workers. Do not activate a multi-organization UI until every data plane
//! adapter enforces the returned project scope. See this crate's README.

pub mod memory;
mod model;
mod policy;
#[cfg(feature = "postgres")]
pub mod postgres;

use async_trait::async_trait;
pub use model::*;
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
    /// Membership and a grant for whoever presented this token, in one
    /// transaction, once. The redeemer's pair comes from a verified session and
    /// is learned here; the offer never named it.
    async fn redeem(&self, token: &str, redeemer: &Principal) -> Result<Redeemed>;
}
