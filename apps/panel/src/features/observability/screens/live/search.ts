import { attributeSearchSchema } from '@/features/observability/lib/selection-params';
import { windowSearchSchema } from '@/shared/components/time-range';
import { z } from 'zod';

/**
 * Watching a selection of agents work, as it happens.
 *
 * Explore answers from the read model once a run has finished; this answers
 * from the log while it runs, when there is no finished-runs row to look at —
 * which tools an agent reached for, where it stalled, whether the second agent
 * picked up what the first handed over.
 *
 * The server filters: `/api/v1/events/stream` takes the selection as repeated
 * parameters and `Scope::Selection` applies it before anything is sent, because
 * `llm.chunk` is most of the log by volume and filtering here would pull every
 * one of them across the wire to be thrown away.
 *
 * Model and tool are span-level facts assembled from several events (ADR_0003)
 * and status is a fold over a whole run, so no event carries them. A selection
 * from the Query builder may validly name them, and this page says which parts
 * it cannot follow — passing them to a stream that does not understand them
 * would look identical to nothing happening.
 *
 * The feed keeps the last few hundred events and the counters keep totals, so
 * memory is flat however long the tab stays open. Pause stops the *rendering*,
 * never the subscription: closing the connection would leave a silent gap on
 * resume, which is worse than a live view that fell behind.
 */

export const searchSchema = z.object({
  ...windowSearchSchema,
  ...attributeSearchSchema,
});
