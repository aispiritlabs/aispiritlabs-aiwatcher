/** Durable publication raises on failure. Retry an identical body and ID after
 * a timeout; the server owns idempotency and returns conflicts explicitly. */
import type { EvaluationManifest } from './evaluation.js';

export interface CaseMeasurement {
  case_id: string;
  repetition_id: string;
  actual: unknown;
  metrics: Record<string, number>;
  error: string | null;
  trace_id: string | null;
  span_id: string | null;
}
export interface EvaluationReceipt {
  evaluation_id: string;
  version: string;
  variant_id: string;
  context_id: string;
  committed_at: number;
  expires_at: number;
}
export type EvidenceState = 'complete' | 'partial' | 'missing_artifact' | 'corrupt_artifact' | 'expired' | 'deleted_source' | 'forbidden';
export interface DurableEvaluation {
  receipt: EvaluationReceipt;
  state: EvidenceState;
  manifest: EvaluationManifest | null;
  status: 'succeeded' | 'failed' | 'partial' | null;
  counts: { selected: number; scored: number; failed: number; unscored: number } | null;
  metrics: Record<string, number>;
}
export interface CasePage {
  version: string;
  state: EvidenceState;
  cases: { measurement: CaseMeasurement; expected: unknown }[];
  next_cursor: string | null;
}
export class EvaluationRegistryError extends Error {
  readonly status: number;
  constructor(status: number, message: string) { super(message); this.status = status; }
}
export class EvaluationRegistry {
  private readonly base: string;
  private readonly options: { token?: string; timeoutMs?: number; fetch?: typeof fetch };
  constructor(baseUrl: string, options: { token?: string; timeoutMs?: number; fetch?: typeof fetch } = {}) {
    this.options = options;
    this.base = baseUrl.replace(/\/+$/, '');
  }
  publish(manifest: EvaluationManifest, cases: CaseMeasurement[], status: 'succeeded' | 'failed' | 'partial' = 'succeeded'): Promise<EvaluationReceipt> {
    return this.request('', { method: 'POST', body: JSON.stringify({ manifest, cases, status }) });
  }
  get(id: string): Promise<DurableEvaluation> { return this.request('/' + encodeURIComponent(id)); }
  list(cursor?: string, limit = 200): Promise<{ evaluations: DurableEvaluation[]; next_cursor: string | null }> {
    const query = new URLSearchParams({ limit: String(limit) });
    if (cursor !== undefined) query.set('cursor', cursor);
    return this.request('?' + query);
  }
  cases(id: string, version: string, cursor?: string, limit = 200): Promise<CasePage> {
    const query = new URLSearchParams({ version, limit: String(limit) });
    if (cursor !== undefined) query.set('cursor', cursor);
    return this.request('/' + encodeURIComponent(id) + '/cases?' + query);
  }
  private async request<T>(path: string, init: RequestInit = {}): Promise<T> {
    const headers: Record<string, string> = { 'content-type': 'application/json' };
    if (this.options.token) headers.authorization = 'Bearer ' + this.options.token;
    const response = await (this.options.fetch ?? fetch)(this.base + '/api/v1/evaluation-results' + path, {
      ...init, headers, redirect: 'error', signal: AbortSignal.timeout(this.options.timeoutMs ?? 30_000),
    });
    if (!response.ok) throw new EvaluationRegistryError(response.status, await response.text());
    return response.json() as Promise<T>;
  }
}
