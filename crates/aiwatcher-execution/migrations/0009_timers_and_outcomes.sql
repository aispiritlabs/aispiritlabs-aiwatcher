-- Deferred appends, and an index for the receipt lookup.
--
-- ## Timers
--
-- The one *active* thing this engine does for a hosted run, and the smallest
-- possible version of it: a message the worker composed, stored until a time,
-- then appended. `agentic.workflow.Saga` already keeps its timeouts *in* its
-- stream and folds them to find what is due; that fold is the worker's and
-- stays the worker's. What it cannot do from there is notice — nothing wakes up
-- and looks. This is what one tick reads to find what is due across every
-- hosted execution without opening a single stream.
--
-- `message` is the whole `HostedMessage` the worker handed over, which is what
-- keeps "the engine does not interpret the messages" true: it stores one and
-- gives it back, and composes nothing.
create table if not exists execution_timers (
    execution_id text        not null,
    -- The worker's own id. Scheduling it twice is one timer, which is what
    -- makes a decider's retry safe.
    timer_id     text        not null,
    due_at       timestamptz not null,
    message      jsonb       not null,
    primary key (execution_id, timer_id)
);

-- What the tick scans. Every row is one that has not fired, because a fired
-- timer is deleted rather than marked — the shape `0004` settled for attempts,
-- for the same reason: nothing reads one back, and keeping it would make every
-- read of what is due filter out history.
create index if not exists execution_timers_due
    on execution_timers (due_at);

-- ## The receipt lookup
--
-- A worker whose report committed and whose HTTP reply was lost asks again, and
-- the answer is in the history: whether this attempt already recorded a
-- terminal event. That read loaded the *whole* execution stream — bounded by
-- nothing but retention, and the open performance item the worker protocol
-- review left behind.
--
-- Expression index rather than columns: the step and the attempt are inside the
-- message document, and lifting them out would be a second copy of two fields
-- the message already carries — free to disagree with it, and needing a
-- backfill to exist at all.
create index if not exists workflow_messages_outcomes
    on workflow_messages (
        execution_id,
        (message ->> 'step_id'),
        ((message ->> 'attempt')::int)
    )
    where message_type in ('step_completed', 'step_failed');
