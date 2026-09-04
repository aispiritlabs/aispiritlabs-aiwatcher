-- The execution schema of ADR_0025, section 9 of docs/PIPELINE_ARCHITECTURE.md.
--
-- Idempotent by construction: every statement is `IF NOT EXISTS`, and the whole
-- file runs inside one transaction under an advisory lock, so two replicas
-- starting together apply it once. What records that it ran is
-- `execution_schema_migrations`, which is what makes a *second* file possible
-- later without re-running this one.
--
-- Definitions are deliberately absent. They are content-addressed objects in
-- the registry's object store, beside the recipes and the dataset versions that
-- name them (ADR_0025, section 7). What lives here is what is *derived* from a
-- revision, plus the state of running it.

create table if not exists execution_plans (
    plan_id         text primary key,
    definition_kind text        not null,
    definition_name text        not null,
    revision        text        not null,
    plan            jsonb       not null,
    compiled_at     timestamptz not null default now(),
    unique (definition_kind, definition_name, revision)
);

-- One execution's ordered inputs and outputs. The primary key is the
-- compare-and-append: two deciders racing to write version N produce one row
-- and one unique violation, which the handler reads as "re-read and decide
-- again".
create table if not exists workflow_messages (
    execution_id   text        not null,
    stream_version bigint      not null,
    message_id     text        not null,
    kind           text        not null check (kind in ('command', 'event')),
    direction      text        not null check (direction in ('input', 'output')),
    message_type   text        not null,
    message        jsonb       not null,
    metadata       jsonb       not null,
    occurred_at    timestamptz not null,
    recorded_at    timestamptz not null default now(),
    primary key (execution_id, stream_version),
    -- The durable inbox, and permanent: re-deciding one input years later
    -- would still be wrong.
    unique (execution_id, message_id)
);

-- The inline projection. Rebuildable from the stream, written in the same
-- transaction as the decision that produced it — it exists to accept the
-- *next* command and to serve the run's own page, never a list the event log's
-- folds already serve (ADR_0026).
create table if not exists execution_runs (
    execution_id          text        primary key,
    plan_id               text        not null,
    definition_name       text        not null,
    owner                 text        not null,
    mode                  text        not null,
    state_type            text        not null,
    state_name            text        not null default '',
    requested_by          text        not null default '',
    steps                 jsonb       not null default '[]',
    last_message_version  bigint      not null default 0,
    created_at            timestamptz not null,
    started_at            timestamptz,
    ended_at              timestamptz
);

create index if not exists execution_runs_by_state
    on execution_runs (state_type, created_at desc);

-- The one place PostgreSQL is used as a queue, and bounded: one row per
-- attempt, one heartbeat per half-lease.
create table if not exists step_attempts (
    execution_id  text        not null,
    step_id       text        not null,
    attempt       integer     not null,
    runtime       text        not null,
    command_id    text        not null,
    queue         text,
    task_ref      text,
    state         text        not null,
    lease_owner   text,
    -- Who held it before. What a takeover asks the runtime about; see
    -- `AttemptRow::previous_owner`.
    previous_owner text,
    claimed_at    timestamptz,
    not_before    timestamptz,
    updated_at    timestamptz not null default now(),
    primary key (execution_id, step_id, attempt)
);

-- What `claim_attempt` scans. Partial, because a finished attempt is out of
-- every claimant's view and indexing it would grow the index with history.
-- `awaiting_input` is excluded for a different reason: it is not finished, and
-- it is still not claimable — it resumes on an answer, never on a timer.
create index if not exists step_attempts_claimable
    on step_attempts (queue, runtime, updated_at)
    where state not in ('completed', 'failed', 'crashed', 'cancelled', 'awaiting_input');

create table if not exists outbox_messages (
    message_id    text        primary key,
    execution_id  text        not null,
    event_type    text        not null,
    partition_key text        not null,
    payload       jsonb       not null,
    available_at  timestamptz not null default now(),
    attempts      integer     not null default 0,
    published_at  timestamptz,
    last_error    text
);

-- Partial for the same reason: the publisher only ever asks for what is
-- unpublished, and a table that keeps its history must not make that scan grow.
create index if not exists outbox_pending
    on outbox_messages (available_at)
    where published_at is null;

create table if not exists processor_checkpoints (
    processor_id text        primary key,
    checkpoint   text        not null,
    updated_at   timestamptz not null default now()
);
