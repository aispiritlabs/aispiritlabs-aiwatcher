import { attributeSearchSchema } from '@/features/observability/lib/selection-params';
import { windowSearchSchema } from '@/shared/components/time-range';
import { QUERY_ENGINES } from '@/shared/lib/query';
import { z } from 'zod';

/**
 * Questions asked of the same runs the explorer shows — clicked, or written.
 *
 * **Build** is the one somebody arrives at: attributes come from what has
 * actually run, the numbers are a checkbox each, and the text is compiled in
 * the deployed engine's language and shown as it goes. It exists because the
 * price of the editor was three pieces of trivia — that a run carries `agents`
 * as a list, that a span calls the same thing `agent_id`, that every column is
 * nullable — none of which is the question anybody came with.
 * **Write** is the one somebody graduates to, unchanged.
 *
 * The move between them is **one-way**, and the button says so. Build compiles
 * to text; text does not parse back into chips, because each language's parser
 * lives in its engine and a second one here would be free to rewrite a
 * hand-written query. So the two are never both the truth at once: in Build the
 * draft is and the text is derived, in Write the reverse.
 *
 * The builder **generates and refuses nothing**. Everything compiled here goes
 * through `/query/check` exactly as typed text does, and the diagnostics are the
 * engine's own — the same split the annotation and pipeline canvases make.
 *
 * The query engine is optional, so "not running" is a normal state with a
 * screen of its own rather than a failure.
 */

export const searchSchema = z.object({
  ...windowSearchSchema,
  ...attributeSearchSchema,
  mode: z.enum(['build', 'write']).optional(),
  /** The pipeline, in `write` mode. */
  q: z.string().optional(),
  /**
   * The engine `q` was written for (AW-3). A link without it is Flow's, so one
   * opened on a deployment running another engine is shown and not run.
   */
  writtenFor: z.enum(QUERY_ENGINES).optional(),
  grain: z.enum(['runs', 'spans']).optional(),
  group: z.array(z.string()).optional(),
  metric: z.array(z.string()).optional(),
  /** `runs:desc`. One parameter, because the two halves are never useful apart. */
  sort: z.string().optional(),
  limit: z.number().optional(),
});
