//! One organization's authoritative membership/grant aggregate. Both adapters
//! run this policy while holding their write lock, so a concurrent demotion
//! cannot be checked against a stale snapshot and committed afterward.

use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const SCHEMA_VERSION: u32 = 1;
const MAX_DOCUMENT_BYTES: usize = 4 * 1024 * 1024;

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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OrganizationState {
    schema_version: u32,
    pub organization: Organization,
    members: Vec<Member>,
    teams: Vec<TeamRecord>,
    projects: Vec<Project>,
    grants: Vec<Grant>,
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

    pub fn member_role(&self, actor: &Principal) -> Result<OrganizationRole> {
        actor.validate()?;
        self.members
            .iter()
            .find(|member| member.principal == *actor)
            .map(|member| member.role)
            .ok_or(Error::NotFound)
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
