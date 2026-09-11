import { z } from 'zod';

/**
 * What comes out of the archive: a job, and then an immutable corpus.
 *
 * The job is the point of this screen. An export of a real archive is minutes
 * of work, so it is queued rather than awaited, and what a reader needs while
 * it runs is not a spinner but the counts — how many turns it has considered,
 * how many it kept, and *why* it dropped the rest. An export that quietly
 * produced forty rows from four thousand turns looks exactly like one that
 * worked; the exclusion table is what turns that into "three thousand nine
 * hundred are still waiting for review".
 *
 * There is no progress bar for a queued job and there is one for a running one,
 * and the difference is honest: the conversation list is pinned when the job is
 * created, so the denominator is a fact rather than a guess. The training area
 * draws no bar for exactly the opposite reason — see `training.runs.tsx`.
 *
 * Polling stops when nothing is running, the same rule the training area keeps.
 */

export const searchSchema = z.object({
  corpus: z.string().optional(),
  version: z.string().optional(),
});
