//! Test/development adapter. Authorization and mutation share one lock.

use crate::{policy::OrganizationState, *};
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

    async fn access(&self, scope: ProjectScope, actor: &Principal) -> Result<ProjectAccess> {
        self.organizations
            .read()
            .await
            .get(&scope.organization)
            .ok_or(Error::NotFound)?
            .state
            .access(scope, actor, self.clock.now())
    }
}
