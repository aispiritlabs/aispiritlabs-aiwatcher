import { z } from 'zod';

/**
 * A corpus-sized import: the batch somebody staged, the job reading it, and
 * the rows it refused.
 *
 * The rejected rows are the reason this screen exists. An import of six
 * hundred thousand pictures that registered four hundred thousand of them
 * looks, from a success response, exactly like one that worked — and the two
 * hundred thousand it dropped are the whole story. So the counts come first,
 * grouped by reason, and the rows behind one reason are a click away.
 *
 * There *is* a progress bar here, unlike Training and like Conversations, and
 * for the same reason: the pages were counted when the batch was sealed, so
 * the denominator is a fact rather than a guess.
 *
 * Polling stops when nothing is moving, the same rule every other job view
 * keeps. See ADR_0022.
 */

export const searchSchema = z.object({
  job: z.string().optional(),
});
