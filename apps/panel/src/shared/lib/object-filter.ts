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
 * them. `prompt` is the newest axis and the one the read model had to grow —
 * a span carried the registered prompt a call named and no read grouped by it,
 * so "which prompts does this agent run on" was a question with data behind it
 * and no way to ask.
 *
 * 1. **One name per axis**, the same in every URL. Translating an axis to a
 *    route's parameter happens here and nowhere else.
 * 2. **A filter selects runs.** Every axis names a property a run has —
 *    directly, or through its spans, which is how `RunFilter` already matches
 *    `model` and `tool`. Axes are conjunctive.
 * 3. **A number is counted over the selected runs**, and narrowed further only
 *    where the axis is about the thing being counted *and* the read counts
 *    something smaller than a run. On `/metrics`, which folds calls, a chosen
 *    model makes LLM calls and tokens that model's while tool calls stay every
 *    tool call in those runs, because a tool call has no model. On `/runs` and
 *    `/dimensions`, whose every figure is a run's own total, nothing below the
 *    run narrows at all — and [`queryFor`] says which of the two a page is
 *    showing.
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
  'prompt',
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
  prompt: values,
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
 * The run's own axes, under the read model's parameter names.
 *
 * Three reads share this table because they now share a predicate: `/runs`
 * lists the runs a filter selects, `/dimensions/{kind}` groups them and
 * `/metrics` folds them, so a question one of them can be asked can be asked
 * of all three. The span list is the one that differs, and differs about
 * *what a span is* rather than about what has been implemented.
 */
const RUN_PARAMETERS: Record<ObjectAxis, string | null> = {
  agent: 'agent_id',
  runtime: 'runtime',
  workflow: 'workflow',
  session: 'conversation_id',
  variant: 'variant_id',
  trace: 'trace_id',
  model: 'model',
  tool: 'tool',
  prompt: 'prompt',
  status: 'status',
};

/**
 * Axis → the route's parameter, or why it has none.
 *
 * `null` is "this route has no parameter for it"; a string is the parameter's
 * name. The sentences for the nulls are in [`WHY_NOT`] rather than here, so
 * that adding a parameter server-side is one edit in one table — and that is
 * what this table's history is. Three of the four reads took a handful of axes
 * when it was written; the dimension and metrics rows are full now because the
 * projector grew the rest and folds all three reads through one predicate
 * (`crate::selection`).
 */
const PARAMETER: Record<FilterTarget, Record<ObjectAxis, string | null>> = {
  runs: RUN_PARAMETERS,
  spans: {
    agent: 'agent_id',
    runtime: null,
    workflow: null,
    session: null,
    variant: null,
    trace: 'trace_id',
    model: 'model',
    tool: 'tool',
    prompt: 'prompt',
    // Deliberately not `status`: the span route's is `ok | error`, which is a
    // span's outcome and not the run's. A failed span inside a run that
    // succeeded is an ordinary thing, so sharing one word would make the
    // filter lie about what it selected.
    status: null,
  },
  dimensions: RUN_PARAMETERS,
  metrics: RUN_PARAMETERS,
};

/**
 * Why a route cannot answer an axis, in the words the page renders.
 *
 * A fact about the read model, not an apology — and the span list is the only
 * read left with any. The five it names are properties of a *run*: a span does
 * not name the session, the workflow, the runtime or the variant its run has,
 * and its own `ok | error` is not the run's status. No parameter is coming for
 * them, which is the opposite of the six the dimension and metrics routes were
 * missing when this was written and now have.
 */
const WHY_NOT: Partial<Record<FilterTarget, Partial<Record<ObjectAxis, string>>>> = {
  spans: {
    runtime: 'a span does not name the service that produced it',
    workflow: 'a span does not name its workflow',
    session: 'a span does not name its session',
    variant: 'a span list is not narrowed by variant',
    status: 'a span carries its own outcome, not the run’s status',
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
  runs: RunQuery;
  spans: { agent_id?: string; trace_id?: string; model?: string; tool?: string; prompt?: string };
  dimensions: RunQuery;
  metrics: RunQuery;
}

/** The query [`RUN_PARAMETERS`] translates to, for the three reads that take it. */
interface RunQuery {
  agent_id?: string;
  runtime?: string;
  workflow?: string;
  conversation_id?: string;
  variant_id?: string;
  trace_id?: string;
  model?: string;
  tool?: string;
  prompt?: string;
  status?: RunStatus;
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
 * Rule 3, per read, because the four reads count different things and only one
 * of them counts anything smaller than a run.
 *
 * `/metrics` folds the calls of the selected runs, so a call-level axis
 * narrows the call-level counters and leaves the rest: with a model chosen,
 * LLM calls and tokens are that model's while tool calls are every tool call
 * in those runs, because a tool call has no model.
 *
 * `/runs` and `/dimensions/{kind}` count nothing below the run: every figure
 * on a row is the run's own total, folded when the run was ingested and the
 * same number whatever the filter says. So a model axis *selects* there and
 * narrows nothing, and saying otherwise would put the metrics page's sentence
 * over a list it is not true of — two numbers a reader would take for one.
 *
 * The span list says nothing, and that is the point: its rows *are* the calls
 * the filter names, which is what the chips above them already claim.
 */
function notesFor(target: FilterTarget, applied: Set<ObjectAxis>): string[] {
  // The axes a call carries. A run carries the rest, and no read narrows a run
  // by something the run itself is.
  const call = (['model', 'prompt', 'tool'] as const).filter((axis) => applied.has(axis));
  if (call.length === 0 || target === 'spans') return [];

  if (target === 'metrics') {
    const clauses: string[] = [];
    const llm = call.filter((axis) => axis !== 'tool');
    if (llm.length > 0) {
      clauses.push(
        `LLM calls, tokens, cost and LLM latency are ${llm
          .map((axis) => `that ${axis}’s`)
          .join(' and ')}`,
      );
    }
    if (applied.has('tool')) clauses.push('tool counters are that tool’s');
    return [
      `Runs are the ones matching the filter. Within them, ${clauses.join(
        '; and ',
      )}; every other counter covers every call in those runs.`,
    ];
  }

  return [
    `Runs are the ones matching the filter. Each row's counts are the run's own totals, over every call in it rather than only ${call
      .map((axis) => `that ${axis}’s`)
      .join(' and ')}.`,
  ];
}
