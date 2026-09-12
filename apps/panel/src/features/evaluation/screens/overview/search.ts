import { z } from 'zod';

import { windowSearchSchema } from '@/shared/components/time-range';

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
 * ## Why the period control defaults to everything
 *
 * Half of this list is not folded from that log: durable evidence is kept on
 * purpose, under its own retention (ADR_0030). The catalogue had no order but
 * the hash of an evaluation ID, so a period would have narrowed nothing and
 * this screen carried no control at all; `evaluations/index/` gave it a
 * published order, and a period is a bound on that key.
 *
 * What stays different here is the default. Every other list defaults to a
 * day because everything on it goes when the log's retention takes it; the
 * kept half of this one exists *because* it outlives that, so a day would hide
 * the evidence the screen was built to show. It opens on everything and the
 * control narrows.
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
  /** The log-folded report open in the pane on the right. */
  report: z.string().optional(),
  /** The durable evidence open in the pane on the right, which is the other kind. */
  evidence: z.string().optional(),
  baseline: z.string().optional(),
  metrics: z.string().optional(),
});
