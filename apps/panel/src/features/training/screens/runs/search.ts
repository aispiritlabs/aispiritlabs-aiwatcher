import { z } from 'zod';

export const searchSchema = z.object({
  run: z.string().optional(),
  runs: z
    .array(z.string().min(1))
    .optional()
    .transform((ids) => (ids ? [...new Set(ids)].slice(0, 5) : undefined)),
  model: z.string().optional(),
  status: z.enum(['running', 'succeeded', 'failed', 'cancelled']).optional(),
  dataset: z.string().optional(),
  metrics: z.string().optional(),
  normalise: z.boolean().optional(),
});
