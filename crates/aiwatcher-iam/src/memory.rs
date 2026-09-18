//! Test/development adapter. Authorization and mutation share one lock.

use crate::{policy, policy::OrganizationState, *};
use async_trait::async_trait;
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::RwLock;

#[derive(Debug)]
struct AuditedOrganization {
    state: OrganizationState,
    audit: Vec<AuditEntry>,
}

#[derive(Debug)]
pub struct MemoryIamStore {
    organizations: RwLock<BTreeMap<OrganizationId, AuditedOrganization>>,
    clock: Arc<dyn Clock>,
}

impl Default for MemoryIamStore {
    fn default() -> Self {
        Self::with_clock(Arc::new(SystemClock))
    }
}

impl MemoryIamStore {
    #[must_use]
    pub fn with_clock(clock: Arc<dyn Clock>) -> Self {
        Self {
            organizations: RwLock::new(BTreeMap::new()),
            clock,
        }
    }
}

#[async_trait]
impl IamStore for MemoryIamStore {
    async fn create_organization(&self, owner: &Principal, name: &str) -> Result<Organization> {
        let state = OrganizationState::new(owner, name)?;
        state.encode()?;
        let organization = state.organization.clone();
        self.organizations.write().await.insert(
            organization.id,
            AuditedOrganization {
                state,
                audit: vec![AuditEntry {
                    sequence: 1,
                    organization: organization.id,
                    actor: owner.clone(),
                    occurred_at: self.clock.now(),
                    action: AuditAction::OrganizationCreated {
                        organization: organization.clone(),
                    },
                }],
            },
        );
        Ok(organization)
    }

    async fn audit(
        &self,
        organization: OrganizationId,
        actor: &Principal,
        after: i64,
        limit: usize,
    ) -> Result<Vec<AuditEntry>> {
        model::audit_limit(after, limit)?;
        let organizations = self.organizations.read().await;
        let entry = organizations.get(&organization).ok_or(Error::NotFound)?;
        entry.state.authorize_audit(actor)?;
        Ok(entry
            .audit
            .iter()
            .filter(|e| e.sequence > after)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn organizations(&self, actor: &Principal) -> Result<Vec<Organization>> {
        actor.validate()?;
        Ok(self
            .organizations
            .read()
            .await
            .values()
            .filter(|entry| entry.state.member_role(actor).is_ok())
            .map(|entry| entry.state.organization.clone())
            .collect())
    }

    async fn apply(
        &self,
        organization: OrganizationId,
        actor: &Principal,
        command: Command,
    ) -> Result<Change> {
        let mut organizations = self.organizations.write().await;
        let stored = organizations
            .get_mut(&organization)
            .ok_or(Error::NotFound)?;
        let mut next = stored.state.clone();
        let now = self.clock.now();
        let result = next.apply(actor, command.clone(), now)?;
        next.encode()?;
        let sequence =
            i64::try_from(stored.audit.len()).map_err(|e| Error::Backend(e.to_string()))? + 1;
        stored.audit.push(AuditEntry {
            sequence,
            organization,
            actor: actor.clone(),
            occurred_at: now,
            action: AuditAction::CommandApplied {
                command: Box::new(command),
                change: result.clone(),
            },
        });
        stored.state = next;
        Ok(result)
    }

    async fn projects(
        &self,
        organization: OrganizationId,
        actor: &Principal,
    ) -> Result<Vec<ProjectAccess>> {
        self.organizations
            .read()
            .await
            .get(&organization)
            .ok_or(Error::NotFound)?
            .state
            .projects(actor, self.clock.now())
    }

    async fn roster(&self, organization: OrganizationId, actor: &Principal) -> Result<Roster> {
        self.organizations
            .read()
            .await
            .get(&organization)
            .ok_or(Error::NotFound)?
            .state
            .roster(actor)
    }

    async fn project_grants(&self, scope: ProjectScope, actor: &Principal) -> Result<Vec<Grant>> {
        self.organizations
            .read()
            .await
            .get(&scope.organization)
            .ok_or(Error::NotFound)?
            .state
            .project_grants(scope, actor, self.clock.now())
    }

    async fn access(&self, scope: ProjectScope, actor: &Principal) -> Result<ProjectAccess> {
        self.organizations
            .read()
            .await
            .get(&scope.organization)
            .ok_or(Error::NotFound)?
            .state
            .access(scope, actor, self.clock.now())
    }

    async fn invite(
        &self,
        scope: ProjectScope,
        actor: &Principal,
        offer: InvitationOffer,
    ) -> Result<IssuedInvitation> {
        let token = mint_invitation_token();
        let mut organizations = self.organizations.write().await;
        let stored = organizations
            .get_mut(&scope.organization)
            .ok_or(Error::NotFound)?;
        let mut next = stored.state.clone();
        let now = self.clock.now();
        let invitation = next.invite(actor, scope.project, offer, digest_of(&token), now)?;
        next.encode()?;
        push_audit(
            stored,
            scope.organization,
            actor,
            now,
            AuditAction::InvitationCreated {
                invitation: Box::new(invitation.clone()),
            },
        )?;
        stored.state = next;
        Ok(IssuedInvitation { invitation, token })
    }

    async fn invitations(
        &self,
        organization: OrganizationId,
        actor: &Principal,
    ) -> Result<Vec<Invitation>> {
        self.organizations
            .read()
            .await
            .get(&organization)
            .ok_or(Error::NotFound)?
            .state
            .invitations(actor, self.clock.now())
    }

    async fn revoke_invitation(
        &self,
        organization: OrganizationId,
        actor: &Principal,
        invitation: InvitationId,
    ) -> Result<()> {
        let mut organizations = self.organizations.write().await;
        let stored = organizations
            .get_mut(&organization)
            .ok_or(Error::NotFound)?;
        let mut next = stored.state.clone();
        let now = self.clock.now();
        next.revoke_invitation(actor, invitation, now)?;
        next.encode()?;
        push_audit(
            stored,
            organization,
            actor,
            now,
            AuditAction::InvitationRevoked { invitation },
        )?;
        stored.state = next;
        Ok(())
    }

    /// A scan across organizations, which a development adapter can afford and
    /// the PostgreSQL one replaces with an index. The digest is what is
    /// compared, never the token.
    async fn redeem(&self, token: &str, redeemer: &Principal) -> Result<Redeemed> {
        let digest = digest_of(token);
        let mut organizations = self.organizations.write().await;
        let id = *organizations
            .iter()
            .find(|(_, stored)| stored.state.holds_invitation(&digest))
            .map(|(id, _)| id)
            .ok_or(Error::NotFound)?;
        let stored = organizations.get_mut(&id).ok_or(Error::NotFound)?;
        let mut next = stored.state.clone();
        let now = self.clock.now();
        let redeemed = next.redeem(&digest, redeemer, now)?;
        next.encode()?;
        push_audit(
            stored,
            id,
            redeemer,
            now,
            AuditAction::InvitationRedeemed {
                invitation: invitation_id(&next, redeemed.grant)?,
                grant: redeemed.grant,
            },
        )?;
        stored.state = next;
        Ok(redeemed)
    }
}

fn push_audit(
    stored: &mut AuditedOrganization,
    organization: OrganizationId,
    actor: &Principal,
    occurred_at: i64,
    action: AuditAction,
) -> Result<()> {
    let sequence =
        i64::try_from(stored.audit.len()).map_err(|e| Error::Backend(e.to_string()))? + 1;
    stored.audit.push(AuditEntry {
        sequence,
        organization,
        actor: actor.clone(),
        occurred_at,
        action,
    });
    Ok(())
}

/// Which offer produced a grant, for the audit entry beside it.
fn invitation_id(state: &policy::OrganizationState, grant: GrantId) -> Result<InvitationId> {
    state
        .invitation_of(grant)
        .ok_or_else(|| Error::Backend("a redemption produced no invitation".into()))
}
