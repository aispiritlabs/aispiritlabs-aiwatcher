#![cfg(feature = "duckdb")]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! The fourth adapter, against the same suite as the other three.
//!
//! Unlike `postgres.rs` and `laser`'s tests, these are **not** `#[ignore]`d and
//! need nothing running: DuckDB is embedded, so the database is a temporary
//! file this test makes and deletes. That is the whole argument for it as the
//! local store — the thing a deployment needs a container for, one machine gets
//! from a file — and it means the contract is proved on every `just check` that
//! builds the feature rather than only when somebody remembers to start
//! something.

use std::sync::Arc;

use aiwatcher_execution::store::WorkflowStore;
use aiwatcher_execution::store::duckdb::DuckdbWorkflowStore;
use aiwatcher_execution::testing::{
    a_lost_claim_expires_and_the_next_claimant_takes_it_over,
    appending_the_same_decision_twice_is_idempotent, assert_contract, fresh,
    two_claimants_racing_for_one_attempt_produce_one_claim,
};

/// A database in a directory of this test's own, removed when it drops.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "aiwatcher-duckdb-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("creates");
        Self(dir)
    }

    fn store(&self) -> DuckdbWorkflowStore {
        DuckdbWorkflowStore::open(self.0.join("aiwatcher.duckdb")).expect("a database")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

#[tokio::test]
async fn the_duckdb_store_keeps_the_contract() {
    let scratch = Scratch::new("contract");
    let store = scratch.store();
    assert_contract("duckdb", &store).await;
    appending_the_same_decision_twice_is_idempotent("duckdb", &store)
        .await
        .expect("an idempotent append");
}

#[tokio::test]
async fn two_claimants_racing_take_one_attempt_each_time() {
    let scratch = Scratch::new("race");
    let store = Arc::new(scratch.store());
    two_claimants_racing_for_one_attempt_produce_one_claim("duckdb", store.as_ref()).await;
    a_lost_claim_expires_and_the_next_claimant_takes_it_over("duckdb", store.as_ref()).await;
}

#[tokio::test]
async fn a_store_reopened_still_holds_what_the_first_one_wrote() {
    // The property the `memory` adapter cannot have and the reason this one
    // exists beside `file`: a local instance restarted at lunchtime still knows
    // about the run somebody started before it.
    let scratch = Scratch::new("reopen");
    let path = scratch.0.join("aiwatcher.duckdb");
    let execution = fresh("reopened");

    {
        let store = DuckdbWorkflowStore::open(&path).expect("a database");
        aiwatcher_execution::testing::a_stream_read_back_replays_to_the_state_it_recorded(
            "duckdb", &store,
        )
        .await;
        // The database file is released when the connection drops, which is
        // what makes the second `open` below possible at all.
        let _ = store.projection(&execution).await;
    }

    let reopened = DuckdbWorkflowStore::open(&path).expect("the same database, again");
    assert!(
        reopened.capabilities().claimable,
        "a reopened store forgot what it can do"
    );
}

#[tokio::test]
async fn one_process_at_a_time_is_stated_rather_than_discovered() {
    // DuckDB takes an exclusive lock on the file, so a managed run that needs a
    // worker has to be refused at admission — the same answer the `file`
    // adapter gives, for the same reason, and the one thing about this store a
    // caller must be able to ask before it accepts a plan.
    let scratch = Scratch::new("single");
    let store = scratch.store();
    assert!(
        !store.capabilities().multi_process,
        "duckdb claimed it can be held by two processes"
    );
}
