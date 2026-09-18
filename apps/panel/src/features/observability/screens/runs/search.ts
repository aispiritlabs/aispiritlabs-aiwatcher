import { windowSearchSchema } from '@/shared/components/time-range';
import { objectFilterSchema } from '@/shared/lib/object-filter';
import { z } from 'zod';

/**
 * Filters live in the URL, not in component state. A run that looks wrong is
 * something people paste into a chat, and a link that does not carry the filter
 * lands the reader somewhere else.
 *
 * The axes are the shared ones (`shared/lib/object-filter.ts`), so this list is
 * the vocabulary and not a list of its own. Before it, this route declared
 * three of the nine the runs route accepts, under the API's parameter names
 * rather than the dimensions' — so "the failed runs of this workflow" was a
 * question Explore could ask and this page could not, and a link from one to
 * the other lost the half the reader had already answered.
 */
export const searchSchema = z.object({
  ...windowSearchSchema,
  ...objectFilterSchema,
});
