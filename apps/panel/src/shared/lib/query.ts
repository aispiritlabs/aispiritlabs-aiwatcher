import { useQuery, type UseQueryResult } from '@tanstack/react-query';
import { z } from 'zod';

/**
 * The client for the query engine.
 *
 * A deployment runs one — Flow PHP, DataFusion or DuckDB — chosen by
 * `AIWATCHER_QUERY_ENGINE`, and every one of them serves the same contract under
 * `/query` (AW-3). So this is one client; what differs between engines is the
 * language a query is written in, which `/query/healthz` names and
 * `useQueryEngine()` reads.
 *
 * Hand-written, unlike everything under `api/generated`: the engine is not in
 * the Rust OpenAPI document and deliberately so — the panel talks to it
 * directly, and aiwatcher's binary does not know it exists (ADR_0008). That
 * makes runtime validation the right call here for the same reason it is for
 * the SSE frames: there is no codegen to lean on.
 *
 * It is also the one part of the panel that must work when its backend is
 * absent. `isQueryEngineAvailable()` answering `false` is a normal state, not an
 * error.
 */

/** The engines a deployment may run, by the word it is chosen by. */
export const QUERY_ENGINES = ['flow', 'datafusion', 'duckdb'] as const;
export type QueryEngineName = (typeof QUERY_ENGINES)[number];

/** Each engine's name in a sentence somebody reads. */
export const ENGINE_LABEL: Record<QueryEngineName, string> = {
  flow: 'Flow PHP',
  datafusion: 'DataFusion',
  duckdb: 'DuckDB',
};

/**
 * The sentence content written for another engine is shown with, or `null` when
 * it is this deployment's.
 *
 * Shown rather than run (AW-3): text in one engine's language is a syntax error
 * in another's, and the engine's refusal would read as the text being wrong
 * rather than as the deployment being a different one. `deployed` is absent
 * while healthz has not answered, and then nothing is foreign yet.
 */
export function writtenElsewhere(
  written: QueryEngineName,
  deployed: QueryEngineName | null | undefined,
): string | null {
  if (!deployed || written === deployed) return null;
  return `Written for ${ENGINE_LABEL[written]}, and this deployment runs ${ENGINE_LABEL[deployed]}, so it is shown here and not run.`;
}

/**
 * The engine text carried in a URL was written for.
 *
 * A link says, as `writtenFor`; one that does not is Flow's, because that is what
 * every link made before the field held — the rule stored content already lives
 * under. No text at all means the deployed engine's own starter.
 */
export function linkedEngine(
  q: string | undefined,
  writtenFor: QueryEngineName | undefined,
  deployed: QueryEngineName | undefined,
): QueryEngineName {
  if (q === undefined) return deployed ?? 'flow';
  return writtenFor ?? 'flow';
}

const engineSchema = z.object({
  /** Absent from a Flow build that predates AW-3, which is Flow. */
  engine: z.enum(QUERY_ENGINES).optional().default('flow'),
  /** The language a query is written in: `flow-dsl`, `datafusion-python`, `duckdb-python`. */
  language: z.string().optional().default('flow-dsl'),
});

const columnSchema = z.object({ name: z.string(), type: z.string() });

/**
 * A named argument a `read()` may carry, beyond the dataset name.
 *
 * Part of the contract rather than a hint: the aiwatcher API rejects unknown
 * query parameters, so a `read()` argument the dataset never declared turns
 * the whole query into a 400.
 */
const parameterSchema = z.object({
  name: z.string(),
  required: z.boolean(),
  description: z.string(),
  values: z.array(z.string()),
});

const datasetSchema = z.object({
  name: z.string(),
  aliases: z.array(z.string()),
  grain: z.string(),
  description: z.string(),
  requires_run: z.boolean(),
  columns: z.array(columnSchema),
  /** Optional so a panel build stays compatible with an older Flow service. */
  parameters: z.array(parameterSchema).optional().default([]),
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
  /** Which checkers ran — for Flow, `mago` for syntax and `aiwatcher` for the grammar and schema. */
  checked_by: z.array(z.string()),
});

const errorSchema = z.object({
  error: z.object({
    message: z.string(),
    column: z.number(),
    near: z.string().nullable().optional(),
  }),
});

export type QueryEngineInfo = z.infer<typeof engineSchema>;
export type QueryDiagnostic = z.infer<typeof diagnosticSchema>;
export type QueryCheck = z.infer<typeof checkSchema>;
export type QueryDataset = z.infer<typeof datasetSchema>;
export type QueryParameter = z.infer<typeof parameterSchema>;
export type QueryDatasets = z.infer<typeof datasetsSchema>;
export type QueryResult = z.infer<typeof resultSchema>;

/** A query the engine refused, with where it gave up. */
export class QueryError extends Error {
  constructor(
    message: string,
    readonly column: number,
  ) {
    super(message);
    this.name = 'QueryError';
  }
}

/** Raised when the engine itself is not there. Its own type, because the page treats it differently. */
export class QueryEngineUnavailableError extends Error {
  constructor() {
    super('The query engine is not running.');
    this.name = 'QueryEngineUnavailableError';
  }
}

async function call(path: string, init?: RequestInit): Promise<unknown> {
  let response: Response;

  try {
    response = await fetch(`/query${path}`, init);
  } catch {
    // A refused connection is the engine being absent, not a failed query.
    throw new QueryEngineUnavailableError();
  }

  const body: unknown = await response.json().catch(() => null);
  const serviceError = errorSchema.safeParse(body);
  // A structured upstream failure proves the engine answered. Preserve its
  // reason (e.g. a dataset hub's 502); only an unstructured proxy failure means
  // absent.
  if ([500, 502, 503, 504].includes(response.status) && !serviceError.success) {
    throw new QueryEngineUnavailableError();
  }

  // 404 is a *different* process answering. Every path here is one the engine
  // serves, so it never 404s on its own; what does is whatever else happens to
  // hold the port the proxy points at — measured against a Go service on :8081
  // answering `404 page not found` to everything. Reading that as a refused
  // query blames the pipeline for a port collision, and the message it produces
  // ("The service answered 404") sends the reader to the query they just wrote.
  if (response.status === 404) {
    throw new QueryEngineUnavailableError();
  }

  if (!response.ok) {
    const parsed = serviceError;
    throw new QueryError(
      parsed.success ? parsed.data.error.message : `The service answered ${response.status}.`,
      parsed.success ? parsed.data.error.column : 0,
    );
  }

  return body;
}

export async function isQueryEngineAvailable(): Promise<boolean> {
  try {
    await call('/healthz');
    return true;
  } catch {
    return false;
  }
}

/** Which engine this deployment runs, and the language it reads. */
export async function fetchQueryEngine(): Promise<QueryEngineInfo> {
  return engineSchema.parse(await call('/healthz'));
}

/**
 * The engine this deployment runs, or `null` when there is none.
 *
 * `null` is an answer rather than an error: the engine is optional, and a panel
 * with no engine behind it says so rather than failing.
 */
export function useQueryEngine(): UseQueryResult<QueryEngineInfo | null> {
  return useQuery({
    queryKey: ['query-engine'],
    queryFn: async () => {
      try {
        return await fetchQueryEngine();
      } catch (error) {
        if (error instanceof QueryEngineUnavailableError) return null;
        throw error;
      }
    },
    // Which engine a deployment runs changes with a release, not with a click.
    staleTime: 5 * 60_000,
  });
}

export async function fetchDatasets(): Promise<QueryDatasets> {
  return datasetsSchema.parse(await call('/datasets'));
}

/**
 * What is wrong with a query, without running it.
 *
 * The engine decides what its checkers are. Flow's are two: Mago parses the
 * query as PHP (after the service substitutes the bareword dataset names, which
 * are not valid PHP) and reports where the brackets stopped making sense, and
 * the service's own parser knows the grammar and every column. Neither executes
 * anything.
 */
export async function checkQuery(pipeline: string): Promise<QueryCheck> {
  return checkSchema.parse(
    await call('/check', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ pipeline }),
    }),
  );
}

/**
 * Run a query, scoped to the script's period or the panel's fallback window.
 *
 * `read(default, period: '24h')` is reproducible and wins when present. The
 * request parameter remains a fallback for older/ad-hoc scripts and is
 * forwarded only to aiwatcher routes that accept one.
 */
export async function runQuery(pipeline: string, windowSeconds?: number): Promise<QueryResult> {
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
 * Execute the same plan against a 25-row preview and persist nothing.
 *
 * This has its own endpoint rather than a client-side slice: the cap is
 * applied while the engine reads, so a simulation cannot accidentally walk the
 * full result just to hide most of it in the browser.
 */
export async function simulateQuery(
  pipeline: string,
  windowSeconds?: number,
): Promise<QueryResult> {
  return resultSchema.parse(
    await call('/simulate', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(
        windowSeconds ? { pipeline, window_seconds: windowSeconds } : { pipeline },
      ),
    }),
  );
}
