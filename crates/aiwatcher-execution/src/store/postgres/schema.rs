//! Applying the schema, once, however many replicas start together.
//!
//! Not a migration framework: files in order, every statement re-runnable,
//! applied inside one transaction holding an advisory lock, so two pods
//! starting in the same second apply them once. A version is recorded in
//! `execution_schema_migrations` after its statements commit and skipped
//! thereafter — [`aiwatcher_jobs::ORDERING`], the work then the cursor.
//!
//! `sqlx::migrate!` needs `sqlx-macros`, which wants a live database at compile
//! time; forty lines of `include_str!` are easier to reason about than a
//! proc-macro reading `DATABASE_URL` during `cargo build`.
//!
//! **A file may not remove something a released binary still names.** The
//! schema is applied at start-up by whichever replica gets there first, workers
//! roll rather than stop, and an image rollback runs the old binary against the
//! new schema. So a removal is two releases: one that stops using the thing,
//! and a later one that drops it. Adding is unconstrained — a column the old
//! binary never heard of costs it nothing.
//!
//! **An applied file is never edited.** A rewrite reaches no database that ran
//! it and only makes two installations at one version disagree about what that
//! version did. What withdraws a migration is another migration.

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
    (
        3,
        include_str!("../../../migrations/0003_drop_dead_timestamps.sql"),
    ),
    (
        4,
        include_str!("../../../migrations/0004_retire_finished_attempts.sql"),
    ),
    (
        5,
        include_str!("../../../migrations/0005_restore_run_timestamps.sql"),
    ),
    (
        6,
        include_str!("../../../migrations/0006_schedule_slots.sql"),
    ),
    (
        7,
        include_str!("../../../migrations/0007_hosted_messages.sql"),
    ),
    (
        8,
        include_str!("../../../migrations/0008_decider_leases.sql"),
    ),
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
