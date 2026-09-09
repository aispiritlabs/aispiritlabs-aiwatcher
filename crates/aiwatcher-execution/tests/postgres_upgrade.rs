#![cfg(feature = "postgres")]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Upgrading a database a previous release already wrote to.
//!
//! `tests/postgres.rs` proves the adapter keeps the contract; this file proves
//! the *schema* one release leaves behind is one the release before it can
//! still use. They are different questions, and the review found the second one
//! unanswered: 0003 dropped two columns that the binary at `3117259` names in
//! both of its projection statements, workers roll rather than stop
//! (`deploy/helm/aiwatcher/templates/worker.yaml`), and an image rollback does
//! not put a column back. A fresh-database test and a twice-applied test can
//! both pass while that is true, which is why neither caught it.
//!
//! So the fixture here is not this crate's code. It is the **previous
//! release's SQL**, copied verbatim from `3117259` and pinned in
//! [`OLD_UPSERT_PROJECTION`], [`OLD_SELECT_PROJECTION`] and
//! [`OLD_CLAIM_ATTEMPT`]. A test that built the old statements out of the
//! current adapter's strings would agree with itself and prove nothing.
//!
//! Every test owns a PostgreSQL *schema* of its own and reaches it through
//! `search_path`, because these run DDL on `execution_runs` at historical
//! versions and the contract suite is using the shared one. The advisory lock
//! is database-wide, so they serialise on it — which is what `--test-threads=1`
//! already asks for.

use sqlx::postgres::PgPoolOptions;
use sqlx::{Executor as _, PgPool, Row as _};
use time::OffsetDateTime;

use aiwatcher_execution::store::WorkflowStore;
use aiwatcher_execution::store::postgres::{PostgresWorkflowStore, schema};
use aiwatcher_execution::testing::assert_contract;

/// Where `just postgres-up` puts it.
fn url() -> String {
    std::env::var("AIWATCHER_WORKFLOW_POSTGRES_URL")
        .unwrap_or_else(|_| "postgres://aiwatcher:aiwatcher@127.0.0.1:5433/aiwatcher".to_owned())
}

/// Every version this build brings a database to.
///
/// One place, because four tests assert it and a fifth migration should be a
/// one-line change here rather than a hunt. It is written out rather than read
/// from `schema`'s own list, which would make the assertion agree with itself.
const APPLIED: [i64; 9] = [1, 2, 3, 4, 5, 6, 7, 8, 9];

/// Every schema file a released build could have applied, with its version.
///
/// Deliberately not [`schema`]'s own list: this is what a database *arrived*
/// at, and reading it from the runner under test would make "upgrade from 2"
/// mean "whatever the current code calls 2".
const HISTORY: &[(i64, &str)] = &[
    (1, include_str!("../migrations/0001_execution.sql")),
    (2, include_str!("../migrations/0002_retention.sql")),
    (
        3,
        include_str!("../migrations/0003_drop_dead_timestamps.sql"),
    ),
    (
        4,
        include_str!("../migrations/0004_retire_finished_attempts.sql"),
    ),
];

// ── The previous release's statements, from `3117259` ────────────────────────

/// `upsert_projection` before this release stopped naming the two columns.
const OLD_UPSERT_PROJECTION: &str = "insert into execution_runs
       (execution_id, plan_id, definition_name, owner, mode, state_type,
        state_name, requested_by, steps, last_message_version,
        created_at, started_at, ended_at, updated_at)
     values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, now())
     on conflict (execution_id) do update set
       plan_id = excluded.plan_id,
       definition_name = excluded.definition_name,
       owner = excluded.owner,
       mode = excluded.mode,
       state_type = excluded.state_type,
       state_name = excluded.state_name,
       requested_by = excluded.requested_by,
       steps = excluded.steps,
       last_message_version = excluded.last_message_version,
       started_at = excluded.started_at,
       ended_at = excluded.ended_at,
       updated_at = now()";

/// The `SELECT` behind the old `projection`.
const OLD_SELECT_PROJECTION: &str = "select execution_id, plan_id, definition_name, owner, mode,
            state_type, state_name, requested_by, steps, last_message_version,
            created_at, started_at, ended_at
       from execution_runs where execution_id = $1";

/// The old claim query. Every column it names still exists, so what this
/// pins is that 0004 deleting rows did not change the answer it gives.
const OLD_CLAIM_ATTEMPT: &str = "select execution_id, step_id, attempt, runtime, command_id, queue,
            task_ref, state, lease_owner, previous_owner, claimed_at, not_before
       from step_attempts
      where state not in ('completed', 'failed', 'crashed', 'cancelled',
                          'awaiting_input')
      order by updated_at";

// ── A database at a historical version ───────────────────────────────────────

/// A pool onto an empty schema of this test's own, brought to `version` the way
/// a released build would have brought it there — the files, then the versions
/// recorded. `version` 0 is a database no build has ever opened.
///
/// Dropped first rather than after, so a failed run leaves its schema behind to
/// be looked at and the next run still starts clean.
async fn database_at(namespace: &str, version: i64) -> PgPool {
    assert!(
        namespace
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_'),
        "a namespace is interpolated into DDL, so it is a literal in this file"
    );

    let bootstrap = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url())
        .await
        .expect("a database — run `just postgres-up` first");
    bootstrap
        .execute(format!("drop schema if exists {namespace} cascade").as_str())
        .await
        .expect("a clean start");
    bootstrap
        .execute(format!("create schema {namespace}").as_str())
        .await
        .expect("a schema of this test's own");
    bootstrap.close().await;

    let search_path = namespace.to_owned();
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |connection, _| {
            let search_path = search_path.clone();
            Box::pin(async move {
                connection
                    .execute(format!("set search_path to {search_path}").as_str())
                    .await?;
                Ok(())
            })
        })
        .connect(&url())
        .await
        .expect("a pool scoped to it");

    if version == 0 {
        return pool;
    }

    pool.execute(
        "create table if not exists execution_schema_migrations (
             version    bigint      primary key,
             applied_at timestamptz not null default now()
         )",
    )
    .await
    .expect("the table the runner records into");
    for (recorded, sql) in HISTORY.iter().filter(|(at, _)| *at <= version) {
        pool.execute(*sql)
            .await
            .unwrap_or_else(|error| panic!("migration {recorded}: {error}"));
        sqlx::query("insert into execution_schema_migrations (version) values ($1)")
            .bind(recorded)
            .execute(&pool)
            .await
            .expect("a recorded version");
    }
    pool
}

async fn columns_of(pool: &PgPool, table: &str) -> Vec<String> {
    sqlx::query_scalar::<_, String>(
        "select column_name from information_schema.columns
          where table_schema = current_schema() and table_name = $1
          order by column_name",
    )
    .bind(table)
    .fetch_all(pool)
    .await
    .expect("the columns a table has")
}

async fn applied_versions(pool: &PgPool) -> Vec<i64> {
    sqlx::query_scalar::<_, i64>("select version from execution_schema_migrations order by version")
        .fetch_all(pool)
        .await
        .expect("the versions this database has applied")
}

/// Write a projection the way the release before this one wrote one.
async fn old_binary_writes(pool: &PgPool, execution: &str) -> Result<(), sqlx::Error> {
    sqlx::query(OLD_UPSERT_PROJECTION)
        .bind(execution)
        .bind("plan-1")
        .bind("import")
        .bind("local")
        .bind("compiled")
        .bind("running")
        .bind("Running")
        .bind("somebody")
        .bind(serde_json::json!([]))
        .bind(1_i64)
        .bind(OffsetDateTime::UNIX_EPOCH)
        .bind(None::<OffsetDateTime>)
        .bind(None::<OffsetDateTime>)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Read one back the way it read one.
async fn old_binary_reads(pool: &PgPool, execution: &str) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query(OLD_SELECT_PROJECTION)
        .bind(execution)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|row| row.get::<String, _>("state_name")))
}

/// Both halves, on a schema that has just been upgraded. This is the assertion
/// the whole file exists for: a rolling upgrade leaves the old process serving,
/// and a rollback puts it back.
async fn assert_the_old_binary_still_works(pool: &PgPool, from: &str) {
    let columns = columns_of(pool, "execution_runs").await;
    for column in ["started_at", "ended_at"] {
        assert!(
            columns.contains(&column.to_owned()),
            "upgrading from {from} left no `{column}`, which the previous release names in \
             both of its projection statements: {columns:?}"
        );
    }

    let execution = format!("old-binary-{from}");
    old_binary_writes(pool, &execution)
        .await
        .unwrap_or_else(|error| panic!("the old binary could not write after {from}: {error}"));
    let read = old_binary_reads(pool, &execution)
        .await
        .unwrap_or_else(|error| panic!("the old binary could not read after {from}: {error}"));
    assert_eq!(read.as_deref(), Some("Running"));
}

// ── The upgrade paths ────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "needs a PostgreSQL: just postgres-up"]
async fn a_database_at_schema_two_upgrades_to_one_the_previous_release_can_still_use() {
    // The longest path: 0003 drops the columns and 0005 puts them back, inside
    // one `apply`. Whether they were ever absent between two statements of one
    // transaction is not observable; whether they are absent afterwards is.
    let pool = database_at("aiwatcher_upgrade_from_two", 2).await;
    schema::apply(&pool).await.expect("an upgrade from 2");
    assert_the_old_binary_still_works(&pool, "schema 2").await;
    assert_eq!(applied_versions(&pool).await, APPLIED);
}

#[tokio::test]
#[ignore = "needs a PostgreSQL: just postgres-up"]
async fn a_database_at_schema_three_upgrades_to_one_the_previous_release_can_still_use() {
    let pool = database_at("aiwatcher_upgrade_from_three", 3).await;
    assert!(
        !columns_of(&pool, "execution_runs")
            .await
            .contains(&"started_at".to_owned()),
        "a database at 3 is one 0003 has already run, which is the state this repairs"
    );
    schema::apply(&pool).await.expect("an upgrade from 3");
    assert_the_old_binary_still_works(&pool, "schema 3").await;
    assert_eq!(applied_versions(&pool).await, APPLIED);
}

#[tokio::test]
#[ignore = "needs a PostgreSQL: just postgres-up"]
async fn reopening_a_database_already_at_schema_four_restores_the_columns_it_dropped() {
    // The case that rules out repairing this by editing 0003: version 4 is
    // recorded, so 3 is skipped forever after and its contents no longer reach
    // this database at all. Only a further version can put the columns back.
    let pool = database_at("aiwatcher_upgrade_from_four", 4).await;
    schema::apply(&pool).await.expect("an upgrade from 4");
    assert_the_old_binary_still_works(&pool, "schema 4").await;
    assert_eq!(applied_versions(&pool).await, APPLIED);
}

#[tokio::test]
#[ignore = "needs a PostgreSQL: just postgres-up"]
async fn a_fresh_database_ends_with_the_columns_the_previous_release_names() {
    let pool = database_at("aiwatcher_upgrade_from_nothing", 0).await;
    schema::apply(&pool).await.expect("a fresh schema");
    assert_the_old_binary_still_works(&pool, "an empty database").await;
    assert_eq!(applied_versions(&pool).await, APPLIED);
}

#[tokio::test]
#[ignore = "needs a PostgreSQL: just postgres-up"]
async fn applying_the_schema_again_after_an_upgrade_changes_nothing() {
    let pool = database_at("aiwatcher_upgrade_repeated", 3).await;
    schema::apply(&pool).await.expect("the first");
    let after_one = columns_of(&pool, "execution_runs").await;
    schema::apply(&pool).await.expect("the second");
    schema::apply(&pool).await.expect("a restarted pod");
    assert_eq!(applied_versions(&pool).await, APPLIED);
    assert_eq!(after_one, columns_of(&pool, "execution_runs").await);
}

// ── Both binaries against one database ───────────────────────────────────────

#[tokio::test]
#[ignore = "needs a PostgreSQL: just postgres-up"]
async fn a_row_the_new_code_wrote_is_one_the_previous_release_can_read() {
    // Rollback, and the second half of a rolling upgrade: the new binary has
    // been writing, and an old one is reading what it left. `started_at` comes
    // back NULL, which is what it was for every row that ever existed and what
    // the old `projection_from` already handled.
    let pool = database_at("aiwatcher_upgrade_rollback", 2).await;
    schema::apply(&pool).await.expect("an upgrade");

    let store = PostgresWorkflowStore::from_pool(pool.clone());
    assert_contract("postgres/upgraded", &store).await;

    let execution = "rollback-mixed";
    sqlx::query(
        "insert into execution_runs
           (execution_id, plan_id, definition_name, owner, mode, state_type,
            state_name, requested_by, steps, last_message_version,
            created_at, updated_at)
         values ($1, 'plan-1', 'import', 'local', 'compiled', 'running',
                 'Running', 'somebody', '[]', 1, $2, now())",
    )
    .bind(execution)
    .bind(OffsetDateTime::UNIX_EPOCH)
    .execute(&pool)
    .await
    .expect("the new code's own columns");

    assert_eq!(
        old_binary_reads(&pool, execution)
            .await
            .expect("a rolled-back binary reading a row the new one wrote"),
        Some("Running".to_owned())
    );

    // And it writes over it, as it would once it is the only version running.
    old_binary_writes(&pool, execution)
        .await
        .expect("a rolled-back binary writing over that row");

    // Then the new code reads the row the old one left.
    let projection = store
        .projection(&aiwatcher_execution::state::ExecutionId::new(
            execution.to_owned(),
        ))
        .await
        .expect("a read")
        .expect("the row is there");
    assert_eq!(projection.state.name, "Running");
}

// ── 0004, which is data rather than schema ───────────────────────────────────

#[tokio::test]
#[ignore = "needs a PostgreSQL: just postgres-up"]
async fn retiring_finished_attempts_keeps_the_one_that_is_awaiting_input() {
    // `awaiting_input` is absent from 0004's state list on purpose: it is not
    // an ending, and such an attempt resumes on the answer it asked for. An
    // upgrade that swept it away would strand every run waiting on a person.
    let pool = database_at("aiwatcher_upgrade_attempts", 3).await;
    for (step, state) in [
        ("done", "completed"),
        ("broke", "failed"),
        ("died", "crashed"),
        ("stopped", "cancelled"),
        ("asking", "awaiting_input"),
        ("going", "running"),
    ] {
        sqlx::query(
            "insert into step_attempts
               (execution_id, step_id, attempt, runtime, command_id, state)
             values ('upgrade', $1, 1, 'flow_php', $2, $3)",
        )
        .bind(step)
        .bind(format!("command-{step}"))
        .bind(state)
        .execute(&pool)
        .await
        .expect("an attempt row the old binary would have written");
    }

    schema::apply(&pool).await.expect("an upgrade past 0004");

    let mut left: Vec<String> = sqlx::query_scalar("select step_id from step_attempts order by 1")
        .fetch_all(&pool)
        .await
        .expect("what 0004 left");
    left.sort();
    assert_eq!(
        left,
        vec!["asking".to_owned(), "going".to_owned()],
        "0004 takes the four terminal states and nothing else"
    );

    // And the previous release's claim query still answers the same way: the
    // waiting attempt is excluded because it is waiting, not because it is gone.
    let claimable: Vec<String> = sqlx::query(OLD_CLAIM_ATTEMPT)
        .fetch_all(&pool)
        .await
        .expect("the old claim query against the upgraded schema")
        .iter()
        .map(|row| row.get::<String, _>("step_id"))
        .collect();
    assert_eq!(claimable, vec!["going".to_owned()]);
}
