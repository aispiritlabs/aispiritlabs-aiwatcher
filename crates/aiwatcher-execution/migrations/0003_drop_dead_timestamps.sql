-- Two columns nothing ever wrote.
--
-- Section 43.33. `execution_runs.started_at` and `ended_at` mirrored fields on
-- `RunProjection` that had no writer: `evolve` reads no clock and the terminal
-- events carry no timestamp, so both have been NULL in every row since the
-- table was created. They had no reader either.
--
-- Dropped rather than filled in, and the reason is not that filling them would
-- be expensive. ADR_0026 puts every managed run's facts on the event log under
-- the execution's id, and the workflow fold answers when a run started and
-- ended — with `duration_ms` beside them. Writing these would be a second
-- answer to a question something else answers better, which is the thing the
-- projection's own docstring says it is not for.
--
-- Safe by construction: every value is NULL, so nothing is lost. `if exists`
-- because a database created after this file will never have had them.

alter table execution_runs drop column if exists started_at;
alter table execution_runs drop column if exists ended_at;
