import { z } from 'zod';

/**
 * Freezing a project into something a training run can name.
 *
 * The reference is `project@export-sha256`, and it is the same shape as
 * `dataset@version` in ADR_0015 for the same reason: a name alone is mutable,
 * and a training run that records only a name cannot prove what it was trained
 * on. Two exports of an unchanged project are one export, so building this
 * nightly costs nothing.
 *
 * The exclusion table is the part people skip and should not. An export that
 * quietly loses a third of a corpus reads exactly like one that did not, so
 * every image left out is listed with its reason.
 */

export const searchSchema = z.object({
  project: z.string().optional(),
  export: z.string().optional(),
});
