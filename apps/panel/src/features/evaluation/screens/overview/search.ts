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
 * Every other list defaults to a day, because everything on it goes when the
 * log's retention takes it. Half of this one is kept on purpose *because* it
 * outlives that (ADR_0030), so a day would hide what that half is for. It
 * opens on everything and the control narrows.
 *
 * ## Why a delta is coloured on one half and not on the other
 *
 * A folded report's metric is a name a producer sent, and higher is better for
 * a pass rate and worse for a cost, so colouring one would mean guessing and a
 * green number could read as "we got more expensive". A pinned context
 * *declares* the direction per metric, which makes published evidence the one
 * place a delta may be coloured — and plain where it declares `none`.
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
  /** Whether the operator's half — which pairs this instance admits — is open. */
  approvals: z.boolean().optional(),
  baseline: z.string().optional(),
  /**
   * The published result the open evidence is compared with.
   *
   * Not `baseline`: that one is the folded half's, where blank means "the
   * previous success" and the server picks. Published evidence has no
   * automatic baseline — the catalogue narrowed to one context is what offers
   * candidates — so blank there means nothing is being compared, and one name
   * for two rules would be the control that quietly does something else.
   */
  compare: z.string().optional(),
  /**
   * Which cases the open comparison is showing, and whether it is showing any.
   *
   * Absent means closed, which is the default because the diff is a full read
   * of both results where everything else on the screen is a read of two
   * headers. `all` is the fourth value for the same reason `worse` is the
   * first: absent already means something, so "open with nothing filtered out"
   * needs a word of its own.
   */
  cases: z.enum(['worse', 'better', 'changed', 'all']).optional(),
  metrics: z.string().optional(),
});
