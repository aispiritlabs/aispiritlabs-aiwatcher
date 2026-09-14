import { z } from 'zod';

/**
 * What the run page holds in the URL.
 *
 * The selected span belongs here for the reason every other filter in this
 * panel does: a person who has found the call that went wrong sends the link,
 * and the link has to land the next reader on that call rather than on the top
 * of the trace.
 *
 * The two view flags are the same decision from the other side. This page's
 * promise is that nothing is hidden, so what they turn on is *more* — every
 * attribute including the correlation ids, and the token chunks a streaming
 * call fills the feed with. Defaults are the readable view, and a reader who
 * wants the wire gets it in one click and can send that too.
 */
export const searchSchema = z.object({
  /** The span opened beside the waterfall. */
  span: z.string().optional(),
  /** Show every span attribute, correlation ids included. */
  attrs: z.boolean().optional(),
  /** Show `llm.chunk` in the event feed. */
  chunks: z.boolean().optional(),
  /** Open what the content archive holds for each call, without a click each. */
  content: z.boolean().optional(),
});

export type RunSearch = z.infer<typeof searchSchema>;
