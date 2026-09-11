import { z } from 'zod';

export const searchSchema = z.object({
  run: z.string().optional(),
  model: z.string().optional(),
  status: z.enum(['running', 'succeeded', 'failed', 'cancelled']).optional(),
  dataset: z.string().optional(),
  metrics: z.string().optional(),
  normalise: z.boolean().optional(),
});
