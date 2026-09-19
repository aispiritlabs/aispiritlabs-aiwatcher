import { windowSearchSchema } from '@/shared/components/time-range';
import { objectFilterSchema } from '@/shared/lib/object-filter';
import { z } from 'zod';

/**
 * Metrics over the runs the projector still holds.
 *
 * Served from aiwatcher's own read model, not from a metrics backend: the page
 * renders with nothing else running, and the numbers are the same ones the runs
 * list is built from. The window is bounded by retention, which the header
 * states rather than hides.
 *
 * The filter is the shared one, so arriving from a table keeps the question,
 * and this route takes every axis of it: it selects the runs the numbers are
 * about and then narrows the call counters to the call the axis names. It took
 * three of them when the vocabulary was written, and the page named the rest as
 * unapplied — which was honest and is now unnecessary.
 */
export const searchSchema = z.object({
  ...windowSearchSchema,
  ...objectFilterSchema,
});
