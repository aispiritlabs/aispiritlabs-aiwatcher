import { windowSearchSchema } from '@/shared/components/time-range';
import { z } from 'zod';

/**
 * Scoring the thing the traces come from.
 *
 * ```text
 * suite (on a dataset)  ── the level MLflow calls an experiment
 * └── report                = one execution: params in, metrics and a document out
 *     ├── cases             = one score each, with the rationale that produced it
 *     └── comparison        = against the previous report on the same dataset
 * ```
 *
 * Everything here is folded from the same event log the traces come from —
 * `eval.*` events produce no span and no row in the runs list. See
 * `crates/aiwatcher-projector/src/evaluations.rs`.
 *
 * ## Why the metric deltas are not coloured
 *
 * Higher is better for a pass rate and worse for a cost, and this page has no
 * way to know which a producer's metric is. Colouring them would mean guessing,
 * and a green number that means "we got more expensive" is worse than a plain
 * one. What *is* unambiguous is a case that passed on the baseline and fails
 * now, so that is the thing that gets a colour.
 */

export const searchSchema = z.object({
  ...windowSearchSchema,
  suite: z.string().optional(),
  dataset: z.string().optional(),
  status: z.enum(['running', 'succeeded', 'failed']).optional(),
  q: z.string().optional(),
  /** The report open in the pane on the right. */
  report: z.string().optional(),
  baseline: z.string().optional(),
  metrics: z.string().optional(),
});
