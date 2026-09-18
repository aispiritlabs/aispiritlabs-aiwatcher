//! Whether the named organization and project exist, asked of IAM itself.
//!
//! Mapping data into a project is not granting anybody access to it, and this
//! module is where both halves of that are kept true. It **asks** IAM whether
//! the target is real and whether the operator running the migration may write
//! to it; it never creates an organization, a project, a team, a membership or
//! a grant, and it reads no identity-provider group. A principal here is the
//! exact `(provider, subject)` pair the control plane compares, never an email
//! address and never a group name.
//!
//! One answer is deliberately coarse. `IamStore::access` returns the same
//! `NotFound` for "there is no such project" and "you hold no grant on it",
//! because telling them apart would let anybody enumerate an organization's
//! projects. So the refusal here says both, and an operator reads it as either.

use std::sync::Arc;

use aiwatcher_iam::{IamStore, Principal, ProjectRole, ProjectScope};
use async_trait::async_trait;

use crate::manifest::IamCheck;
use crate::{MigrationError, Result};

/// Who says the target project is real.
#[async_trait]
pub trait TargetAuthority: Send + Sync + std::fmt::Debug {
    /// # Errors
    ///
    /// [`MigrationError::Authority`] when the scope is not one this authority
    /// admits for this operator, or when the store cannot be reached.
    async fn admit(&self, scope: ProjectScope) -> Result<IamCheck>;
}

/// The control plane, asked as one operator.
#[derive(Debug)]
pub struct IamAuthority {
    store: Arc<dyn IamStore>,
    principal: Principal,
    label: String,
    authoritative: bool,
    required: ProjectRole,
}

impl IamAuthority {
    /// The deployment's own IAM store.
    ///
    /// `authoritative` is what separates a cutover from a rehearsal: the
    /// server refuses to run `oidc` against anything but PostgreSQL, so a
    /// migration admitted by a memory fixture is admitted by something no
    /// deployment authorizes from — which is a fine thing to test against and
    /// never a thing to cut over on. It rides into the receipt either way.
    #[must_use]
    pub fn new(
        store: Arc<dyn IamStore>,
        principal: Principal,
        label: impl Into<String>,
        authoritative: bool,
    ) -> Self {
        Self {
            store,
            principal,
            label: label.into(),
            authoritative,
            // Admin, because the operator is writing every authored artifact a
            // project will hold. An editor grant is what a producer's token
            // gets by construction, and a token in an agent's environment must
            // not be enough to land somebody's whole registry somewhere.
            required: ProjectRole::Admin,
        }
    }
}

#[async_trait]
impl TargetAuthority for IamAuthority {
    async fn admit(&self, scope: ProjectScope) -> Result<IamCheck> {
        let access =
            self.store
                .access(scope, &self.principal)
                .await
                .map_err(|error| match error {
                    aiwatcher_iam::Error::NotFound => MigrationError::Authority(format!(
                        "organization {} project {} is not one this principal may write to: either \
                     it does not exist or {}:{} holds no live grant on it",
                        scope.organization.0,
                        scope.project.0,
                        self.principal.provider,
                        self.principal.subject
                    )),
                    other => MigrationError::Authority(other.to_string()),
                })?;
        access.require(self.required).map_err(|_| {
            MigrationError::Authority(format!(
                "{}:{} holds {:?} on organization {} project {}; a migration needs {:?}",
                self.principal.provider,
                self.principal.subject,
                access.role,
                scope.organization.0,
                scope.project.0,
                self.required
            ))
        })?;
        Ok(IamCheck::Verified {
            authority: self.label.clone(),
            authoritative: self.authoritative,
            provider: self.principal.provider.clone(),
            subject: self.principal.subject.clone(),
            role: access.role,
            evaluated_at: access.evaluated_at,
        })
    }
}

/// No IAM reachable. A dry run may say so; an execution may not.
#[derive(Debug)]
pub struct Offline {
    reason: String,
}

impl Offline {
    #[must_use]
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl Default for Offline {
    fn default() -> Self {
        Self::new(
            "this run reached no IAM store, so nothing has confirmed that the target \
             organization and project exist",
        )
    }
}

#[async_trait]
impl TargetAuthority for Offline {
    async fn admit(&self, _scope: ProjectScope) -> Result<IamCheck> {
        Ok(IamCheck::NotChecked {
            reason: self.reason.clone(),
        })
    }
}

/// A file saying which scope it admits, for rehearsals and tests.
///
/// It is not an IAM store and never claims to be one: it answers
/// `authoritative: false`, which puts a [`crate::manifest::BlockerKind::
/// NonAuthoritativeIam`] on every receipt it admits. A run authorized this way
/// copies bytes faithfully and is not a cutover, and the receipt says so for
/// as long as it is kept. Production uses `--iam postgres:`.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct Fixture {
    /// Exactly the scopes and principals this file admits.
    pub admits: Vec<Admission>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct Admission {
    pub organization: String,
    pub project: String,
    pub provider: String,
    pub subject: String,
    pub role: ProjectRole,
}

/// A fixture, read as one principal.
#[derive(Debug)]
pub struct FixtureAuthority {
    fixture: Fixture,
    principal: Principal,
    evaluated_at: i64,
}

impl FixtureAuthority {
    #[must_use]
    pub fn new(fixture: Fixture, principal: Principal, evaluated_at: i64) -> Self {
        Self {
            fixture,
            principal,
            evaluated_at,
        }
    }
}

#[async_trait]
impl TargetAuthority for FixtureAuthority {
    async fn admit(&self, scope: ProjectScope) -> Result<IamCheck> {
        let admitted = self.fixture.admits.iter().find(|admission| {
            admission.organization == scope.organization.0.to_string()
                && admission.project == scope.project.0.to_string()
                && admission.provider == self.principal.provider
                && admission.subject == self.principal.subject
        });
        let Some(admitted) = admitted else {
            return Err(MigrationError::Authority(format!(
                "the fixture admits no principal {}:{} on organization {} project {}",
                self.principal.provider,
                self.principal.subject,
                scope.organization.0,
                scope.project.0
            )));
        };
        if admitted.role != ProjectRole::Admin {
            return Err(MigrationError::Authority(format!(
                "the fixture admits {:?}; a migration needs {:?}",
                admitted.role,
                ProjectRole::Admin
            )));
        }
        Ok(IamCheck::Verified {
            authority: "fixture".to_owned(),
            authoritative: false,
            provider: self.principal.provider.clone(),
            subject: self.principal.subject.clone(),
            role: admitted.role,
            evaluated_at: self.evaluated_at,
        })
    }
}
