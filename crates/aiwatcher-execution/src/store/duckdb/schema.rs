//! The tables, and the one rule about changing them.
//!
//! Applied at `open`, every time, with `create table if not exists`. There is
//! no version table and no migration list here, and that is a decision rather
//! than an omission: the PostgreSQL adapter needs both because two API replicas
//! start in the same second against one database and an image rollback runs the
//! old binary against the new schema. Neither is true of a file one process
//! holds. What *is* true is the other half of that guardrail — a release may
//! not remove what the release before it names — and here it has teeth for a
//! different reason: this database is also the thing `aiwatcher sql` opens, so
//! a column somebody has a query for is a column with a reader outside this
//! crate.
//!
//! Every table keeps the whole row as JSON in `payload` and lifts out only what
//! is indexed or ordered by. The payload is the truth; the columns are how it
//! is found. That is what keeps this adapter from becoming a second definition
//! of the domain types — a field added to `AttemptRow` needs no change here.

use duckdb::Connection;

use crate::error::{Result, StoreError};

/// Bring a database up to the shape this build expects.
///
/// # Errors
///
/// [`StoreError::Backend`] naming what the database refused.
pub fn apply(connection: &Connection) -> Result<()> {
    connection
        .execute_batch(SCHEMA)
        .map_err(|error| StoreError::Backend(format!("applying the schema: {error}")))
}

/// Idempotent, so it runs at every `open`.
const SCHEMA: &str = "
create table if not exists streams (
    execution   varchar not null,
    version     bigint  not null,
    direction   varchar not null,
    -- Microseconds since the epoch. See `stamp`: what is compared has to be
    -- exactly what was written, and a round trip through a database's own
    -- temporal type is a second representation with its own precision.
    recorded_at bigint  not null,
    payload     varchar not null,
    primary key (execution, version)
);

-- The durable inbox: permanent, unlike a processor's own dedup window, because
-- re-deciding one input years later would still be wrong.
create table if not exists inbox (
    execution  varchar not null,
    message_id varchar not null,
    version    bigint  not null,
    primary key (execution, message_id)
);

-- `definition_name` and `terminal` are lifted out because `admit_slot` asks
-- exactly that question — is anything still running for this definition — in
-- the transaction that takes the slot.
create table if not exists projections (
    execution       varchar primary key,
    definition_name varchar not null,
    terminal        boolean not null,
    payload         varchar not null
);

-- A row here is a fact that has not reached the log yet. `mark_published`
-- deletes rather than flags, so this table is short by construction and its
-- sequence is only ever used for ordering.
create sequence if not exists outbox_sequence start 1;
create table if not exists outbox (
    sequence      bigint primary key,
    message_id    varchar not null,
    partition_key varchar not null,
    payload       varchar not null
);

create table if not exists checkpoints (
    processor varchar primary key,
    payload   varchar not null
);

-- Live attempts only: a settlement is the row ceasing to exist, so this stays
-- bounded by concurrency rather than by retention. `dispatched_at` is the
-- ordering `claim_attempt` reads oldest-first, which is what stops a backlog
-- being served newest-first while its head starves.
create table if not exists attempts (
    execution     varchar not null,
    step          varchar not null,
    attempt       bigint  not null,
    dispatched_at bigint  not null,
    payload       varchar not null,
    primary key (execution, step, attempt)
);

-- The key is its encoded form, so one lookup is one comparison; the three
-- parts beside it are what `recent_slots` filters and orders by.
create table if not exists slots (
    key             varchar primary key,
    definition_kind varchar not null,
    definition_name varchar not null,
    slot            bigint  not null,
    payload         varchar not null
);

-- Kept after it expires, so a takeover can name who was interrupted.
create table if not exists decider_leases (
    execution varchar primary key,
    payload   varchar not null
);
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applying_the_schema_twice_changes_nothing() {
        // `open` applies it every time, so this is the ordinary path rather
        // than an edge case: the second start of a local instance runs it
        // against the database the first one wrote.
        let connection = Connection::open_in_memory().expect("a database");
        apply(&connection).expect("the first");
        apply(&connection).expect("the second");
    }

    #[test]
    fn the_outbox_sequence_survives_a_second_apply() {
        // `create sequence if not exists` rather than `create sequence`: the
        // failure this prevents is a second `open` erroring out on a database
        // that is already correct.
        let connection = Connection::open_in_memory().expect("a database");
        apply(&connection).expect("the first");
        connection
            .execute_batch("select nextval('outbox_sequence')")
            .expect("the sequence exists");
        apply(&connection).expect("the second");
        connection
            .execute_batch("select nextval('outbox_sequence')")
            .expect("the sequence still exists");
    }
}
