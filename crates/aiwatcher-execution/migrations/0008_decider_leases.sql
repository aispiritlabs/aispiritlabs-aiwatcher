-- One decider at a time for a hosted execution (section 40.3).
--
-- `agentic.workflow`'s `ProcessorLock`, in the store this system already keeps.
-- The plan said `execution_runs.lease_*`; this is a table of its own, for two
-- reasons that only appeared once the projection's upsert was read beside it.
--
-- The first is review R3's, one store along: `execution_runs` is rewritten
-- whole by every decision's projection, and configuration written by one party
-- and outcomes written by another do not belong in one row. The `do update set`
-- names its columns, so a lease *would* survive today — and the day somebody
-- adds a projection field and regenerates that statement, it silently would
-- not, and nothing would fail.
--
-- The second is that the three adapters have to mean the same thing. An
-- `update execution_runs` matches no row for an execution this store has never
-- seen, while `memory` and `file` would happily invent one; a table of its own
-- makes "take the lease" the same operation in all three, which is what the
-- contract suite can then prove about the rule rather than about PostgreSQL.
create table if not exists execution_decider_leases (
    execution_id     text        primary key,
    holder           text        not null,
    -- Who held it before, and only across a takeover. It cannot be read off
    -- `holder`: by the time a takeover is visible that column already names the
    -- new one. `AttemptRow::previous_owner`, for a whole run.
    previous_holder  text,
    claimed_at       timestamptz not null
);
