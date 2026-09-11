import { attributeSearchSchema } from '@/features/observability/lib/selection-params';
import { windowSearchSchema } from '@/shared/components/time-range';
import { z } from 'zod';

/**
 * Watching a selection of agents work, as it happens.
 *
 * ## What this is for
 *
 * Testing an agent is a different job from reading about one that finished.
 * You run it, and you want to see it: which tools it reached for, where it
 * stalled, whether the second agent picked up what the first handed over.
 * Explore answers that afterwards, from the read model; this answers it now,
 * from the log, and the difference is that a run in progress has no row in a
 * finished-runs table to look at.
 *
 * ## Why the server does the filtering
 *
 * `/api/v1/events/stream` takes the selection as repeated parameters and
 * `Scope::Selection` applies it before anything is sent. Subscribing to
 * everything and discarding most of it here would be the same mistake as
 * filtering a list after downloading it — and worse than usual, because
 * `llm.chunk` is most of the log by volume and every one of them would cross
 * the wire to be thrown away.
 *
 * ## Why some filters are refused rather than ignored
 *
 * Model and tool are span-level facts assembled from several events (ADR_0003)
 * and status is a fold over a whole run, so an event carries none of them. A
 * selection naming one arrives here from the Query builder, where it is
 * perfectly valid, and this page says which parts it cannot follow. Passing
 * them to a stream that does not understand them would look identical to
 * nothing happening.
 *
 * ## Why the buffer is bounded and the pause is a freeze
 *
 * A busy selection produces thousands of events a minute. The feed keeps the
 * last few hundred and the counters keep totals, so memory is flat however
 * long the tab is left open. Pause stops the *rendering*, not the
 * subscription: closing the connection would mean the resume had a gap in it,
 * and a live view with a silent hole is worse than one that fell behind.
 */

export const searchSchema = z.object({
  ...windowSearchSchema,
  ...attributeSearchSchema,
});
