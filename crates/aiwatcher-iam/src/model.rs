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
id!(InvitationId);

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

// `Ord` and `Hash` because a scope is a key: a fold groups rows by it, a
// dispatcher keeps one entry per project, and both want a total order so two
// runs of one loop read the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamProjectScope))]
pub struct ProjectScope {
    pub organization: OrganizationId,
    pub project: ProjectId,
}

impl ProjectScope {
    /// The one text form: `<organization-uuid>/<project-uuid>`.
    ///
    /// Two uuids, so building it by concatenation is safe — neither half can
    /// hold the separator, and no pair of scopes produces one string. The
    /// spelling `aiwatcher_execution::ExecutionScope::key` and
    /// `aiwatcher_core::ProjectScope::key` write, which is what lets a scope
    /// configured as text, stamped onto an event and folded into a row be
    /// recognised as the same scope in all three.
    #[must_use]
    pub fn key(&self) -> String {
        format!("{}/{}", self.organization.0, self.project.0)
    }

    /// The same project as the event log carries it: two uuids and nothing
    /// else.
    ///
    /// `aiwatcher-core` takes no dependency on a store and this crate is one,
    /// so the log's form cannot be this type. This is the **one** conversion
    /// between them — every caller that stamps a scope onto an envelope, folds
    /// a row or resolves a route goes through it, rather than each spelling out
    /// `.0` twice and one of them one day spelling it the other way round.
    #[must_use]
    pub const fn on_the_log(self) -> aiwatcher_core::ProjectScope {
        aiwatcher_core::ProjectScope::new(self.organization.0, self.project.0)
    }

    /// Read [`Self::key`] back.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] naming the text, when it is not two uuids separated
    /// by a slash.
    pub fn parse(text: &str) -> Result<Self> {
        let invalid = || {
            Error::Invalid(format!(
                "{text:?} is not a project scope; write it as \
                 <organization-uuid>/<project-uuid>"
            ))
        };
        let (organization, project) = text.trim().split_once('/').ok_or_else(invalid)?;
        Ok(Self {
            organization: OrganizationId(
                Uuid::parse_str(organization.trim()).map_err(|_| invalid())?,
            ),
            project: ProjectId(Uuid::parse_str(project.trim()).map_err(|_| invalid())?),
        })
    }
}

impl std::fmt::Display for ProjectScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.key())
    }
}

impl std::str::FromStr for ProjectScope {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        Self::parse(text)
    }
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

/// An offer of a grant to whoever holds its token, redeemable once.
///
/// This is what lets somebody be given access **before** they have ever signed
/// in, which a grant cannot do: a grant names a `(provider, subject)` pair and
/// nobody knows a stranger's subject until their provider has minted one. So
/// the offer is made to a secret instead, and the pair is learned at the moment
/// it is redeemed.
///
/// The token itself is not here and is never stored: only a SHA-256 of it is,
/// and the plaintext exists once, in the response that created it. A `label`
/// is a delivery hint — an email address, a name on a list — and **never an
/// identity key**: whoever holds the token redeems it, and checking the label
/// would be authentication by an unverified string.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamInvitation))]
pub struct Invitation {
    pub id: InvitationId,
    pub scope: ProjectScope,
    /// What redeeming it grants — the same three roles a grant carries.
    pub role: ProjectRole,
    /// The window the resulting grant gets, declared when the offer was made.
    pub window: GrantWindow,
    /// When the *offer* stops being redeemable, which is not the window's end.
    pub expires_at: i64,
    pub created_by: Principal,
    pub created_at: i64,
    /// A note about who it was sent to. Never compared against anybody.
    pub label: Option<String>,
    pub redeemed: Option<Redemption>,
}

/// What an invitation offers: everything its author declares, as one value.
///
/// One type rather than four parameters, and the same shape the HTTP body has —
/// so a field added here is added once and refused everywhere it is not
/// understood, rather than threaded through five signatures.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamInvitationOffer))]
pub struct InvitationOffer {
    pub role: ProjectRole,
    /// The window the resulting grant gets.
    pub window: GrantWindow,
    /// When the offer stops being redeemable, which is not the window's end.
    pub expires_at: i64,
    /// A note about who it was sent to. Never compared against anybody.
    #[serde(default)]
    pub label: Option<String>,
}

/// Who turned an offer into a grant, and which grant it became.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamRedemption))]
pub struct Redemption {
    pub principal: Principal,
    pub at: i64,
    pub grant: GrantId,
}

/// The one moment the token exists in the clear.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamIssuedInvitation))]
pub struct IssuedInvitation {
    pub invitation: Invitation,
    /// Deliver this and forget it. Nothing can show it again.
    pub token: String,
}

/// What a redeemer learns: where they now are, and what they got.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamRedeemed))]
pub struct Redeemed {
    pub organization: Organization,
    pub project: Project,
    pub role: ProjectRole,
    pub window: GrantWindow,
    pub grant: GrantId,
}

/// One person's standing in the organization, which is not access to anything.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamMembership))]
pub struct Membership {
    pub principal: Principal,
    pub role: OrganizationRole,
}

/// A team and the people a grant to it reaches.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamTeamMembers))]
pub struct TeamMembers {
    pub team: Team,
    pub members: Vec<Principal>,
}

/// What an administrator has to see to administer: who is in the organization,
/// which teams exist, and every project in it.
///
/// Deliberately not [`ProjectAccess`]: that answers about the caller, and an
/// organization administrator may grant on a project they hold no grant on and
/// therefore cannot list any other way. It carries no decision — a role here is
/// what somebody was given, never what they may do now, which only
/// [`IamStore::access`] answers and only for one person at a time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamRoster))]
pub struct Roster {
    pub organization: Organization,
    pub members: Vec<Membership>,
    pub teams: Vec<TeamMembers>,
    pub projects: Vec<Project>,
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
    /// An offer, never its token: the record here carries only the hash, and
    /// the hash is not in [`Invitation`].
    InvitationCreated {
        invitation: Box<Invitation>,
    },
    InvitationRevoked {
        invitation: InvitationId,
    },
    /// The one entry written by somebody who was not yet a member.
    InvitationRedeemed {
        invitation: InvitationId,
        grant: GrantId,
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
