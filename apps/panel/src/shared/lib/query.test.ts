import { afterEach, expect, it, vi } from 'vitest';
import {
  fetchDatasets,
  fetchQueryEngine,
  linkedEngine,
  QueryEngineUnavailableError,
  QueryError,
  writtenElsewhere,
} from '@/shared/lib/query';

afterEach(() => vi.unstubAllGlobals());

function answering(body: unknown, status = 200) {
  const fetch = vi
    .fn()
    .mockResolvedValue(
      new Response(typeof body === 'string' ? body : JSON.stringify(body), { status }),
    );
  vi.stubGlobal('fetch', fetch);
  return fetch;
}

it('preserves a structured upstream failure instead of claiming the engine is offline', async () => {
  answering({ error: { message: 'Dataset hub answered 502 Bad Gateway', column: 0 } }, 502);
  const request = fetchDatasets();
  await expect(request).rejects.toBeInstanceOf(QueryError);
  await expect(request).rejects.toThrow('Dataset hub answered 502');
});

it('recognizes an unstructured proxy failure as an unavailable engine', async () => {
  answering('Bad Gateway', 502);
  await expect(fetchDatasets()).rejects.toBeInstanceOf(QueryEngineUnavailableError);
});

it('asks whichever engine is deployed under /query', async () => {
  const fetch = answering({ datasets: [], source: 'http://api.test', max_rows: 1000 });
  await fetchDatasets();
  expect(fetch).toHaveBeenCalledWith('/query/datasets', undefined);
});

it('reads which engine a deployment runs, and a Flow build older than the field as Flow', async () => {
  answering({ status: 'ok', engine: 'datafusion', language: 'datafusion-python' });
  await expect(fetchQueryEngine()).resolves.toEqual({
    engine: 'datafusion',
    language: 'datafusion-python',
  });

  answering({ status: 'ok' });
  await expect(fetchQueryEngine()).resolves.toEqual({ engine: 'flow', language: 'flow-dsl' });
});

it('reads text from a link as the engine the link names, and as Flow when it names none', () => {
  expect(linkedEngine('read("spans")', 'duckdb', 'datafusion')).toBe('duckdb');
  expect(linkedEngine('data_frame()->read(from_aiwatcher("runs"))', undefined, 'datafusion')).toBe(
    'flow',
  );
  expect(linkedEngine(undefined, undefined, 'datafusion')).toBe('datafusion');
  expect(linkedEngine(undefined, undefined, undefined)).toBe('flow');
});

it('shows a link written for another engine rather than running it', () => {
  const flowLink = linkedEngine('data_frame()->read(from_aiwatcher("runs"))', undefined, 'duckdb');
  expect(writtenElsewhere(flowLink, 'duckdb')).toBe(
    'Written for Flow PHP, and this deployment runs DuckDB, so it is shown here and not run.',
  );
  expect(writtenElsewhere(linkedEngine('read("spans")', 'duckdb', 'duckdb'), 'duckdb')).toBeNull();
});
