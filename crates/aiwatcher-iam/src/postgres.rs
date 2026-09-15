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

    async fn access(&self, scope: ProjectScope, actor: &Principal) -> Result<ProjectAccess> {
        self.load(scope.organization)
            .await?
            .access(scope, actor, self.clock.now())
    }
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
    if version.is_some_and(|version| version > 2) {
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
    tx.commit().await.map_err(backend)?;
    Ok(())
}

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
