import { z } from 'zod';

/**
 * The live stream, and how a reconnect closes its own gap.
 *
 * The server tags every SSE frame with the event's checkpoint as the `id:`
 * field. That is not decoration: on a dropped connection the browser reconnects
 * on its own and resends the last id it saw as `Last-Event-ID`, with no code
 * here. The server replays exactly what was missed and then sends a `caught_up`
 * frame.
 *
 * `EventSource` cannot set request headers, so the *first* connection passes
 * its cursor as `?from=`. Every automatic reconnect after that uses the header,
 * which the server prefers — the header is always the more current of the two.
 */

const checkpoint = z.string();

export const liveEventSchema = z.object({
  frame: z.literal('event'),
  checkpoint,
  run_id: z.string(),
  conversation_id: z.string().optional(),
  workflow_id: z.string().optional(),
  workflow_run_id: z.string().optional(),
  agent_id: z.string().optional(),
  /** The producing service — the explorer's `runtime` pivot. */
  service: z.string().default('unknown'),
  trace_id: z.string(),
  span_id: z.string(),
  event_type: z.string(),
  sequence: z.number().optional(),
  occurred_at: z.string(),
  data: z.record(z.unknown()),
});

export const liveFrameSchema = z.discriminatedUnion('frame', [
  liveEventSchema,
  z.object({ frame: z.literal('caught'), checkpoint }),
  z.object({ frame: z.literal('resynced'), from: checkpoint }),
]);

export type LiveEventFrame = z.infer<typeof liveEventSchema>;
export type LiveFrame = z.infer<typeof liveFrameSchema>;

export type StreamPhase =
  /** Replaying what happened before this connection opened. */
  | 'catching-up'
  /** Level with the log; new events arrive as they happen. */
  | 'live'
  /** The connection dropped and the browser is retrying. */
  | 'reconnecting';

export interface LiveHandlers {
  onEvent(frame: LiveEventFrame): void;
  onPhase(phase: StreamPhase): void;
  /**
   * The client was too far behind for the server's in-memory buffer and the
   * durable log was read instead. Surfaced so the UI can say so rather than
   * imply the stream was continuous.
   */
  onResync?(from: string): void;
}

function parseLiveFrame(raw: string): LiveFrame | null {
  try {
    return liveFrameSchema.parse(JSON.parse(raw));
  } catch (error) {
    // A frame this build does not understand — a newer server, a new event
    // type — must not tear down the stream. Drop it and keep going.
    console.warn('[aiwatcher] dropping an unparsable live frame', error);
    return null;
  }
}

/**
 * Open a run's stream. Returns a function that closes it.
 *
 * `from` should be the `last_checkpoint` of the run detail the page already
 * rendered — that is what makes the handoff from history to live seamless
 * instead of duplicating or skipping events.
 */
export function openRunStream(
  runId: string,
  from: string | undefined,
  handlers: LiveHandlers,
): () => void {
  return open(`/api/v1/runs/${encodeURIComponent(runId)}/stream`, from, handlers);
}

/**
 * Open a workflow execution's stream. Returns a function that closes it.
 *
 * Scoped to the *execution*, not to a run, and that is the whole difference: a
 * stage-per-pod orchestrator publishes each stage from a different run, so a
 * run-scoped stream would go quiet at exactly the moment the next stage
 * started. The server filters on `workflow_run_id` for the same reason.
 */
export function openWorkflowStream(
  workflowRunId: string,
  from: string | undefined,
  handlers: LiveHandlers,
): () => void {
  return open(
    `/api/v1/workflow-executions/${encodeURIComponent(workflowRunId)}/stream`,
    from,
    handlers,
  );
}

/** Follow every event in the system over the global SSE stream. */
export function openSystemStream(handlers: LiveHandlers): () => void {
  return open('/api/v1/events/stream', undefined, handlers);
}

/**
 * What a live view is watching: or within a dimension, and across them.
 *
 * The same shape the server's `Selection` has, and deliberately not a superset
 * of it. Model and tool are absent because an event does not carry one — they
 * are span-level facts assembled from several events (ADR_0003), so a filter
 * offering them here would quietly drop everything but the LLM call itself.
 * The explorer offers those pivots over the read model, where the span exists.
 */
export interface LiveSelection {
  agents?: string[];
  runtimes?: string[];
  workflows?: string[];
  sessions?: string[];
  eventTypes?: string[];
}

const SELECTION_PARAMS: Array<[keyof LiveSelection, string]> = [
  ['agents', 'agent'],
  ['runtimes', 'runtime'],
  ['workflows', 'workflow'],
  ['sessions', 'session'],
  ['eventTypes', 'event_type'],
];

/**
 * The query string a selection is, as repeated parameters.
 *
 * Exported because it is also what makes the subscription's identity: React
 * has to know when a selection changed, and comparing two objects by reference
 * would reopen the stream on every render.
 */
export function selectionQuery(selection: LiveSelection): string {
  const params = new URLSearchParams();
  for (const [field, name] of SELECTION_PARAMS) {
    for (const value of selection[field] ?? []) {
      // An empty value is a cleared control, not "the agent whose id is the
      // empty string" — the server drops it too, and sending it anyway would
      // make the URL in the address bar disagree with the stream.
      if (value) params.append(name, value);
    }
  }
  return params.toString();
}

/**
 * Follow the events matching a selection. Returns a function that closes it.
 *
 * **Filtered by the server, not here.** Subscribing to everything and
 * discarding most of it in the browser is the same mistake as filtering a list
 * after downloading it: on a busy instance the interesting events are a
 * fraction of a percent of the traffic, and the browser would pay for all of
 * it — including the `llm.chunk` storm that is most of the log by volume.
 */
export function openSelectionStream(
  selection: LiveSelection,
  from: string | undefined,
  handlers: LiveHandlers,
): () => void {
  const query = selectionQuery(selection);
  return open(`/api/v1/events/stream${query ? `?${query}` : ''}`, from, handlers);
}

/** The mechanics both streams share. Only the path differs. */
function open(path: string, from: string | undefined, handlers: LiveHandlers): () => void {
  // `path` may already carry a selection, so the separator is decided rather
  // than assumed: a second `?` produces a URL whose `from` is part of the last
  // parameter's value, and the resume silently starts from the beginning.
  const resume = from ? `${path.includes('?') ? '&' : '?'}from=${encodeURIComponent(from)}` : '';
  const source = new EventSource(`${import.meta.env.VITE_API_BASE_URL ?? ''}${path}${resume}`);

  handlers.onPhase('catching-up');

  source.addEventListener('event', (message) => {
    const frame = parseLiveFrame((message as MessageEvent<string>).data);
    if (frame?.frame === 'event') handlers.onEvent(frame);
  });

  source.addEventListener('caught_up', () => {
    handlers.onPhase('live');
  });

  source.addEventListener('resynced', (message) => {
    const frame = parseLiveFrame((message as MessageEvent<string>).data);
    if (frame?.frame === 'resynced') handlers.onResync?.(frame.from);
  });

  source.addEventListener('error', () => {
    // EventSource retries on its own; this fires on each failed attempt.
    // Reporting it as a phase rather than an error is honest — the stream is
    // not broken, it is between attempts.
    if (source.readyState !== EventSource.CLOSED) {
      handlers.onPhase('reconnecting');
    }
  });

  return () => source.close();
}
