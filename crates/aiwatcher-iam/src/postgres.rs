//! PostgreSQL adapter. Each organization's ACL is a versioned JSONB aggregate:
//! one row lock covers both permission checks and membership/grant mutations.
//! Different organizations can be updated concurrently. This deliberately
//! serializes control-plane writes within one organization; it is not a store
//! for per-resource ACLs or telemetry. See README for the size/scale boundary.

use crate::{policy::OrganizationState, *};
use async_trait::async_trait;
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct PostgresIamStore {
    pool: PgPool,
    clock: Arc<dyn Clock>,
}

impl PostgresIamStore {
    pub async fn connect(url: &str, max_connections: u32) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .connect(url)
            .await
            .map_err(backend)?;
        Self::from_pool(pool, Arc::new(SystemClock)).await
    }

    /// Apply additive schema migrations under an advisory lock. Tests can
    /// inject a clock; a server uses SystemClock, never client-supplied time.
    pub async fn from_pool(pool: PgPool, clock: Arc<dyn Clock>) -> Result<Self> {
        migrate(&pool).await?;
        Ok(Self { pool, clock })
    }

    async fn load(&self, id: OrganizationId) -> Result<OrganizationState> {
        let value: Option<serde_json::Value> =
            sqlx::query_scalar("SELECT document FROM iam_organizations WHERE id = $1")
                .bind(id.0)
                .fetch_optional(&self.pool)
                .await
                .map_err(backend)?;
        OrganizationState::decode(id, value.ok_or(Error::NotFound)?)
    }
}

#[async_trait]
impl IamStore for PostgresIamStore {
    async fn create_organization(&self, owner: &Principal, name: &str) -> Result<Organization> {
        let state = OrganizationState::new(owner, name)?;
        let mut tx = self.pool.begin().await.map_err(backend)?;
        sqlx::query("INSERT INTO iam_organizations (id, document) VALUES ($1, $2)")
            .bind(state.organization.id.0)
            .bind(state.encode()?)
            .execute(&mut *tx)
            .await
            .map_err(backend)?;
        append_audit(
            &mut tx,
            state.organization.id,
            owner,
            self.clock.now(),
            AuditAction::OrganizationCreated {
                organization: state.organization.clone(),
            },
        )
        .await?;
        tx.commit().await.map_err(backend)?;
        Ok(state.organization)
    }

    async fn audit(
        &self,
        organization: OrganizationId,
        actor: &Principal,
        after: i64,
        limit: usize,
    ) -> Result<Vec<AuditEntry>> {
        model::audit_limit(after, limit)?;
        let mut tx = self.pool.begin().await.map_err(backend)?;
        // Keep authorization and page read together against concurrent demotion.
        let value: Option<serde_json::Value> =
            sqlx::query_scalar("SELECT document FROM iam_organizations WHERE id = $1 FOR SHARE")
                .bind(organization.0)
                .fetch_optional(&mut *tx)
                .await
                .map_err(backend)?;
        OrganizationState::decode(organization, value.ok_or(Error::NotFound)?)?
            .authorize_audit(actor)?;
        let entries: Vec<serde_json::Value> = sqlx::query_scalar(
            "SELECT entry FROM iam_audit WHERE organization_id = $1 AND sequence > $2 ORDER BY sequence LIMIT $3")
            .bind(organization.0).bind(after).bind(i64::try_from(limit).map_err(|e| Error::Invalid(e.to_string()))?)
            .fetch_all(&mut *tx).await.map_err(backend)?;
        let entries = entries
            .into_iter()
            .map(|value| {
                serde_json::from_value(value).map_err(|e| Error::Incompatible(e.to_string()))
            })
            .collect::<Result<Vec<_>>>()?;
        tx.commit().await.map_err(backend)?;
        Ok(entries)
    }

    async fn authorize_audit(&self, organization: OrganizationId, actor: &Principal) -> Result<()> {
        self.load(organization).await?.authorize_audit(actor)
    }

    async fn audit_bounds(
        &self,
        organization: OrganizationId,
        actor: &Principal,
    ) -> Result<AuditBounds> {
        let mut tx = self.pool.begin().await.map_err(backend)?;
        // Authorization and the counts under one shared lock, as the page read
        // above is, so a demotion cannot land between them.
        let value: Option<serde_json::Value> =
            sqlx::query_scalar("SELECT document FROM iam_organizations WHERE id = $1 FOR SHARE")
                .bind(organization.0)
                .fetch_optional(&mut *tx)
                .await
                .map_err(backend)?;
        OrganizationState::decode(organization, value.ok_or(Error::NotFound)?)?
            .authorize_audit(actor)?;
        let row = sqlx::query(
            "SELECT min(sequence) AS first, max(sequence) AS last, count(*) AS entries \
             FROM iam_audit WHERE organization_id = $1",
        )
        .bind(organization.0)
        .fetch_one(&mut *tx)
        .await
        .map_err(backend)?;
        let watermark = read_watermark(&mut tx, organization).await?;
        tx.commit().await.map_err(backend)?;
        Ok(AuditBounds {
            organization,
            first_sequence: row.try_get("first").map_err(backend)?,
            last_sequence: row.try_get("last").map_err(backend)?,
            entries: row.try_get("entries").map_err(backend)?,
            watermark,
        })
    }

    /// One statement for the delete, one for each watermark it produced, one
    /// transaction.
    ///
    /// **The newest entry of an organization is never swept.** That is what
    /// keeps `append_audit`'s `max(sequence) + 1` correct on a trail retention
    /// has been through — including in a binary that predates retention, which
    /// is what makes turning it on safe during a rolling upgrade and safe to
    /// roll back from. One retained row per organization is a small price for
    /// not reissuing a sequence number, and a trail that is empty except for a
    /// watermark tells a reader less than one that still shows its last
    /// administrative act.
    async fn prune_audit(&self, retention: &AuditRetention, now: i64) -> Result<PruneReport> {
        let cutoff = retention.cutoff(now);
        let mut tx = self.pool.begin().await.map_err(backend)?;
        let removed = sqlx::query(
            "WITH tops AS ( \
                 SELECT organization_id, max(sequence) AS top FROM iam_audit \
                 GROUP BY organization_id \
             ), gone AS ( \
                 DELETE FROM iam_audit AS a USING tops \
                 WHERE tops.organization_id = a.organization_id \
                   AND a.sequence < tops.top \
                   AND (a.entry ->> 'occurred_at')::bigint < $1 \
                 RETURNING a.organization_id, a.sequence \
             ) \
             SELECT organization_id, count(*) AS removed, max(sequence) AS through \
             FROM gone GROUP BY organization_id",
        )
        .bind(cutoff)
        .fetch_all(&mut *tx)
        .await
        .map_err(backend)?;

        let mut report = PruneReport {
            cutoff,
            ..PruneReport::default()
        };
        for row in removed {
            let organization: uuid::Uuid = row.try_get("organization_id").map_err(backend)?;
            let count: i64 = row.try_get("removed").map_err(backend)?;
            let through: i64 = row.try_get("through").map_err(backend)?;
            sqlx::query(
                "INSERT INTO iam_audit_retention (organization_id, pruned_through_sequence, \
                     pruned_before, policy_id, ttl_days, pruned_at, removed_total) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7) \
                 ON CONFLICT (organization_id) DO UPDATE SET \
                     pruned_through_sequence = GREATEST( \
                         iam_audit_retention.pruned_through_sequence, \
                         EXCLUDED.pruned_through_sequence), \
                     pruned_before = EXCLUDED.pruned_before, \
                     policy_id = EXCLUDED.policy_id, \
                     ttl_days = EXCLUDED.ttl_days, \
                     pruned_at = EXCLUDED.pruned_at, \
                     removed_total = iam_audit_retention.removed_total + EXCLUDED.removed_total",
            )
            .bind(organization)
            .bind(through)
            .bind(cutoff)
            .bind(&retention.policy_id)
            .bind(i32::try_from(retention.ttl_days).map_err(|e| Error::Invalid(e.to_string()))?)
            .bind(now)
            .bind(count)
            .execute(&mut *tx)
            .await
            .map_err(backend)?;
            report.organizations += 1;
            report.removed += count;
        }
        tx.commit().await.map_err(backend)?;
        Ok(report)
    }

    async fn organizations(&self, actor: &Principal) -> Result<Vec<Organization>> {
        actor.validate()?;
        let rows = sqlx::query("SELECT id, document FROM iam_organizations WHERE document -> 'members' @> $1::jsonb ORDER BY id")
            .bind(serde_json::json!([{ "principal": actor }])).fetch_all(&self.pool).await.map_err(backend)?;
        rows.into_iter()
            .map(|row| {
                let state = OrganizationState::decode(
                    OrganizationId(row.try_get("id").map_err(backend)?),
                    row.try_get("document").map_err(backend)?,
                )?;
                // Recheck the exact pair after deserializing, not only JSON containment.
                state.member_role(actor)?;
                Ok(state.organization)
            })
            .collect()
    }

    async fn apply(
        &self,
        organization: OrganizationId,
        actor: &Principal,
        command: Command,
    ) -> Result<Change> {
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let value: Option<serde_json::Value> =
            sqlx::query_scalar("SELECT document FROM iam_organizations WHERE id = $1 FOR UPDATE")
                .bind(organization.0)
                .fetch_optional(&mut *transaction)
                .await
                .map_err(backend)?;
        let mut state = OrganizationState::decode(organization, value.ok_or(Error::NotFound)?)?;
        // Sample after the lock: a grant may have expired while this command
        // waited behind another replica. No cached role or earlier check.
        let now = self.clock.now();
        let change = state.apply(actor, command.clone(), now)?;
        sqlx::query("UPDATE iam_organizations SET document = $2 WHERE id = $1")
            .bind(organization.0)
            .bind(state.encode()?)
            .execute(&mut *transaction)
            .await
            .map_err(backend)?;
        append_audit(
            &mut transaction,
            organization,
            actor,
            now,
            AuditAction::CommandApplied {
                command: Box::new(command),
                change: change.clone(),
            },
        )
        .await?;
        transaction.commit().await.map_err(backend)?;
        Ok(change)
    }

    async fn projects(
        &self,
        organization: OrganizationId,
        actor: &Principal,
    ) -> Result<Vec<ProjectAccess>> {
        self.load(organization)
            .await?
            .projects(actor, self.clock.now())
    }

    async fn roster(&self, organization: OrganizationId, actor: &Principal) -> Result<Roster> {
        self.load(organization).await?.roster(actor)
    }

    async fn project_grants(&self, scope: ProjectScope, actor: &Principal) -> Result<Vec<Grant>> {
        self.load(scope.organization)
            .await?
            .project_grants(scope, actor, self.clock.now())
    }

    async fn access(&self, scope: ProjectScope, actor: &Principal) -> Result<ProjectAccess> {
        self.load(scope.organization)
            .await?
            .access(scope, actor, self.clock.now())
    }

    async fn invite(
        &self,
        scope: ProjectScope,
        actor: &Principal,
        offer: InvitationOffer,
    ) -> Result<IssuedInvitation> {
        let token = mint_invitation_token();
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let mut state = lock(&mut transaction, scope.organization).await?;
        let now = self.clock.now();
        let invitation = state.invite(actor, scope.project, offer, digest_of(&token), now)?;
        store(&mut transaction, scope.organization, &state).await?;
        append_audit(
            &mut transaction,
            scope.organization,
            actor,
            now,
            AuditAction::InvitationCreated {
                invitation: Box::new(invitation.clone()),
            },
        )
        .await?;
        transaction.commit().await.map_err(backend)?;
        Ok(IssuedInvitation { invitation, token })
    }

    async fn invitations(
        &self,
        organization: OrganizationId,
        actor: &Principal,
    ) -> Result<Vec<Invitation>> {
        self.load(organization)
            .await?
            .invitations(actor, self.clock.now())
    }

    async fn revoke_invitation(
        &self,
        organization: OrganizationId,
        actor: &Principal,
        invitation: InvitationId,
    ) -> Result<()> {
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let mut state = lock(&mut transaction, organization).await?;
        let now = self.clock.now();
        state.revoke_invitation(actor, invitation, now)?;
        store(&mut transaction, organization, &state).await?;
        append_audit(
            &mut transaction,
            organization,
            actor,
            now,
            AuditAction::InvitationRevoked { invitation },
        )
        .await?;
        transaction.commit().await.map_err(backend)?;
        Ok(())
    }

    /// The one read here that starts with no organization in hand: a redeemer
    /// holds a token, not an id. The digest finds the row through the index on
    /// `invitations`, and the same statement locks it — so two people racing
    /// one offer serialize, and the second is told it is spent rather than
    /// granted a second time.
    async fn offered(&self, token: &str) -> Result<Offered> {
        // A plain read, so no row lock: this settles nothing, and an offer that
        // is spent between looking and taking is the refusal `redeem` already
        // serializes.
        let digest = digest_of(token);
        let row = sqlx::query(
            "SELECT id, document FROM iam_organizations \
             WHERE document -> 'invitations' @> $1::jsonb",
        )
        .bind(serde_json::json!([{ "token_sha256": digest }]))
        .fetch_optional(&self.pool)
        .await
        .map_err(backend)?
        .ok_or(Error::NotFound)?;
        let organization = OrganizationId(row.try_get("id").map_err(backend)?);
        OrganizationState::decode(organization, row.try_get("document").map_err(backend)?)?
            .offered(&digest, self.clock.now())
    }

    async fn redeem(&self, token: &str, redeemer: &Principal) -> Result<Redeemed> {
        let digest = digest_of(token);
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let row = sqlx::query(
            "SELECT id, document FROM iam_organizations \
             WHERE document -> 'invitations' @> $1::jsonb FOR UPDATE",
        )
        .bind(serde_json::json!([{ "token_sha256": digest }]))
        .fetch_optional(&mut *transaction)
        .await
        .map_err(backend)?
        .ok_or(Error::NotFound)?;
        let organization = OrganizationId(row.try_get("id").map_err(backend)?);
        let mut state =
            OrganizationState::decode(organization, row.try_get("document").map_err(backend)?)?;
        let now = self.clock.now();
        let redeemed = state.redeem(&digest, redeemer, now)?;
        let invitation = state
            .invitation_of(redeemed.grant)
            .ok_or_else(|| Error::Backend("a redemption produced no invitation".into()))?;
        store(&mut transaction, organization, &state).await?;
        append_audit(
            &mut transaction,
            organization,
            redeemer,
            now,
            AuditAction::InvitationRedeemed {
                invitation,
                grant: redeemed.grant,
            },
        )
        .await?;
        transaction.commit().await.map_err(backend)?;
        Ok(redeemed)
    }
}

async fn read_watermark(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    organization: OrganizationId,
) -> Result<Option<AuditWatermark>> {
    let row = sqlx::query(
        "SELECT pruned_through_sequence, pruned_before, policy_id, ttl_days, pruned_at, \
                removed_total \
         FROM iam_audit_retention WHERE organization_id = $1",
    )
    .bind(organization.0)
    .fetch_optional(&mut **tx)
    .await
    .map_err(backend)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let ttl_days: i32 = row.try_get("ttl_days").map_err(backend)?;
    Ok(Some(AuditWatermark {
        organization,
        pruned_through_sequence: row.try_get("pruned_through_sequence").map_err(backend)?,
        pruned_before: row.try_get("pruned_before").map_err(backend)?,
        policy_id: row.try_get("policy_id").map_err(backend)?,
        ttl_days: u32::try_from(ttl_days).map_err(|e| Error::Incompatible(e.to_string()))?,
        pruned_at: row.try_get("pruned_at").map_err(backend)?,
        removed_total: row.try_get("removed_total").map_err(backend)?,
    }))
}

/// Take the organization's write lock and read its document under it.
async fn lock(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    organization: OrganizationId,
) -> Result<OrganizationState> {
    let value: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT document FROM iam_organizations WHERE id = $1 FOR UPDATE")
            .bind(organization.0)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(backend)?;
    OrganizationState::decode(organization, value.ok_or(Error::NotFound)?)
}

async fn store(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    organization: OrganizationId,
    state: &OrganizationState,
) -> Result<()> {
    sqlx::query("UPDATE iam_organizations SET document = $2 WHERE id = $1")
        .bind(organization.0)
        .bind(state.encode()?)
        .execute(&mut **transaction)
        .await
        .map_err(backend)?;
    Ok(())
}

fn backend(error: sqlx::Error) -> Error {
    Error::Backend(error.to_string())
}

async fn migrate(pool: &PgPool) -> Result<()> {
    let mut tx = pool.begin().await.map_err(backend)?;
    // A different lock/table namespace from execution-store migrations.
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(0x00A1_1A01_i64)
        .execute(&mut *tx)
        .await
        .map_err(backend)?;
    sqlx::query("CREATE TABLE IF NOT EXISTS iam_schema_migrations (version bigint PRIMARY KEY, applied_at timestamptz NOT NULL DEFAULT now())")
        .execute(&mut *tx).await.map_err(backend)?;
    let version: Option<i64> = sqlx::query_scalar("SELECT max(version) FROM iam_schema_migrations")
        .fetch_one(&mut *tx)
        .await
        .map_err(backend)?;
    if version.is_some_and(|version| version > 3) {
        return Err(Error::Incompatible(
            "the database has a newer IAM schema".into(),
        ));
    }
    if version.is_none() {
        sqlx::raw_sql(include_str!("../migrations/0001_organizations.sql"))
            .execute(&mut *tx)
            .await
            .map_err(backend)?;
        sqlx::query("INSERT INTO iam_schema_migrations (version) VALUES (1)")
            .execute(&mut *tx)
            .await
            .map_err(backend)?;
    }
    if version.is_none_or(|v| v < 2) {
        sqlx::raw_sql(include_str!("../migrations/0002_audit.sql"))
            .execute(&mut *tx)
            .await
            .map_err(backend)?;
        sqlx::query("INSERT INTO iam_schema_migrations (version) VALUES (2)")
            .execute(&mut *tx)
            .await
            .map_err(backend)?;
    }
    if version.is_none_or(|v| v < 3) {
        sqlx::raw_sql(include_str!("../migrations/0003_invitations.sql"))
            .execute(&mut *tx)
            .await
            .map_err(backend)?;
        sqlx::query("INSERT INTO iam_schema_migrations (version) VALUES (3)")
            .execute(&mut *tx)
            .await
            .map_err(backend)?;
    }
    additions(&mut tx).await?;
    tx.commit().await.map_err(backend)?;
    Ok(())
}

/// Migrations that add something no released binary reads, in their own ledger.
///
/// `iam_schema_migrations` records what a binary must *understand* in order to
/// read this database, which is why a version past its own is refused rather
/// than ignored: schema 2 changed what an audit entry means, and a binary that
/// silently skipped it would serve a history it did not know was incomplete.
///
/// This is the other kind. `iam_audit_retention` and the index beside it are
/// additive objects that nothing released selects from, and recording them in
/// that ledger would turn every rollback into a start-up refusal — for a change
/// the older binary cannot even see. So they have a ledger of their own, which
/// makes them idempotent without making them a compatibility claim, and the
/// older binary goes on reading `max(version) = 3` and starting.
///
/// The same reasoning is what decides what these migrations may contain: a
/// table or an index, never a column an older binary would have to write and
/// never a change to one it reads. The moment an addition stops being invisible
/// it belongs in the numbered ledger, with the refusal that comes with it.
async fn additions(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS iam_schema_additions \
         (name text PRIMARY KEY, applied_at timestamptz NOT NULL DEFAULT now())",
    )
    .execute(&mut **tx)
    .await
    .map_err(backend)?;
    let applied: Option<String> =
        sqlx::query_scalar("SELECT name FROM iam_schema_additions WHERE name = $1")
            .bind(AUDIT_RETENTION)
            .fetch_optional(&mut **tx)
            .await
            .map_err(backend)?;
    if applied.is_none() {
        sqlx::raw_sql(include_str!("../migrations/0004_audit_retention.sql"))
            .execute(&mut **tx)
            .await
            .map_err(backend)?;
        sqlx::query("INSERT INTO iam_schema_additions (name) VALUES ($1)")
            .bind(AUDIT_RETENTION)
            .execute(&mut **tx)
            .await
            .map_err(backend)?;
    }
    Ok(())
}

const AUDIT_RETENTION: &str = "0004_audit_retention";

async fn append_audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    organization: OrganizationId,
    actor: &Principal,
    occurred_at: i64,
    action: AuditAction,
) -> Result<()> {
    // The caller holds the organization row lock (or just inserted the row).
    let sequence: i64 = sqlx::query_scalar(
        "SELECT COALESCE(max(sequence), 0) + 1 FROM iam_audit WHERE organization_id = $1",
    )
    .bind(organization.0)
    .fetch_one(&mut **tx)
    .await
    .map_err(backend)?;
    let entry = AuditEntry {
        sequence,
        organization,
        actor: actor.clone(),
        occurred_at,
        action,
    };
    sqlx::query("INSERT INTO iam_audit (organization_id, sequence, entry) VALUES ($1, $2, $3)")
        .bind(organization.0)
        .bind(sequence)
        .bind(serde_json::to_value(entry).map_err(|e| Error::Backend(e.to_string()))?)
        .execute(&mut **tx)
        .await
        .map_err(backend)?;
    Ok(())
}
