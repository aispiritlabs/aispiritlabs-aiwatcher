import { z } from 'zod';

/**
 * Where the images come from.
 *
 * A dated table an instance was configured with, not a search against Hugging
 * Face, Kaggle or Roboflow Universe. Those mirrors restate licences wrongly
 * often enough that a live answer would be worse than none: it would arrive
 * looking authoritative. Every row links its original and says when somebody
 * last read the licence there.
 *
 * This build ships no rows — which corpora exist and what their licences
 * permit is a question about one field, and a list shipped here would be one
 * project's homework. Empty is a working state: nothing outranks a mirror's
 * claim, so every hub result stays `unclear`.
 *
 * The filter that matters is the first one. "What may a commercial model be
 * trained on" is a question with an expensive wrong answer, and it should be
 * one click rather than an afternoon of reading licence files.
 */

export const searchSchema = z.object({
  q: z.string().optional(),
  usage: z.enum(['commercial', 'non_commercial', 'unclear']).optional(),
  label: z.string().optional(),
  project: z.string().optional(),
});
