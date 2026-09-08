-- Where a scheduled slot is decided, transactionally (review R1, R2, R3).
--
-- The tick used to keep all of this on the schedule *object* in the object
-- store, which it read, worked from and wrote back whole. Three findings came
-- out of that one arrangement:
--
--   R3  an edit or a DELETE that landed between the read and the write was
--       overwritten by the snapshot. A deleted schedule came back enabled.
--   R1  `overlap = skip` was answered from an asynchronous read model that is
--       empty in the role the tick runs in, so skip never skipped.
--   R2  a transient failure was written down as `refused` and the global
--       cursor moved past the slot, so a store briefly unreachable at 09:00
--       cost the day's run.
--
-- Configuration stays where it was and the tick no longer writes it. What
-- moves here is the part that needs a transaction: taking a slot, checking the
-- definition for a run that has not finished, and recording what was decided.
--
-- `state` is null while somebody holds the lease and set once, terminally. The
-- lease is the claim table's, for the same reason — a tick that dies holding a
-- slot must not keep it — and `aiwatcher_jobs::LEASE_SECONDS` is the window.
--
-- Adding a table is compatible with the binary before this one by
-- construction: it names nothing here. That is the expand half of the rule
-- migration 0005 exists to state, and no second release is owed.

create table if not exists schedule_slots (
    definition_kind text        not null,
    definition_name text        not null,
    slot            timestamptz not null,
    -- 'started' | 'skipped' | 'refused'. Null while it is still being decided.
    state           text,
    execution_id    text,
    detail          text,
    lease_owner     text,
    leased_at       timestamptz,
    updated_at      timestamptz not null default now(),
    primary key (definition_kind, definition_name, slot)
);

-- What the panel reads: one definition's slots, newest first.
create index if not exists schedule_slots_by_definition
    on schedule_slots (definition_kind, definition_name, slot desc);
