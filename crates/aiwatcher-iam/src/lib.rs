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
use std::fmt::Debug;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the requested IAM resource is not available to this principal")]
    NotFound,
    #[error("the operation needs a higher IAM role")]
    Forbidden,
    #[error("an organization must retain at least one owner")]
    LastOwner,
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
    /// Fresh policy decision; not a durable capability. Streams, jobs and
    /// later writes must recheck rather than treating it as a session grant.
    async fn access(&self, scope: ProjectScope, actor: &Principal) -> Result<ProjectAccess>;
}
