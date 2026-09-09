import { afterEach, expect, it, vi } from 'vitest';
import { fetchDatasets, FlowQueryError, FlowUnavailableError } from './flow';

afterEach(() => vi.unstubAllGlobals());

it('preserves a structured upstream failure instead of claiming Flow is offline', async () => {
  vi.stubGlobal(
    'fetch',
    vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          error: { message: 'Dataset hub answered 502 Bad Gateway', column: 0 },
        }),
        { status: 502 },
      ),
    ),
  );
  const request = fetchDatasets();
  await expect(request).rejects.toBeInstanceOf(FlowQueryError);
  await expect(request).rejects.toThrow('Dataset hub answered 502');
});

it('recognizes an unstructured proxy failure as an unavailable service', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('Bad Gateway', { status: 502 })));
  await expect(fetchDatasets()).rejects.toBeInstanceOf(FlowUnavailableError);
});
