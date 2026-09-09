-- Where a run's words live, on the run's own row.
--
-- The projection already carries `owner` and `mode` as columns because they are
-- what a reader asks about a run before anything else; this is the third
-- question of that kind, and the one whose answer somebody has an obligation
-- about. Whether a deployment is holding somebody's turns should be readable
-- off the run rather than inferred from a variable nobody looking at the panel
-- can see.
--
-- `external` for every row that already exists, which is what they were: the
-- policy did not exist when they were written, and nothing sealed anything.
alter table execution_runs
    add column if not exists payloads text not null default 'external';
