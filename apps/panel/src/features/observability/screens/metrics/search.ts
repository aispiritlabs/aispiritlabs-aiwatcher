import { windowSearchSchema } from '@/shared/components/time-range';
import { z } from 'zod';

/**
 * Metrics over the runs the projector still holds.
 *
 * Served from aiwatcher's own read model, not from a metrics backend: the page
 * renders with nothing else running, and the numbers are the same ones the runs
 * list is built from. The window is bounded by retention, which the header
 * states rather than hides.
 */

export const searchSchema = z.object({
  ...windowSearchSchema,
  agent_id: z.string().optional(),
  model: z.string().optional(),
});
