import { z } from 'zod';

/**
 * The labelling workspace.
 *
 * Three columns, in the order attention moves: what to label, the plan, what
 * the shape says. The middle column is the only one that matters and gets the
 * space; the other two are lists.
 *
 * Two decisions here are worth stating, because both look like omissions.
 *
 * The draft lives in component state and is *not* written back on every
 * change. A revision is content-addressed and immutable, so autosaving every
 * vertex drag would mint a revision per mouse move. Save is explicit, and
 * saving the same drawing twice is one revision anyway.
 *
 * Validation is not repeated here. The registry refuses a bad drawing with one
 * line per problem, and the panel renders exactly those lines. A second
 * implementation in TypeScript would drift from the first, and the day it does
 * is the day somebody trusts the wrong one.
 */

export const searchSchema = z.object({
  project: z.string().optional(),
  image: z.string().optional(),
  review: z.enum(['draft', 'in_review', 'accepted', 'rejected']).optional(),
  split: z.enum(['train', 'validation', 'test']).optional(),
  q: z.string().optional(),
});
