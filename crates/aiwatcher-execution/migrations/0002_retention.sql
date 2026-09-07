-- The retention clock, and the index a sweep scans.
--
-- Section 43.25. `WorkflowStore::prune` needs one timestamp per execution
-- meaning "when this store last wrote anything about it", and the other two
-- adapters read it from what they already hold — the last recorded message for
-- `memory`, the stream file's own modification time for `file`. Here that
-- answer would be `max(recorded_at)` over `workflow_messages`, a grouped scan
-- of the largest table in the schema to decide whether to delete from the
-- smallest. So it is a column, written by the same upsert that writes the
-- projection, in the same transaction as the decision behind it.
--
-- Deliberately not `ended_at`: a run that never got a terminal event has none,
-- and a retry has to reset the clock. See `store::prunable`.
--
-- `default now()` gives every execution already in the table an updated_at of
-- the upgrade, so turning retention on afterwards starts the window then rather
-- than deleting a year of history in the first sweep.

alter table execution_runs
    add column if not exists updated_at timestamptz not null default now();

-- What one sweep scans. Not partial, unlike the two partial indexes beside it,
-- and the reason is the query rather than the data: the terminal states are
-- bound from `StateType::TERMINAL` so that list lives in one place, and a
-- planner cannot prove a bound array implies a literal index predicate. A
-- composite index it can use is worth more than the rows a partial one saves.
create index if not exists execution_runs_prunable
    on execution_runs (state_type, updated_at);

-- The other half of the candidate test: an execution whose facts have not
-- reached the log yet is kept, whatever its age. The publisher's own index is
-- partial on `available_at` and answers a different question, so this one is by
-- execution.
create index if not exists outbox_unpublished_by_execution
    on outbox_messages (execution_id)
    where published_at is null;
