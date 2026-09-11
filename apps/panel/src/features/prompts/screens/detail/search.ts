import { z } from 'zod';

/**
 * One prompt: every version of it, and every optimisation run against it.
 *
 * ```text
 * head            labels, description, and the index of what is stored
 * ├── versions    immutable, content-addressed — the id is sha256(text)
 * └── optimisations   baseline → candidate, dev scores, held-out scores, a verdict
 * ```
 *
 * ## The verdict is not the optimiser's
 *
 * `outcome` on every row here was computed by the server from the held-out
 * scores and from what the candidate did to the baseline's variables. An
 * optimiser selected its candidate by maximising the number it then reports,
 * which makes it the last thing that should grade it — so the panel shows the
 * dev gain and the held-out gain side by side rather than a single score, and
 * flags the gap between them.
 *
 * ## Why the diff is a first-class view
 *
 * A rewritten prompt with a better score and no visible change behind it is the
 * thing people mean when they say they do not trust an optimiser. The diff is
 * how "it scored 0.07 higher" becomes "it added a sentence telling the model to
 * read the dimension lines" — see `lib/diff.ts`.
 */

export const searchSchema = z.object({
  /** The version shown in the pane. Defaults to whatever is current. */
  version: z.string().optional(),
  /** `text` or a diff against `against`. */
  view: z.enum(['text', 'diff']).optional(),
  /** The version the diff compares against. Defaults to the parent. */
  against: z.string().optional(),
  /** The optimisation whose report is expanded. */
  optimization: z.string().optional(),
});
