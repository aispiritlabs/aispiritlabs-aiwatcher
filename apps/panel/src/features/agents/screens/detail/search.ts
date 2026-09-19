import { compareSearchSchema } from '@/shared/components/period-compare';
import { windowSearchSchema } from '@/shared/components/time-range';
import { objectFilterSchema } from '@/shared/lib/object-filter';
import { z } from 'zod';

/**
 * One agent: what it ran on, what it called, what it cost, and its runs.
 *
 * The agent itself is the path, not a search parameter, because it is what
 * this page is about — and because it is then the one axis of the shared
 * filter nobody can contradict here. Everything else in `object-filter` is
 * accepted so a reader arriving from Explore under a workflow keeps the
 * workflow: the runs route takes every axis, so most of what arrives really
 * applies, and the page names what does not.
 *
 * `compare` is the period before this one, on the same filter — the metrics
 * page's control, because "is it slower than it was" is the question an
 * agent's page is opened with.
 */
export const searchSchema = z.object({
  ...windowSearchSchema,
  ...objectFilterSchema,
  ...compareSearchSchema,
});
