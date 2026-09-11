import { QUERY_ENGINES } from '@/shared/lib/query';
import { z } from 'zod';

export const searchSchema = z.object({
  q: z.string().optional(),
  /** The engine `q` was written for (AW-3); a link without it is Flow's. */
  writtenFor: z.enum(QUERY_ENGINES).optional(),
  name: z.string().optional(),
  dataset: z.string().optional(),
  window: z.number().int().nonnegative().optional(),
});
