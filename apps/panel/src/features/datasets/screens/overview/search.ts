import { z } from 'zod';

export const searchSchema = z.object({
  window: z.number().int().nonnegative().optional(),
  dataset: z.string().optional(),
  version: z.string().optional(),
  view: z.enum(['rows', 'evaluations', 'lineage', 'promote', 'discover']).optional(),
  q: z.string().optional(),
  /** Which annotation project a hub import lands in. In the URL so a link
   *  to a half-configured import is a link somebody else can finish. */
  project: z.string().optional(),
});
