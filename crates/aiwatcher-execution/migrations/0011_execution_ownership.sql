-- Who owns one execution, and what they started.
--
-- Additive, and only additive: no existing table gains a column and no existing
-- statement stops working, so the release before this one runs unchanged
-- against a database this has been applied to. That is the rolling-upgrade rule
-- this schema keeps — a removal is two releases, and an addition costs the old
-- binary nothing, because a table it never names is a table it never reads.
--
-- What a rollback then leaves behind is an ownership row with no reader, and
-- that is the safe direction: the old binary treats every execution as global,
-- which is what it already believed. It is *not* safe to start project
-- executions on a deployment that may roll back, and nothing does yet — no
-- start path creates one (see the crate's `scope` module).
--
-- `scope_key` is `<organization>/<project>`, the same string
-- `ExecutionScope::key` builds, and the empty string is not stored: a global
-- execution has no row at all, which is what every stream written before this
-- migration is. That absence is the unscoped side, so no backfill is performed
-- and none is needed.
--
-- `record` is the whole `ExecutionOwnership` document. The columns beside it
-- are how it is found, not a second definition of it — the adapter's own rule
-- for every other table here.
create table if not exists execution_ownership (
    execution_id text        primary key,
    scope_key    text        not null,
    record       jsonb       not null,
    recorded_at  timestamptz not null default now()
);

-- Every scoped query asks one of two questions: "which executions are this
-- project's" (a claim, a timer, an outbox row, a retention pass) and "who owns
-- this one" (the primary key). Only the first needs an index.
create index if not exists execution_ownership_by_scope
    on execution_ownership (scope_key);
