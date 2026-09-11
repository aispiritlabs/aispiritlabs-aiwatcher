import { z } from 'zod';

/**
 * The prompts a system runs on, and what has been tried on them.
 *
 * The one area here that reads something other than the event log. Runs,
 * spans and evaluations are folds over a log with retention; a prompt is
 * authored, and the version a run used has to be readable long after that run
 * has been evicted. So it lives in an object store — RustFS in a deployment —
 * and this reads it over `/api/v1/prompts`.
 *
 * ## What the list is for
 *
 * Not "which prompts exist" — a repository answers that. It is "which prompts
 * have been optimised, and did any of it stick". So the row that carries the
 * most is the last optimisation: its dev gain beside its held-out gain, and
 * whether it was admitted. A registry of five prompts with twenty rejected
 * optimisations between them is a real finding, and it is one this page shows
 * without opening anything.
 */

export const searchSchema = z.object({
  q: z.string().optional(),
  tag: z.string().optional(),
});
