import { windowSearchSchema } from '@/shared/components/time-range';
import { z } from 'zod';

/**
 * Filters live in the URL, not in component state. A run that looks wrong is
 * something people paste into a chat, and a link that does not carry the filter
 * lands the reader somewhere else.
 */
export const searchSchema = z.object({
  ...windowSearchSchema,
  status: z.enum(['running', 'succeeded', 'failed']).optional(),
  conversation_id: z.string().optional(),
  agent_id: z.string().optional(),
});
