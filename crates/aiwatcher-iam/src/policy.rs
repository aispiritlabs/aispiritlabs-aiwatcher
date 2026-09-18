//! One organization's authoritative membership/grant aggregate. Both adapters
//! run this policy while holding their write lock, so a concurrent demotion
//! cannot be checked against a stale snapshot and committed afterward.

use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const SCHEMA_VERSION: u32 = 1;
const MAX_DOCUMENT_BYTES: usize = 4 * 1024 * 1024;
/// How long a spent or lapsed invitation stays in the aggregate.
///
/// An invitation is an operational record with a short life: once it is
/// redeemed the grant is the thing that matters, and once it has expired
/// unredeemed there is nothing to do with it. Its lasting trace is the audit
/// entry, which is not in this document and is not pruned. Without this, a
/// school year of workshops would fill a 4 MiB aggregate with offers nobody
/// can act on.
const INVITATION_RETENTION_SECONDS: i64 = 30 * 24 * 60 * 60;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {
    principal: Principal,
    role: OrganizationRole,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamRecord {
    team: Team,
    members: Vec<Principal>,
}

/// The stored form of an offer: the record, plus the digest of the one secret
/// that redeems it. The digest never leaves this module, and [`Invitation`] —
/// which does leave — has no field for it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InvitationRecord {
    invitation: Invitation,
    token_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OrganizationState {
    schema_version: u32,
    pub organization: Organization,
    members: Vec<Member>,
    teams: Vec<TeamRecord>,
    projects: Vec<Project>,
    grants: Vec<Grant>,
    // Additive, and read as absent from every document written before
    // invitations existed. The other direction is refused rather than
    // truncated: `deny_unknown_fields` means an older binary reading a newer
    // document fails closed instead of dropping the offers it cannot see.
    #[serde(default)]
    invitations: Vec<InvitationRecord>,
}

impl OrganizationState {
    pub fn authorize_audit(&self, actor: &Principal) -> Result<()> {
        if self.member_role(actor)? < OrganizationRole::Admin {
            return Err(Error::Forbidden);
        }
        Ok(())
    }

    pub fn new(owner: &Principal, label: &str) -> Result<Self> {
        owner.validate()?;
        Ok(Self {
            schema_version: SCHEMA_VERSION,
            organization: Organization {
                id: OrganizationId::new(),
                name: model::name(label)?,
            },
            members: vec![Member {
                principal: owner.clone(),
                role: OrganizationRole::Owner,
            }],
            teams: vec![],
            projects: vec![],
            grants: vec![],
            invitations: vec![],
        })
    }

    /// Validate stored records as well as requests. Unknown versions and broken
    /// cross-references must not turn into a more permissive partial read.
    #[cfg(feature = "postgres")]
    pub fn decode(id: OrganizationId, value: serde_json::Value) -> Result<Self> {
        let state: Self = serde_json::from_value(value)
            .map_err(|error| Error::Incompatible(error.to_string()))?;
        if state.organization.id != id {
            return Err(Error::Incompatible(
                "organization row and document IDs differ".into(),
            ));
        }
        state
            .validate()
            .map_err(|error| Error::Incompatible(error.to_string()))?;
        Ok(state)
    }

    pub fn encode(&self) -> Result<serde_json::Value> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|error| Error::Backend(error.to_string()))?;
        if bytes.len() > MAX_DOCUMENT_BYTES {
            return Err(Error::Invalid(
                "organization IAM metadata exceeds 4 MiB".into(),
            ));
        }
        serde_json::from_slice(&bytes).map_err(|error| Error::Backend(error.to_string()))
    }

    fn validate(&self) -> Result<()> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(Error::Incompatible("unknown schema version".into()));
        }
        model::name(&self.organization.name)?;
        let members: BTreeSet<_> = self
            .members
            .iter()
            .map(|member| &member.principal)
            .collect();
        if members.len() != self.members.len()
            || !self
                .members
                .iter()
                .any(|member| member.role == OrganizationRole::Owner)
        {
            return Err(Error::Incompatible(
                "duplicate memberships or missing owner".into(),
            ));
        }
        for member in &members {
            member.validate()?;
        }
        let projects: BTreeSet<_> = self
            .projects
            .iter()
            .map(|project| project.scope.project)
            .collect();
        let teams: BTreeSet<_> = self.teams.iter().map(|record| record.team.id).collect();
        let grants: BTreeSet<_> = self.grants.iter().map(|grant| grant.id).collect();
        if projects.len() != self.projects.len()
            || teams.len() != self.teams.len()
            || grants.len() != self.grants.len()
        {
            return Err(Error::Incompatible(
                "duplicate project, team or grant IDs".into(),
            ));
        }
        for project in &self.projects {
            model::name(&project.name)?;
            if project.scope.organization != self.organization.id {
                return Err(Error::Incompatible(
                    "project in another organization".into(),
                ));
            }
        }
        for record in &self.teams {
            model::name(&record.team.name)?;
            if record.team.organization != self.organization.id
                || record
                    .members
                    .iter()
                    .any(|member| !members.contains(member))
                || record.members.iter().collect::<BTreeSet<_>>().len() != record.members.len()
            {
                return Err(Error::Incompatible(
                    "invalid team membership or organization".into(),
                ));
            }
        }
        let offers: BTreeSet<_> = self
            .invitations
            .iter()
            .map(|record| record.invitation.id)
            .collect();
        if offers.len() != self.invitations.len() {
            return Err(Error::Incompatible("duplicate invitation IDs".into()));
        }
        for record in &self.invitations {
            record.invitation.window.validate()?;
            record.invitation.created_by.validate()?;
            // A redeemed offer names a member and a grant that exist; an offer
            // for a project this organization does not have could grant into
            // somebody else's scope the moment somebody took it up.
            if record.invitation.scope.organization != self.organization.id
                || !projects.contains(&record.invitation.scope.project)
                || record.token_sha256.len() != 64
                || record
                    .invitation
                    .redeemed
                    .as_ref()
                    .is_some_and(|redemption| !members.contains(&redemption.principal))
            {
                return Err(Error::Incompatible(
                    "invitation outside this organization or naming nothing".into(),
                ));
            }
        }
        for grant in &self.grants {
            grant.window.validate()?;
            if grant.scope.organization != self.organization.id
                || !projects.contains(&grant.scope.project)
                || match &grant.grantee {
                    Grantee::User(user) => !members.contains(user),
                    Grantee::Team(team) => !teams.contains(team),
                }
            {
                return Err(Error::Incompatible(
                    "grant crosses an organization or has a missing reference".into(),
                ));
            }
        }
        Ok(())
    }

    /// Offer an unknown person a grant, by giving a secret the right to become
    /// one.
    ///
    /// The authority is the project's: whoever may issue a grant here may issue
    /// an offer of one, because that is what it becomes. The digest arrives
    /// already computed — this module never sees the token, and the only thing
    /// that can be leaked from a stolen document is a hash.
    pub fn invite(
        &mut self,
        actor: &Principal,
        project: ProjectId,
        offer: InvitationOffer,
        token_sha256: String,
        now: i64,
    ) -> Result<Invitation> {
        let scope = self.manage_project(actor, project, now)?;
        offer.window.validate()?;
        if offer.expires_at <= now {
            return Err(Error::Invalid(
                "an invitation must expire in the future".into(),
            ));
        }
        if token_sha256.len() != 64 || !token_sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(Error::Invalid("the token digest is not a SHA-256".into()));
        }
        let label = offer.label.map(|value| model::name(&value)).transpose()?;
        self.forget_spent_invitations(now);
        let invitation = Invitation {
            id: InvitationId::new(),
            scope,
            role: offer.role,
            window: offer.window,
            expires_at: offer.expires_at,
            created_by: actor.clone(),
            created_at: now,
            label,
            redeemed: None,
        };
        self.invitations.push(InvitationRecord {
            invitation: invitation.clone(),
            token_sha256,
        });
        Ok(invitation)
    }

    /// The offers for projects this caller may administer, newest first.
    ///
    /// Not every offer in the organization: a project admin sees their own
    /// project's, which is the same line `project_grants` draws.
    pub fn invitations(&self, actor: &Principal, now: i64) -> Result<Vec<Invitation>> {
        self.member_role(actor)?;
        let mut offers: Vec<_> = self
            .invitations
            .iter()
            .filter(|record| {
                self.manage_project(actor, record.invitation.scope.project, now)
                    .is_ok()
            })
            .map(|record| record.invitation.clone())
            .collect();
        offers.sort_by_key(|offer| std::cmp::Reverse(offer.created_at));
        Ok(offers)
    }

    /// Withdraw an offer nobody has taken up.
    ///
    /// A redeemed one is refused rather than deleted, because deleting it would
    /// read as undoing it: what it produced is a grant, and revoking that is
    /// [`Command::RevokeGrant`]'s job.
    pub fn revoke_invitation(
        &mut self,
        actor: &Principal,
        invitation: InvitationId,
        now: i64,
    ) -> Result<()> {
        let record = self
            .invitations
            .iter()
            .find(|record| record.invitation.id == invitation)
            .ok_or(Error::NotFound)?;
        self.manage_project(actor, record.invitation.scope.project, now)?;
        if record.invitation.redeemed.is_some() {
            return Err(Error::Redeemed);
        }
        self.invitations
            .retain(|record| record.invitation.id != invitation);
        Ok(())
    }

    /// Whether this organization holds the offer that digest opens.
    ///
    /// Asked before a redemption takes a write lock, so a token for somebody
    /// else's organization is not a lock on this one.
    pub fn holds_invitation(&self, token_sha256: &str) -> bool {
        self.invitations
            .iter()
            .any(|record| record.token_sha256 == token_sha256)
    }

    /// Turn an offer into membership and a grant, for whoever presented it.
    ///
    /// The redeemer is a stranger here until this moment: they are made an
    /// ordinary member if they are not one already, and the grant is exactly
    /// what the offer declared. Their pair is learned now, from a verified
    /// session — the offer never named it and the `label` on it is not
    /// consulted, because an unverified string is not an identity.
    pub fn redeem(
        &mut self,
        token_sha256: &str,
        redeemer: &Principal,
        now: i64,
    ) -> Result<Redeemed> {
        redeemer.validate()?;
        let record = self
            .invitations
            .iter()
            .find(|record| record.token_sha256 == token_sha256)
            .ok_or(Error::NotFound)?;
        if record.invitation.redeemed.is_some() {
            return Err(Error::Redeemed);
        }
        if now >= record.invitation.expires_at {
            return Err(Error::Expired);
        }
        let invitation = record.invitation.clone();
        let project = self
            .projects
            .iter()
            .find(|project| project.scope == invitation.scope)
            .ok_or(Error::NotFound)?
            .clone();
        if self.member_role(redeemer).is_err() {
            self.members.push(Member {
                principal: redeemer.clone(),
                role: OrganizationRole::Member,
            });
        }
        let grant = Grant {
            id: GrantId::new(),
            scope: invitation.scope,
            grantee: Grantee::User(redeemer.clone()),
            role: invitation.role,
            window: invitation.window,
        };
        self.grants.push(grant.clone());
        if let Some(stored) = self
            .invitations
            .iter_mut()
            .find(|record| record.invitation.id == invitation.id)
        {
            stored.invitation.redeemed = Some(Redemption {
                principal: redeemer.clone(),
                at: now,
                grant: grant.id,
            });
        }
        Ok(Redeemed {
            organization: self.organization.clone(),
            project,
            role: grant.role,
            window: grant.window,
            grant: grant.id,
        })
    }

    /// Which offer a redemption produced, for the audit entry beside it.
    pub fn invitation_of(&self, grant: GrantId) -> Option<InvitationId> {
        self.invitations
            .iter()
            .find(|record| {
                record
                    .invitation
                    .redeemed
                    .as_ref()
                    .is_some_and(|redemption| redemption.grant == grant)
            })
            .map(|record| record.invitation.id)
    }

    /// Drop offers that can no longer do anything, a month after they stopped.
    fn forget_spent_invitations(&mut self, now: i64) {
        self.invitations.retain(|record| {
            let done = record
                .invitation
                .redeemed
                .as_ref()
                .map_or(record.invitation.expires_at, |redemption| redemption.at);
            now - done < INVITATION_RETENTION_SECONDS
        });
    }

    pub fn member_role(&self, actor: &Principal) -> Result<OrganizationRole> {
        actor.validate()?;
        self.members
            .iter()
            .find(|member| member.principal == *actor)
            .map(|member| member.role)
            .ok_or(Error::NotFound)
    }

    /// Who is here, for somebody who administers it.
    ///
    /// The same authority as the audit, and for the same reason: both say what
    /// an organization holds rather than what the reader may reach, and a
    /// member who may reach one project has no business reading the rest of the
    /// membership.
    pub fn roster(&self, actor: &Principal) -> Result<Roster> {
        if self.member_role(actor)? < OrganizationRole::Admin {
            return Err(Error::Forbidden);
        }
        Ok(Roster {
            organization: self.organization.clone(),
            members: self
                .members
                .iter()
                .map(|member| Membership {
                    principal: member.principal.clone(),
                    role: member.role,
                })
                .collect(),
            teams: self
                .teams
                .iter()
                .map(|record| TeamMembers {
                    team: record.team.clone(),
                    members: record.members.clone(),
                })
                .collect(),
            projects: self.projects.clone(),
        })
    }

    /// Every grant on one project, live or not, for whoever may issue one.
    ///
    /// Read by exactly the authority that may write here — an organization
    /// admin or this project's own admin — because "who else may reach this"
    /// and "who may I add" are one question, and the second is already this
    /// authority's.
    ///
    /// The windows come back as they were issued; nothing here is filtered by
    /// the clock. What a grant does *now* is [`Self::access`]'s answer, taken
    /// per person, and a list that quietly dropped a lapsed row would hide the
    /// thing an administrator opened it to see.
    pub fn project_grants(
        &self,
        scope: ProjectScope,
        actor: &Principal,
        now: i64,
    ) -> Result<Vec<Grant>> {
        if scope.organization != self.organization.id {
            return Err(Error::NotFound);
        }
        let scope = self.manage_project(actor, scope.project, now)?;
        Ok(self
            .grants
            .iter()
            .filter(|grant| grant.scope == scope)
            .cloned()
            .collect())
    }

    pub fn projects(&self, actor: &Principal, now: i64) -> Result<Vec<ProjectAccess>> {
        self.member_role(actor)?;
        self.projects
            .iter()
            .filter_map(|project| match self.access(project.scope, actor, now) {
                Ok(access) => Some(Ok(access)),
                Err(Error::NotFound) => None,
                Err(error) => Some(Err(error)),
            })
            .collect()
    }

    pub fn access(
        &self,
        scope: ProjectScope,
        actor: &Principal,
        now: i64,
    ) -> Result<ProjectAccess> {
        self.member_role(actor)?;
        let project = self
            .projects
            .iter()
            .find(|project| project.scope == scope)
            .ok_or(Error::NotFound)?;
        let actor_teams: BTreeSet<_> = self
            .teams
            .iter()
            .filter(|record| record.members.contains(actor))
            .map(|record| record.team.id)
            .collect();
        let grants: Vec<_> = self
            .grants
            .iter()
            .filter(|grant| {
                grant.scope == scope
                    && match &grant.grantee {
                        Grantee::User(user) => user == actor,
                        Grantee::Team(team) => actor_teams.contains(team),
                    }
            })
            .filter_map(|grant| {
                grant
                    .window
                    .role_at(grant.role, now)
                    .map(|role| EffectiveGrant {
                        grant: grant.clone(),
                        role,
                    })
            })
            .collect();
        let role = grants
            .iter()
            .map(|grant| grant.role)
            .max()
            .ok_or(Error::NotFound)?;
        Ok(ProjectAccess {
            project: project.clone(),
            role,
            grants,
            evaluated_at: now,
        })
    }

    fn manage_project(
        &self,
        actor: &Principal,
        project: ProjectId,
        now: i64,
    ) -> Result<ProjectScope> {
        let scope = ProjectScope {
            organization: self.organization.id,
            project,
        };
        if !self.projects.iter().any(|entry| entry.scope == scope) {
            return Err(Error::NotFound);
        }
        if self.member_role(actor)? < OrganizationRole::Admin {
            self.access(scope, actor, now)?
                .require(ProjectRole::Admin)?;
        }
        Ok(scope)
    }

    fn keep_owner(&self, user: &Principal, next: Option<OrganizationRole>) -> Result<()> {
        if self.member_role(user)? == OrganizationRole::Owner
            && next != Some(OrganizationRole::Owner)
            && self
                .members
                .iter()
                .filter(|member| member.role == OrganizationRole::Owner)
                .count()
                == 1
        {
            return Err(Error::LastOwner);
        }
        Ok(())
    }

    pub fn apply(&mut self, actor: &Principal, command: Command, now: i64) -> Result<Change> {
        let role = self.member_role(actor)?;
        // Project grant administration can also be held directly by a project
        // admin. Every organization/team operation requires an organization admin.
        if !matches!(command, Command::Grant { .. } | Command::RevokeGrant { .. })
            && role < OrganizationRole::Admin
        {
            return Err(Error::Forbidden);
        }
        match command {
            Command::SetMember {
                principal,
                role: next,
            } => {
                principal.validate()?;
                let previous = self
                    .members
                    .iter()
                    .find(|member| member.principal == principal)
                    .map(|member| member.role);
                // Admins manage ordinary members. Only an owner changes an admin
                // or owner, including their own role; there is no self-promotion.
                if role != OrganizationRole::Owner
                    && (next != OrganizationRole::Member
                        || previous.is_some_and(|role| role != OrganizationRole::Member))
                {
                    return Err(Error::Forbidden);
                }
                if previous.is_some() {
                    self.keep_owner(&principal, Some(next))?;
                }
                if let Some(member) = self
                    .members
                    .iter_mut()
                    .find(|member| member.principal == principal)
                {
                    member.role = next;
                } else {
                    self.members.push(Member {
                        principal,
                        role: next,
                    });
                }
            }
            Command::RemoveMember { principal } => {
                let previous = self.member_role(&principal)?;
                if role != OrganizationRole::Owner && previous != OrganizationRole::Member {
                    return Err(Error::Forbidden);
                }
                self.keep_owner(&principal, None)?;
                self.members.retain(|member| member.principal != principal);
                for team in &mut self.teams {
                    team.members.retain(|member| *member != principal);
                }
                self.grants
                    .retain(|grant| grant.grantee != Grantee::User(principal.clone()));
            }
            Command::CreateTeam { name } => {
                let team = Team {
                    id: TeamId::new(),
                    organization: self.organization.id,
                    name: model::name(&name)?,
                };
                self.teams.push(TeamRecord {
                    team: team.clone(),
                    members: vec![],
                });
                return Ok(Change::TeamCreated(team));
            }
            Command::DeleteTeam { team } => {
                if !self.teams.iter().any(|entry| entry.team.id == team) {
                    return Err(Error::NotFound);
                }
                self.teams.retain(|entry| entry.team.id != team);
                self.grants
                    .retain(|grant| grant.grantee != Grantee::Team(team));
            }
            Command::SetTeamMember {
                team,
                principal,
                present,
            } => {
                self.member_role(&principal)?;
                let record = self
                    .teams
                    .iter_mut()
                    .find(|entry| entry.team.id == team)
                    .ok_or(Error::NotFound)?;
                record.members.retain(|member| *member != principal);
                if present {
                    record.members.push(principal);
                }
            }
            Command::CreateProject { name } => {
                let project = Project {
                    scope: ProjectScope {
                        organization: self.organization.id,
                        project: ProjectId::new(),
                    },
                    name: model::name(&name)?,
                };
                self.projects.push(project.clone());
                // A creator's access is an explicit grant committed with the
                // project, not a hidden organization-role bypass.
                self.grants.push(Grant {
                    id: GrantId::new(),
                    scope: project.scope,
                    grantee: Grantee::User(actor.clone()),
                    role: ProjectRole::Admin,
                    window: GrantWindow::permanent(now),
                });
                return Ok(Change::ProjectCreated(project));
            }
            Command::Grant {
                project,
                grantee,
                role,
                window,
            } => {
                let scope = self.manage_project(actor, project, now)?;
                window.validate()?;
                match &grantee {
                    Grantee::User(user) => {
                        self.member_role(user)?;
                    }
                    Grantee::Team(team)
                        if !self.teams.iter().any(|entry| entry.team.id == *team) =>
                    {
                        return Err(Error::NotFound);
                    }
                    Grantee::Team(_) => {}
                }
                let grant = Grant {
                    id: GrantId::new(),
                    scope,
                    grantee,
                    role,
                    window,
                };
                self.grants.push(grant.clone());
                return Ok(Change::GrantCreated(grant));
            }
            Command::RevokeGrant { project, grant } => {
                let scope = self.manage_project(actor, project, now)?;
                if !self
                    .grants
                    .iter()
                    .any(|entry| entry.id == grant && entry.scope == scope)
                {
                    return Err(Error::NotFound);
                }
                self.grants.retain(|entry| entry.id != grant);
            }
        }
        Ok(Change::Applied)
    }
}
