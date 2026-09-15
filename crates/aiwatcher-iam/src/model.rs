use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! id {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
        #[serde(transparent)]
        pub struct $name(pub Uuid);
        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }
        }
        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
    };
}
id!(OrganizationId);
id!(ProjectId);
id!(TeamId);
id!(GrantId);

/// A stable provider namespace and its subject, compared exactly. Email and
/// display names are deliberately absent; neither is an identity key.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamPrincipal))]
pub struct Principal {
    pub provider: String,
    pub subject: String,
}

impl Principal {
    pub fn new(provider: impl Into<String>, subject: impl Into<String>) -> Result<Self> {
        let value = Self {
            provider: provider.into(),
            subject: subject.into(),
        };
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn validate(&self) -> Result<()> {
        for (label, value) in [("provider", &self.provider), ("subject", &self.subject)] {
            if value.is_empty() || value.len() > 1024 || value.chars().any(char::is_control) {
                return Err(Error::Invalid(format!(
                    "{label} must contain 1–1024 bytes without control characters"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamOrganizationRole))]
pub enum OrganizationRole {
    Member,
    Admin,
    Owner,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamProjectRole))]
pub enum ProjectRole {
    Viewer,
    Editor,
    Admin,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamOrganization))]
pub struct Organization {
    pub id: OrganizationId,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamProjectScope))]
pub struct ProjectScope {
    pub organization: OrganizationId,
    pub project: ProjectId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamProject))]
pub struct Project {
    pub scope: ProjectScope,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamTeam))]
pub struct Team {
    pub id: TeamId,
    pub organization: OrganizationId,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamGrantee))]
pub enum Grantee {
    User(Principal),
    Team(TeamId),
}

/// Half-open intervals in Unix seconds: start inclusive, end exclusive.
/// After edit_until a still-readable grant falls back to Viewer. No read_until
/// means indefinite read access, even when the edit period has ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamGrantWindow))]
pub struct GrantWindow {
    pub valid_from: i64,
    pub edit_until: Option<i64>,
    pub read_until: Option<i64>,
}

impl GrantWindow {
    #[must_use]
    pub const fn permanent(valid_from: i64) -> Self {
        Self {
            valid_from,
            edit_until: None,
            read_until: None,
        }
    }
    pub(crate) fn validate(self) -> Result<()> {
        if self.edit_until.is_some_and(|end| end <= self.valid_from)
            || self.read_until.is_some_and(|end| end <= self.valid_from)
            || matches!((self.edit_until, self.read_until), (Some(edit), Some(read)) if read < edit)
        {
            return Err(Error::Invalid(
                "grant end must follow its start; read access cannot end before edit access".into(),
            ));
        }
        Ok(())
    }
    pub(crate) fn role_at(self, role: ProjectRole, now: i64) -> Option<ProjectRole> {
        if now < self.valid_from || self.read_until.is_some_and(|end| now >= end) {
            return None;
        }
        Some(if self.edit_until.is_some_and(|end| now >= end) {
            ProjectRole::Viewer
        } else {
            role
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamGrant))]
pub struct Grant {
    pub id: GrantId,
    pub scope: ProjectScope,
    pub grantee: Grantee,
    pub role: ProjectRole,
    pub window: GrantWindow,
}

/// Each independently active source remains visible. Expiring a workshop
/// grant must not conceal a permanent source that still grants access.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamEffectiveGrant))]
pub struct EffectiveGrant {
    pub grant: Grant,
    pub role: ProjectRole,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamProjectAccess))]
pub struct ProjectAccess {
    pub project: Project,
    pub role: ProjectRole,
    pub grants: Vec<EffectiveGrant>,
    pub evaluated_at: i64,
}

impl ProjectAccess {
    pub fn require(&self, role: ProjectRole) -> Result<()> {
        if self.role >= role {
            Ok(())
        } else {
            Err(Error::Forbidden)
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamCommand))]
pub enum Command {
    SetMember {
        principal: Principal,
        role: OrganizationRole,
    },
    RemoveMember {
        principal: Principal,
    },
    CreateTeam {
        name: String,
    },
    DeleteTeam {
        team: TeamId,
    },
    SetTeamMember {
        team: TeamId,
        principal: Principal,
        present: bool,
    },
    CreateProject {
        name: String,
    },
    Grant {
        project: ProjectId,
        grantee: Grantee,
        role: ProjectRole,
        window: GrantWindow,
    },
    RevokeGrant {
        project: ProjectId,
        grant: GrantId,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamChange))]
pub enum Change {
    Applied,
    TeamCreated(Team),
    ProjectCreated(Project),
    GrantCreated(Grant),
}

pub(crate) fn name(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 200 || value.chars().any(char::is_control) {
        return Err(Error::Invalid(
            "name must contain 1–200 bytes without control characters".into(),
        ));
    }
    Ok(value.to_owned())
}

/// Successful administrative mutations only; committed with the state change.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AuditEntry {
    /// Monotonic within this organization; pagination resumes after this value.
    pub sequence: i64,
    pub organization: OrganizationId,
    pub actor: Principal,
    pub occurred_at: i64,
    pub action: AuditAction,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuditAction {
    OrganizationCreated {
        organization: Organization,
    },
    CommandApplied {
        command: Box<Command>,
        change: Change,
    },
}

pub(crate) fn audit_limit(after: i64, limit: usize) -> Result<()> {
    if after < 0 || !(1..=100).contains(&limit) {
        return Err(Error::Invalid(
            "audit requires after >= 0 and limit in 1..=100".into(),
        ));
    }
    Ok(())
}
