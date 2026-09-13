import { z } from 'zod';

import { windowSearchSchema } from '@/shared/components/time-range';

/**
 * Experiments: one context's variants side by side.
 *
 * `context` is the content address results share when they were measured on
 * the same cases the same way, and `baseline` the result every other row is
 * compared with. Both in the URL, so a link lands on the same comparison.
 * `window` is how far back what each variant was observed doing reaches; absent
 * is everything the log still holds, because the evidence beside it is kept
 * longer than any window.
 */
export const searchSchema = z.object({
  ...windowSearchSchema,
  context: z.string().optional(),
  baseline: z.string().optional(),
});
