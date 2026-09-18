import { z } from 'zod';

/**
 * Which organization and which project are being looked at.
 *
 * In the URL rather than in state, like every other selection in this panel:
 * "here is the grant I am asking you about" has to survive being sent to
 * somebody.
 */
export const searchSchema = z.object({
  organization: z.string().optional(),
  project: z.string().optional(),
});
