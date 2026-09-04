#![cfg(feature = "postgres")]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! The third adapter, against the same suite as the other two.
//!
//! `#[ignore]`, and run by `just test-postgres` after `just postgres-up` — the
//! pattern `just test-rustfs` and `just test-laser` already set. `just check`
//! does not start a database, and a test that silently passed because one was
//! missing would be worse than one that is skipped.
//!
//! Every property mints its own execution id and its own queue, so these run
//! against a database somebody else's run also used — which is the point:
//! `SKIP LOCKED`, the primary-key compare-and-append and the lease are all
//! things that only mean anything when more than one writer exists.

use std::sync::Arc;

use aiwatcher_execution::store::WorkflowStore;
use aiwatcher_execution::store::postgres::{PostgresWorkflowStore, connect};
use aiwatcher_execution::testing::{
    a_lost_claim_expires_and_the_next_claimant_takes_it_over,
    appending_the_same_decision_twice_is_idempotent, assert_contract, fresh,
    two_claimants_racing_for_one_attempt_produce_one_claim,
};

/// Where `just postgres-up` puts it.
fn url() -> String {
    std::env::var("AIWATCHER_WORKFLOW_POSTGRES_URL")
        .unwrap_or_else(|_| "postgres://aiwatcher:aiwatcher@127.0.0.1:5433/aiwatcher".to_owned())
}

async fn store() -> PostgresWorkflowStore {
    connect(&url(), 8)
        .await
        .expect("a database — run `just postgres-up` first")
}

#[tokio::test]
#[ignore = "needs a PostgreSQL: just postgres-up"]
async fn the_postgres_store_keeps_the_contract() {
    let store = store().await;
    assert_contract("postgres", &store).await;
    appending_the_same_decision_twice_is_idempotent("postgres", &store)
        .await
        .expect("an idempotent append");
}

#[tokio::test]
#[ignore = "needs a PostgreSQL: just postgres-up"]
async fn applying_the_schema_twice_changes_nothing() {
    // Two API replicas starting in the same second. The advisory lock is what
    // makes the second wait rather than race the first through `CREATE TABLE`,
    // and the recorded version is what makes it skip rather than re-apply.
    let one = connect(&url(), 2).await.expect("the first");
    let two = connect(&url(), 2).await.expect("the second");
    assert!(one.capabilities().multi_process);
    assert!(two.capabilities().multi_process);
}

#[tokio::test]
#[ignore = "needs a PostgreSQL: just postgres-up"]
async fn two_replicas_racing_for_one_attempt_produce_one_claim() {
    // The property the `file` adapter cannot have, run across two *pools*
    // rather than two calls: `SKIP LOCKED` is what makes the loser take a
    // different row instead of waiting on the winner's lock.
    let first = Arc::new(store().await);
    let second = Arc::new(store().await);
    two_claimants_racing_for_one_attempt_produce_one_claim("postgres/pool-a", first.as_ref()).await;
    two_claimants_racing_for_one_attempt_produce_one_claim("postgres/pool-b", second.as_ref())
        .await;
}

#[tokio::test]
#[ignore = "needs a PostgreSQL: just postgres-up"]
async fn a_worker_that_died_has_its_attempt_taken_over_by_another_replica() {
    let store = store().await;
    a_lost_claim_expires_and_the_next_claimant_takes_it_over("postgres", &store).await;
}

#[tokio::test]
#[ignore = "needs a PostgreSQL: just postgres-up"]
async fn a_stream_survives_the_process_that_wrote_it() {
    use aiwatcher_execution::testing as suite;

    let execution = fresh("restart");
    {
        let store = store().await;
        suite::a_stream_read_back_replays_to_the_state_it_recorded("postgres", &store).await;
    }
    // A second connection, as a restarted pod would make.
    let reopened = store().await;
    assert!(
        reopened
            .projection(&execution)
            .await
            .expect("a read")
            .is_none(),
        "a fresh execution id has no projection, which is what proves the read reaches the database"
    );
}
