//! Applying the schema, once, however many replicas start together.
//!
//! Not a migration framework. There is one file, every statement in it is
//! `IF NOT EXISTS`, and it runs inside one transaction holding a PostgreSQL
//! advisory lock — so two API pods starting at the same second apply it once
//! and the second waits rather than racing the first through `CREATE TABLE`.
//!
//! What makes a *second* file possible later is
//! `execution_schema_migrations`: a version is recorded after its statements
//! commit, and a version already recorded is skipped. That ordering is
//! [`aiwatcher_jobs::ORDERING`] again — the work, then the cursor that passes
//! it. A crash between them re-runs a file that is idempotent by construction.
//!
//! `sqlx`'s own `migrate!` was the alternative and needs `sqlx-macros`, which
//! wants a live database at compile time. A proc-macro reading `DATABASE_URL`
//! during `cargo build` is a larger thing to reason about than forty lines that
//! run `include_str!`.

use sqlx::{Executor as _, PgPool};

use super::error::PostgresError;
use crate::Result;

/// Chosen once and never changed: it is the *name* of this lock, and two builds
/// that disagreed about it would not exclude each other. `0x_A1_0025` reads as
/// "aiwatcher, ADR 0025" and collides with nothing else in a shared database
/// that this system does not also own.
const ADVISORY_LOCK: i64 = 0x00A1_0025;

/// Every schema file, in order, with the version it records.
const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../../../migrations/0001_execution.sql")),
    (2, include_str!("../../../migrations/0002_retention.sql")),
];

/// Bring the database up to the schema this build expects.
///
/// # Errors
///
/// [`PostgresError::Migration`] naming the version that failed.
pub async fn apply(pool: &PgPool) -> Result<()> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| crate::StoreError::Backend(PostgresError::from(error).to_string()))?;

    // Held for the whole transaction and released by the commit. A second pod
    // waits here rather than racing this one through `CREATE TABLE`.
    sqlx::query("select pg_advisory_xact_lock($1)")
        .bind(ADVISORY_LOCK)
        .execute(&mut *transaction)
        .await
        .map_err(|error| crate::StoreError::Backend(PostgresError::from(error).to_string()))?;

    transaction
        .execute(
            "create table if not exists execution_schema_migrations (
                 version    bigint      primary key,
                 applied_at timestamptz not null default now()
             )",
        )
        .await
        .map_err(|error| crate::StoreError::Backend(PostgresError::from(error).to_string()))?;

    for (version, sql) in MIGRATIONS {
        let applied: Option<i64> = sqlx::query_scalar(
            "select version from execution_schema_migrations where version = $1",
        )
        .bind(version)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|error| crate::StoreError::Backend(PostgresError::from(error).to_string()))?;
        if applied.is_some() {
            continue;
        }
        transaction.execute(*sql).await.map_err(|error| {
            crate::StoreError::Backend(
                PostgresError::Migration {
                    version: *version,
                    source: error,
                }
                .to_string(),
            )
        })?;
        sqlx::query("insert into execution_schema_migrations (version) values ($1)")
            .bind(version)
            .execute(&mut *transaction)
            .await
            .map_err(|error| crate::StoreError::Backend(PostgresError::from(error).to_string()))?;
    }

    transaction
        .commit()
        .await
        .map_err(|error| crate::StoreError::Backend(PostgresError::from(error).to_string()))?;
    Ok(())
}
