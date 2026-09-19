import { z } from 'zod';

/**
 * Which rule's deliveries are being read, and in which state.
 *
 * Both live in the URL rather than in component state, like every other filter
 * here, so "the three notifications that failed last night" is a link somebody
 * can paste into a ticket.
 */
export const searchSchema = z.object({
  rule: z.string().optional(),
  state: z.enum(['queued', 'running', 'completed', 'failed', 'cancelled']).optional(),
});

export type AlertSearch = z.infer<typeof searchSchema>;
