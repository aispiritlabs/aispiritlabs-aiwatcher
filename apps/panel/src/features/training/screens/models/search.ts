import { z } from 'zod';

/**
 * The model registry: what a training run produced, and which version a service
 * loads next.
 *
 * This page is the reason the training module lives in aiwatcher rather than in
 * Weights & Biases. A version names the export it was trained on and the run
 * that produced it; an agent span names a model. From a floor plan coming back
 * with bad geometry, the path back to the labelled images is two clicks and
 * never leaves one system.
 *
 * The refusal is the part worth reading. A version with no held-out
 * measurement, or one trained on a dataset name nobody can reconstruct, cannot
 * take a label — and the reason is shown on the version rather than as a
 * disabled button with no explanation.
 */

export const searchSchema = z.object({
  model: z.string().optional(),
  version: z.string().optional(),
});
