-- A finished attempt is not a row (section 43.34).
--
-- Until now a completion overwrote its attempt with a terminal one, whose
-- command id, queue and task ref all had to be blanked to store it. The rows
-- described nothing, were excluded from `step_attempts_claimable` by that
-- index being partial, and were deleted only when retention took the whole
-- execution — which is off unless a deployment asks for it.
--
-- The states are written out rather than taken from `StateType::TERMINAL`
-- because a migration is a one-time backfill of what the code now maintains,
-- not a second copy of the rule. `awaiting_input` is absent on purpose: it is
-- not an ending, and such an attempt resumes on the answer it asked for.
delete from step_attempts
 where state in ('completed', 'failed', 'crashed', 'cancelled');
