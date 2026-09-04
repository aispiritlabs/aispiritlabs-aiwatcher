import { z } from 'zod';

/**
 * The client for the `ml_pipeline` notebook runtime.
 *
 * Hand-written for the same reason `flow.ts` is: this service is not in the
 * Rust OpenAPI document and deliberately so — the panel talks to it directly,
 * and aiwatcher's binary does not know it exists (ADR_0008, ADR_0024). No
 * codegen to lean on means runtime validation belongs here.
 *
 * Like Flow's, it must work when its backend is absent: `isMlPipelineAvailable()`
 * answering `false` is a normal state, not an error. A pipeline whose blocks
 * are all source and transform runs perfectly well without this service.
 */

const summarySchema = z.object({
  name: z.string(),
  title: z.string(),
  /** sha256 of the source. What a saved pipeline block pins. */
  revision: z.string(),
  size: z.number(),
  modified_at: z.string(),
});

const notebookSchema = summarySchema.extend({
  source: z.string(),
  app_url: z.string(),
});

const runSchema = z.object({
  notebook: z.string(),
  revision: z.string(),
  columns: z.array(z.string()),
  rows: z.array(z.record(z.string(), z.unknown())),
  row_count: z.number(),
  truncated: z.boolean(),
  /** Whatever the notebook printed. The first place to look when it did nothing. */
  stdout: z.string(),
  took_ms: z.number(),
  app_url: z.string(),
});

const errorSchema = z.object({
  error: z.object({
    message: z.string(),
    stdout: z.string().optional(),
    stderr: z.string().optional(),
  }),
});

export type NotebookSummary = z.infer<typeof summarySchema>;
export type NotebookSource = z.infer<typeof notebookSchema>;
export type NotebookRun = z.infer<typeof runSchema>;

/** Raised when the service itself is not there. Its own type, because the page treats it differently. */
export class MlPipelineUnavailableError extends Error {
  constructor() {
    super('The notebook runtime is not running.');
    this.name = 'MlPipelineUnavailableError';
  }
}

/**
 * A notebook the service refused or that failed while running.
 *
 * `stderr` is carried separately and rendered as a block: a traceback squeezed
 * into one sentence is a traceback nobody reads, and the line that names the
 * cell is usually the answer.
 */
export class NotebookError extends Error {
  constructor(
    message: string,
    readonly stderr = '',
    readonly stdout = '',
  ) {
    super(message);
    this.name = 'NotebookError';
  }
}

async function call(path: string, init?: RequestInit): Promise<unknown> {
  let response: Response;

  try {
    response = await fetch(`/ml-pipeline${path}`, init);
  } catch {
    throw new MlPipelineUnavailableError();
  }

  // The same statuses `flow.ts` reads as "nothing is listening": vite answers
  // 500 for a refused connection, an ingress 502/503, and a 404 is a different
  // process holding the port. This service never emits any of them — a refused
  // notebook is a 422 and a missing one a 404 with a body, both of which parse.
  if ([500, 502, 503, 504].includes(response.status)) throw new MlPipelineUnavailableError();

  const body: unknown = await response.json().catch(() => null);
  const parsed = errorSchema.safeParse(body);

  if (response.status === 404 && !parsed.success) throw new MlPipelineUnavailableError();

  if (!response.ok) {
    throw new NotebookError(
      parsed.success ? parsed.data.error.message : `The service answered ${response.status}.`,
      parsed.success ? (parsed.data.error.stderr ?? '') : '',
      parsed.success ? (parsed.data.error.stdout ?? '') : '',
    );
  }

  return body;
}

export async function isMlPipelineAvailable(): Promise<boolean> {
  try {
    await call('/healthz');
    return true;
  } catch {
    return false;
  }
}

export async function getNotebooks(): Promise<NotebookSummary[]> {
  const body = z.object({ notebooks: z.array(summarySchema) }).parse(await call('/notebooks'));
  return body.notebooks;
}

export async function getNotebook(name: string): Promise<NotebookSource> {
  return notebookSchema.parse(await call(`/notebooks/${encodeURIComponent(name)}`));
}

export async function saveNotebook(name: string, source: string): Promise<NotebookSource> {
  return notebookSchema.parse(
    await call(`/notebooks/${encodeURIComponent(name)}`, {
      method: 'PUT',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ source }),
    }),
  );
}

/**
 * Run one notebook over rows and return what it handed on.
 *
 * It also leaves those rows staged, which is what makes the live app in the
 * block's editor useful: open it after a preview and the widgets are being
 * moved against the rows the chain actually produced.
 */
export async function runNotebook(
  notebook: string,
  rows: Array<Record<string, unknown>>,
  params: Record<string, unknown> = {},
): Promise<NotebookRun> {
  return runSchema.parse(
    await call('/run', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ notebook, rows, params }),
    }),
  );
}
