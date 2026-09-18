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
 * The filter is the shared one, so arriving from a table keeps the question.
 * The metrics route takes three of its nine axes today and the page names the
 * six it could not apply — the alternative, a control that silently narrows
 * here and not there, is what this vocabulary exists to end.
 */
export const searchSchema = z.object({
  ...windowSearchSchema,
  ...objectFilterSchema,
});
