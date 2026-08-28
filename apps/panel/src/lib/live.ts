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
  const query = from ? `?from=${encodeURIComponent(from)}` : '';
  const source = new EventSource(
    `${import.meta.env.VITE_API_BASE_URL ?? ''}/api/v1/runs/${encodeURIComponent(runId)}/stream${query}`,
  );

  handlers.onPhase('catching-up');

  const parse = (raw: string): LiveFrame | null => {
    try {
      return liveFrameSchema.parse(JSON.parse(raw));
    } catch (error) {
      // A frame this build does not understand — a newer server, a new event
      // type — must not tear down the stream. Drop it and keep going.
      console.warn('[aiwatcher] dropping an unparsable live frame', error);
      return null;
    }
  };

  source.addEventListener('event', (message) => {
    const frame = parse((message as MessageEvent<string>).data);
    if (frame?.frame === 'event') handlers.onEvent(frame);
  });

  source.addEventListener('caught_up', () => {
    handlers.onPhase('live');
  });

  source.addEventListener('resynced', (message) => {
    const frame = parse((message as MessageEvent<string>).data);
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
