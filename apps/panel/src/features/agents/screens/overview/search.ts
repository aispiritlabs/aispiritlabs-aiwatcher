import { windowSearchSchema } from '@/shared/components/time-range';
import { z } from 'zod';

/**
 * The agents this instance has seen, and what each has been doing.
 *
 * An agent is **not an authored object**. There is no registry of agents, no
 * version of one and nothing about one that outlives retention: an agent is a
 * key a run contributes to a fold (ADR_0007), and this list is that fold's
 * `agent` rows. So a page here never 404s on an unknown name — it says the
 * period holds nothing under it, which is a different and truer thing.
 *
 * It is an area of its own rather than a pivot of Explore because the pivot is
 * a *cell*: `?by=agent&key=…` answers "how do the agents compare", and a
 * reader arriving from a failing trace is asking "what does this one do, on
 * which models, at what cost" — which is a page.
 *
 * The list takes the period and a search and nothing else. The shared object
 * filter starts on the agent's own page, where the runs route can apply all of
 * it; here the one axis that would narrow anything is the agent, which is what
 * the rows already are.
 */
export const searchSchema = z.object({
  ...windowSearchSchema,
  /** Substring over the agent's name. Server-side, like every search here. */
  find: z.string().optional(),
});
