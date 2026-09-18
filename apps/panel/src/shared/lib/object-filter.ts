import { z } from 'zod';

import type { RunStatus } from '@/api/generated/types.gen';

/**
 * One filter, every table and every chart.
 *
 * The panel had two filter vocabularies. Live and Query spoke the dimensions'
 * own names — `agent`, `runtime`, `workflow`, `session` — because that is what
 * `GET /api/v1/events/stream` takes and what the query engines' catalog is
 * named after. Runs and Metrics spoke the read model's parameter names —
 * `agent_id`, `conversation_id` — and between them offered four of the nine
 * axes the routes actually accept. So "the failed runs of this workflow" was
 * expressible in one view and not in the next, and moving between them dropped
 * the half of the question the reader had already answered.
 *
 * This is the vocabulary, and it is the dimensions' (ADR_0007) plus the run's
 * status. Five rules carry it; `docs/flow-01-inventory-2026-09-18.md` argues
 * them.
 *
 * 1. **One name per axis**, the same in every URL. Translating an axis to a
 *    route's parameter happens here and nowhere else.
 * 2. **A filter selects runs.** Every axis names a property a run has —
 *    directly, or through its spans, which is how `RunFilter` already matches
 *    `model` and `tool`. Axes are conjunctive.
 * 3. **A number is counted over the selected runs**, and narrowed further only
 *    where the axis is about the thing being counted: with a model chosen, LLM
 *    calls and tokens are that model's, while tool calls are every tool call in
 *    those runs, because a tool call has no model.
 * 4. **A view that cannot apply an axis says so** rather than dropping it.
 *    [`queryFor`] returns what it could not send and why, in the words the page
 *    renders. The Live view has done this since it existed; this is the same
 *    mechanism for the rest.
 * 5. **It lives in the URL**, and it survives a move between the area's views,
 *    the way the time window already does.
 */

/**
 * The axes, in the order a filter bar reads them: what ran, then what it ran
 * on, then how it went.
 */
export const OBJECT_AXES = [
  'agent',
  'runtime',
  'workflow',
  'session',
  'variant',
  'trace',
  'model',
  'tool',
  'status',
] as const;

export type ObjectAxis = (typeof OBJECT_AXES)[number];

/**
 * What is chosen, per axis.
 *
 * Values are a list because the live stream takes repeated parameters and a
 * query engine composes an `in`. Every read-model route takes one value per
 * axis, so those views send a single value and *say* they narrowed to one —
 * see [`queryFor`]. Taking the first quietly would answer a different question
 * than the one on screen.
 */
export type ObjectFilter = Partial<Record<ObjectAxis, string[]>>;

/**
 * One axis in the URL: a list, or a bare value read as a list of one.
 *
 * The bare form is not a convenience. `?model=gpt-4o` is what somebody types
 * and what every link written before this vocabulary existed already says —
 * `?status=failed` is in the command palette, in saved pins and in whatever
 * chat somebody pasted it into — and a route that threw on it would turn an
 * old link into an error page rather than into the view it names.
 */
const values = z
  .union([z.string(), z.array(z.string())])
  .optional()
  .transform((value) => (typeof value === 'string' ? [value] : value));

/** Merge into a route's search schema to give it the filter. */
export const objectFilterSchema = {
  agent: values,
  runtime: values,
  workflow: values,
  session: values,
  variant: values,
  trace: values,
  model: values,
  tool: values,
  status: values,
};

export type ObjectFilterSearch = z.infer<z.ZodObject<typeof objectFilterSchema>>;

export function filterFromSearch(search: ObjectFilterSearch): ObjectFilter {
  const filter: ObjectFilter = {};
  for (const axis of OBJECT_AXES) {
    const chosen = search[axis]?.filter(Boolean);
    if (chosen && chosen.length > 0) filter[axis] = chosen;
  }
  return filter;
}

/**
 * The search patch a filter is.
 *
 * Every axis is written, including the cleared ones as `undefined`: leaving one
 * out of the patch merges the old value back in, and the chip comes off the
 * screen while the filter stays on the request.
 */
export function filterToSearch(filter: ObjectFilter): ObjectFilterSearch {
  return Object.fromEntries(
    OBJECT_AXES.map((axis) => {
      const chosen = (filter[axis] ?? []).filter(Boolean);
      return [axis, chosen.length > 0 ? chosen : undefined];
    }),
  );
}

export function isEmpty(filter: ObjectFilter): boolean {
  return OBJECT_AXES.every((axis) => (filter[axis] ?? []).length === 0);
}

/** One axis, as a single value — `undefined` when it is unset or ambiguous. */
export function only(filter: ObjectFilter, axis: ObjectAxis): string | undefined {
  const chosen = filter[axis] ?? [];
  return chosen.length === 1 ? chosen[0] : undefined;
}

/** Add one value to an axis, or take it off if it is already there. */
export function toggle(filter: ObjectFilter, axis: ObjectAxis, value: string): ObjectFilter {
  const chosen = filter[axis] ?? [];
  const next = chosen.includes(value)
    ? chosen.filter((held) => held !== value)
    : [...chosen, value];
  return { ...filter, [axis]: next };
}

// ── What each route can take ─────────────────────────────────────────────────

/** A read the filter is translated for. */
export type FilterTarget = 'runs' | 'spans' | 'dimensions' | 'metrics';

/**
 * Axis → the route's parameter, or why it has none.
 *
 * `null` is "this route has no parameter for it"; a string is the parameter's
 * name. The sentences for the nulls are in [`WHY_NOT`] rather than here, so
 * that adding a parameter server-side is one edit in one table.
 */
const PARAMETER: Record<FilterTarget, Record<ObjectAxis, string | null>> = {
  runs: {
    agent: 'agent_id',
    runtime: 'runtime',
    workflow: 'workflow',
    session: 'conversation_id',
    variant: 'variant_id',
    trace: 'trace_id',
    model: 'model',
    tool: 'tool',
    status: 'status',
  },
  spans: {
    agent: 'agent_id',
    runtime: null,
    workflow: null,
    session: null,
    variant: null,
    trace: 'trace_id',
    model: 'model',
    tool: 'tool',
    // Deliberately not `status`: the span route's is `ok | error`, which is a
    // span's outcome and not the run's. A failed span inside a run that
    // succeeded is an ordinary thing, so sharing one word would make the
    // filter lie about what it selected.
    status: null,
  },
  dimensions: {
    agent: 'agent_id',
    runtime: null,
    workflow: null,
    session: null,
    variant: null,
    trace: null,
    model: null,
    tool: null,
    status: null,
  },
  metrics: {
    agent: 'agent_id',
    runtime: null,
    workflow: null,
    session: 'conversation_id',
    variant: null,
    trace: null,
    model: 'model',
    tool: null,
    status: null,
  },
};

/**
 * Why a route cannot answer an axis, in the words the page renders.
 *
 * A fact about the read model, not an apology: `/spans` has no session because
 * a span does not name one, while `/metrics` has no runtime because nobody has
 * added the parameter yet. A reader deciding whether to switch views needs to
 * know which of the two they are looking at.
 */
const WHY_NOT: Partial<Record<FilterTarget, Partial<Record<ObjectAxis, string>>>> = {
  spans: {
    runtime: 'a span does not name the service that produced it',
    workflow: 'a span does not name its workflow',
    session: 'a span does not name its session',
    variant: 'a span list is not narrowed by variant',
    status: 'a span carries its own outcome, not the run’s status',
  },
  dimensions: {
    runtime: 'the dimension route narrows by agent alone',
    workflow: 'the dimension route narrows by agent alone',
    session: 'the dimension route narrows by agent alone',
    variant: 'the dimension route narrows by agent alone',
    trace: 'the dimension route narrows by agent alone',
    model: 'the dimension route narrows by agent alone',
    tool: 'the dimension route narrows by agent alone',
    status: 'the dimension route narrows by agent alone',
  },
  metrics: {
    runtime: 'the metrics route takes agent, session and model',
    workflow: 'the metrics route takes agent, session and model',
    variant: 'the metrics route takes agent, session and model',
    trace: 'the metrics route takes agent, session and model',
    tool: 'the metrics route takes agent, session and model',
    status: 'the metrics route takes agent, session and model',
  },
};

/** One axis a read could not carry, and why. */
export interface Unapplied {
  axis: ObjectAxis;
  /** A sentence, already in the reader's words. */
  why: string;
}

/**
 * What each target's query looks like once the axes are translated.
 *
 * Written out rather than inferred so that a call site keeps the generated
 * client's own types: spreading a `Record<string, string>` into a request
 * would type `status` as a string, and the one axis with a closed vocabulary
 * is the one worth keeping closed.
 */
export interface TargetQuery {
  runs: {
    agent_id?: string;
    runtime?: string;
    workflow?: string;
    conversation_id?: string;
    variant_id?: string;
    trace_id?: string;
    model?: string;
    tool?: string;
    status?: RunStatus;
  };
  spans: { agent_id?: string; trace_id?: string; model?: string; tool?: string };
  dimensions: { agent_id?: string };
  metrics: { agent_id?: string; conversation_id?: string; model?: string };
}

export interface TranslatedFilter<T extends FilterTarget> {
  /** Ready to spread into the generated client's `query`. */
  query: TargetQuery[T];
  /** Empty when everything chosen was sent. */
  unapplied: Unapplied[];
  /**
   * What the axes that *were* sent do to this read's numbers.
   *
   * Sentences rather than flags, and produced here rather than on the page,
   * because they differ per route and a page repeating one from memory is a
   * page free to keep saying it after the route stops doing it.
   */
  notes: string[];
}

const RUN_STATUSES: readonly string[] = ['running', 'succeeded', 'failed'];

/**
 * The filter, as one route's query — and what that route could not take.
 *
 * Nothing is guessed and nothing is halved: an axis with two values on a route
 * that takes one is *not sent*, and is reported instead. Sending the first
 * would put a narrower answer on the screen than the chips claim, which is the
 * one failure a filter must not have.
 *
 * A status the run states have no word for is reported the same way rather
 * than forwarded. Everything else in a URL is somebody's id and the server is
 * the only thing that can say whether it exists; `status` is the one axis with
 * a closed vocabulary, so a typo in a pasted link becomes a sentence here
 * instead of a 400 from serde and an error page.
 */
export function queryFor<T extends FilterTarget>(
  target: T,
  filter: ObjectFilter,
): TranslatedFilter<T> {
  const query: Record<string, string> = {};
  const unapplied: Unapplied[] = [];
  const applied = new Set<ObjectAxis>();

  for (const axis of OBJECT_AXES) {
    const chosen = filter[axis] ?? [];
    if (chosen.length === 0) continue;
    const parameter = PARAMETER[target][axis];
    if (!parameter) {
      unapplied.push({
        axis,
        why: WHY_NOT[target]?.[axis] ?? 'this view has no parameter for it',
      });
      continue;
    }
    if (chosen.length > 1) {
      unapplied.push({
        axis,
        why: `this view narrows to one ${axis}; ${chosen.length} are chosen`,
      });
      continue;
    }
    const value = chosen[0] as string;
    if (axis === 'status' && !RUN_STATUSES.includes(value)) {
      unapplied.push({ axis, why: `a run is running, succeeded or failed — not “${value}”` });
      continue;
    }
    query[parameter] = value;
    applied.add(axis);
  }

  // The one cast, and [`PARAMETER`] above is what makes it true: every value
  // that reaches here passed the axis's own admission.
  return { query: query as TargetQuery[T], unapplied, notes: notesFor(target, applied) };
}

/**
 * What the sent axes do to one read's numbers.
 *
 * The general rule is rule 3: the filter selects runs, and an axis narrows a
 * counter below the run only where it is about what that counter counts.
 * `/metrics` is the one read that does something else today — its model
 * parameter skips other models' spans and leaves the run set alone — so it
 * says that instead. The two sentences differ in what they claim about the
 * *run* count beside them, which is the difference a reader is entitled to.
 */
function notesFor(target: FilterTarget, applied: Set<ObjectAxis>): string[] {
  if (target === 'metrics') {
    return applied.has('model')
      ? [
          'The model narrows LLM calls, tokens, cost and LLM latency. It does not select the runs, so the run, tool and step counters cover every run matching the other axes.',
        ]
      : [];
  }
  const clauses: string[] = [];
  if (applied.has('model')) {
    clauses.push(
      'LLM calls, tokens, cost and LLM latency are that model’s; tool and step counters cover every call in them',
    );
  }
  if (applied.has('tool')) {
    clauses.push(
      'tool counters are that tool’s; LLM and step counters cover every call in them',
    );
  }
  return clauses.length === 0
    ? []
    : [`Runs are the ones matching the filter. Within them, ${clauses.join('; and ')}.`];
}

