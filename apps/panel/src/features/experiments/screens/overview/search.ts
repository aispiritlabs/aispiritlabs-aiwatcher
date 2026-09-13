import { z } from 'zod';

/**
 * Experiments: one context's variants side by side.
 *
 * `context` is the content address results share when they were measured on
 * the same cases the same way, and `baseline` the result every other row is
 * compared with. Both in the URL, so a link lands on the same comparison.
 */
export const searchSchema = z.object({
  context: z.string().optional(),
  baseline: z.string().optional(),
});
