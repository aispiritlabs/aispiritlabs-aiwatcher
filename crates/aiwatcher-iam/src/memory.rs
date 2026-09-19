//! Test/development adapter. Authorization and mutation share one lock.

use crate::{policy, policy::OrganizationState, *};
use async_trait::async_trait;
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::RwLock;

#[derive(Debug)]
struct AuditedOrganization {
    state: OrganizationState,
    audit: Vec<AuditEntry>,
    /// Where a sweep moved the beginning of `audit`, when one has. The
    /// PostgreSQL adapter keeps this in a table of its own; here it is a field
    /// beside the entries it explains, which is the same thing under one lock.
    watermark: Option<AuditWatermark>,
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
                watermark: None,
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

    async fn authorize_audit(&self, organization: OrganizationId, actor: &Principal) -> Result<()> {
        self.organizations
            .read()
            .await
            .get(&organization)
            .ok_or(Error::NotFound)?
            .state
            .authorize_audit(actor)
    }

    async fn audit_bounds(
        &self,
        organization: OrganizationId,
        actor: &Principal,
    ) -> Result<AuditBounds> {
        let organizations = self.organizations.read().await;
        let stored = organizations.get(&organization).ok_or(Error::NotFound)?;
        stored.state.authorize_audit(actor)?;
        Ok(AuditBounds {
            organization,
            first_sequence: stored.audit.first().map(|entry| entry.sequence),
            last_sequence: stored.audit.last().map(|entry| entry.sequence),
            entries: i64::try_from(stored.audit.len())
                .map_err(|error| Error::Backend(error.to_string()))?,
            watermark: stored.watermark.clone(),
        })
    }

    async fn prune_audit(&self, retention: &AuditRetention, now: i64) -> Result<PruneReport> {
        let cutoff = retention.cutoff(now);
        let mut report = PruneReport {
            cutoff,
            ..PruneReport::default()
        };
        for (organization, stored) in self.organizations.write().await.iter_mut() {
            // The newest entry is never swept, which is what keeps the next
            // sequence correct on a trail retention has been through. The
            // PostgreSQL adapter keeps the same invariant, for the sharper
            // reason that a binary predating retention computes that sequence
            // from `max(sequence)` alone.
            let Some(top) = stored.audit.last().map(|entry| entry.sequence) else {
                continue;
            };
            let removed = stored
                .audit
                .iter()
                .filter(|entry| entry.occurred_at < cutoff && entry.sequence < top)
                .map(|entry| entry.sequence)
                .collect::<Vec<_>>();
            let Some(through) = removed.iter().copied().max() else {
                continue;
            };
            stored
                .audit
                .retain(|entry| entry.occurred_at >= cutoff || entry.sequence >= top);
            let count =
                i64::try_from(removed.len()).map_err(|error| Error::Backend(error.to_string()))?;
            let previous = stored
                .watermark
                .as_ref()
                .map_or(0, |mark| mark.removed_total);
            stored.watermark = Some(AuditWatermark {
                organization: *organization,
                pruned_through_sequence: through,
                pruned_before: cutoff,
                policy_id: retention.policy_id.clone(),
                ttl_days: retention.ttl_days,
                pruned_at: now,
                removed_total: previous + count,
            });
            report.organizations += 1;
            report.removed += count;
        }
        Ok(report)
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
        let sequence = next_sequence(stored);
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
    async fn offered(&self, token: &str) -> Result<Offered> {
        let digest = digest_of(token);
        let organizations = self.organizations.read().await;
        let now = self.clock.now();
        organizations
            .values()
            .find(|stored| stored.state.holds_invitation(&digest))
            .ok_or(Error::NotFound)?
            .state
            .offered(&digest, now)
    }

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

/// The next sequence, counted from the last entry rather than from how many
/// there are.
///
/// Those were the same number until retention could remove the oldest. They are
/// not now, and taking the length would reissue sequences a swept trail has
/// already used — so a reader paging from the watermark would meet two entries
/// with one number.
fn next_sequence(stored: &AuditedOrganization) -> i64 {
    stored
        .audit
        .last()
        .map(|entry| entry.sequence)
        .or_else(|| stored.watermark.as_ref().map(|w| w.pruned_through_sequence))
        .unwrap_or(0)
        + 1
}

fn push_audit(
    stored: &mut AuditedOrganization,
    organization: OrganizationId,
    actor: &Principal,
    occurred_at: i64,
    action: AuditAction,
) -> Result<()> {
    let sequence = next_sequence(stored);
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
