import { z } from 'zod';

/**
 * The client for the Flow query service.
 *
 * Hand-written, unlike everything under `api/generated`: this service is not in
 * the Rust OpenAPI document and deliberately so — the panel talks to it
 * directly, and aiwatcher's binary does not know it exists (ADR_0008). That
 * makes runtime validation the right call here for the same reason it is for
 * the SSE frames: there is no codegen to lean on.
 *
 * It is also the one part of the panel that must work when its backend is
 * absent. `probe()` answering `false` is a normal state, not an error.
 */

const columnSchema = z.object({ name: z.string(), type: z.string() });

const datasetSchema = z.object({
  name: z.string(),
  aliases: z.array(z.string()),
  grain: z.string(),
  description: z.string(),
  requires_run: z.boolean(),
  columns: z.array(columnSchema),
});

const datasetsSchema = z.object({
  datasets: z.array(datasetSchema),
  source: z.string(),
  max_rows: z.number(),
});

const resultSchema = z.object({
  columns: z.array(z.string()),
  rows: z.array(z.record(z.string(), z.unknown())),
  row_count: z.number(),
  truncated: z.boolean(),
  /** From `to_output(truncate:)` — whether long cells may be shortened. */
  truncate_cells: z.boolean(),
  dataset: z.string().nullable(),
  grain: z.string().nullable(),
  source: z.string(),
  /** The window the rows were read through, so a short table reads as scoped. */
  window_seconds: z.number().nullable().optional(),
  took_ms: z.number(),
});

const diagnosticSchema = z.object({
  level: z.string(),
  message: z.string(),
  /** Offset into the query as typed — enrichment maps it back for us. */
  offset: z.number(),
  line: z.number(),
  help: z.string().nullable(),
});

const checkSchema = z.object({
  ok: z.boolean(),
  diagnostics: z.array(diagnosticSchema),
  /** Which checkers ran: `mago` for syntax, `aiwatcher` for the grammar and schema. */
  checked_by: z.array(z.string()),
});

const errorSchema = z.object({
  error: z.object({
    message: z.string(),
    column: z.number(),
    near: z.string().nullable().optional(),
  }),
});

export type FlowDiagnostic = z.infer<typeof diagnosticSchema>;
export type FlowCheck = z.infer<typeof checkSchema>;
export type FlowDataset = z.infer<typeof datasetSchema>;
export type FlowDatasets = z.infer<typeof datasetsSchema>;
export type FlowResult = z.infer<typeof resultSchema>;

/** A query the service refused, with where it gave up. */
export class FlowQueryError extends Error {
  constructor(
    message: string,
    readonly column: number,
  ) {
    super(message);
    this.name = 'FlowQueryError';
  }
}

/** Raised when the service itself is not there. Its own type, because the page treats it differently. */
export class FlowUnavailableError extends Error {
  constructor() {
    super('The Flow query service is not running.');
    this.name = 'FlowUnavailableError';
  }
}

async function call(path: string, init?: RequestInit): Promise<unknown> {
  let response: Response;

  try {
    response = await fetch(`/flow${path}`, init);
  } catch {
    // A refused connection is the service being absent, not a failed query.
    throw new FlowUnavailableError();
  }

  // Nothing is listening behind the proxy. Which status that is depends on the
  // proxy: vite's dev server answers 500 (measured), an ingress typically 502
  // or 503. Including 500 is safe because the service itself never emits one —
  // a refused query is a 422, and an unreachable aiwatcher is a 502 with a
  // body. Without this, killing the service mid-session would report the next
  // Run as a refused query rather than as a missing service.
  if ([500, 502, 503, 504].includes(response.status)) {
    throw new FlowUnavailableError();
  }

  const body: unknown = await response.json().catch(() => null);

  if (!response.ok) {
    const parsed = errorSchema.safeParse(body);
    throw new FlowQueryError(
      parsed.success ? parsed.data.error.message : `The service answered ${response.status}.`,
      parsed.success ? parsed.data.error.column : 0,
    );
  }

  return body;
}

export async function isFlowAvailable(): Promise<boolean> {
  try {
    await call('/healthz');
    return true;
  } catch {
    return false;
  }
}

export async function fetchDatasets(): Promise<FlowDatasets> {
  return datasetsSchema.parse(await call('/datasets'));
}

/**
 * What is wrong with a query, without running it.
 *
 * Two checkers behind this: Mago parses the query as PHP (after the service
 * substitutes the bareword dataset names, which are not valid PHP) and reports
 * where the brackets stopped making sense; the service's own parser knows the
 * grammar, the whitelist and every column. Neither executes anything.
 */
export async function checkQuery(pipeline: string): Promise<FlowCheck> {
  return checkSchema.parse(
    await call('/check', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ pipeline }),
    }),
  );
}

/**
 * Run a query, scoped to the panel's time window.
 *
 * The window is a parameter of the request rather than a step in the query:
 * the service forwards it to the aiwatcher routes that accept one and leaves
 * the per-run `events` route alone. A `->window(900)` step would be a second
 * way to say what every other tab's control already says, and the two would
 * disagree the first time somebody set both.
 */
export async function runQuery(pipeline: string, windowSeconds?: number): Promise<FlowResult> {
  return resultSchema.parse(
    await call('/query', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(
        windowSeconds ? { pipeline, window_seconds: windowSeconds } : { pipeline },
      ),
    }),
  );
}

/**
 * The query the editor starts with.
 *
 * Deliberately the corrected form of the obvious first query rather than the
 * obvious one: `runs` carries `agents` as a list, so grouping by agent needs
 * the expansion. Starting from something that runs teaches the shape; starting
 * from something that errors teaches nothing.
 */
export const STARTER_QUERY = `data_frame()
    ->read(default)
    ->withEntry('agent', array_expand(ref('agents')))
    ->groupBy(ref('agent'))
    ->aggregate(
        count(ref('run_id')->as('runs')),
        sum(ref('input_tokens')->as('input_tokens'))
    )
    ->sortBy(ref('runs')->desc())
    ->write(to_output(truncate: false))
    ->run();`;
