import { beforeEach, expect, it, vi } from 'vitest';
import type { PipelineBlock } from '@/api/generated/types.gen';
import { runQuery, simulateQuery } from './flow';
import { getNotebook, runNotebook } from './ml-pipeline';
import { runPipeline, withPinnedNotebooks } from './pipeline';

vi.mock('./flow', () => ({ runQuery: vi.fn(), simulateQuery: vi.fn() }));
vi.mock('./ml-pipeline', () => ({ getNotebook: vi.fn(), runNotebook: vi.fn() }));

const chain: PipelineBlock[] = [
  { id: 'source', title: 'Import', spec: { kind: 'source', dataset: 'runs' } },
  {
    id: 'clean',
    title: 'Clean',
    spec: { kind: 'transform', steps: "->withEntry('cleaned', lit(true))" },
  },
  {
    id: 'python',
    title: 'Python',
    spec: { kind: 'notebook', notebook: 'custom', revision: 'a'.repeat(64) },
  },
  { id: 'publish', title: 'Save', spec: { kind: 'view', dataset: 'curation/test' } },
];
const sourceRows = [{ id: 1 }, { id: 2 }];
const cleanedRows = sourceRows.map((row) => ({ ...row, cleaned: true }));
function result(rows: Record<string, unknown>[]) {
  return {
    columns: Object.keys(rows[0]!),
    rows,
    row_count: rows.length,
    took_ms: 1,
    truncated: false,
    source: 'runs',
  } as Awaited<ReturnType<typeof runQuery>>;
}
beforeEach(() => vi.resetAllMocks());

it('shows the actual output of each PHP prefix and passes the last output to Python once', async () => {
  vi.mocked(simulateQuery)
    .mockResolvedValueOnce(result(sourceRows))
    .mockResolvedValueOnce(result(cleanedRows));
  vi.mocked(runNotebook).mockResolvedValue({
    notebook: 'custom',
    revision: 'a'.repeat(64),
    rows: [{ done: true }],
    columns: ['done'],
    row_count: 1,
    took_ms: 1,
    truncated: false,
    stdout: 'report',
    app_url: '',
  });
  const onBlockResult = vi.fn();
  const output = await runPipeline({ chain, mode: 'preview', inspectBlocks: true, onBlockResult });
  expect(simulateQuery).toHaveBeenCalledTimes(2);
  expect(vi.mocked(simulateQuery).mock.calls[0]![0]).not.toContain('withEntry');
  expect(vi.mocked(simulateQuery).mock.calls[1]![0]).toContain('withEntry');
  expect(runNotebook).toHaveBeenCalledExactlyOnceWith('custom', cleanedRows, {}, 'a'.repeat(64));
  expect(onBlockResult.mock.calls.map(([id, value]) => [id, value.rows])).toEqual([
    ['source', sourceRows],
    ['clean', cleanedRows],
    ['python', [{ done: true }]],
    ['publish', [{ done: true }]],
  ]);
  expect(output.rows).toEqual([{ done: true }]);
});

it('stops at the requested prefix without running downstream Python or publishing', async () => {
  vi.mocked(simulateQuery).mockResolvedValue(result(sourceRows));
  await runPipeline({ chain: chain.slice(0, 1), mode: 'preview', inspectBlocks: true });
  expect(simulateQuery).toHaveBeenCalledTimes(1);
  expect(runNotebook).not.toHaveBeenCalled();
});

it('retains successful cells when a later cell fails and does not run downstream code', async () => {
  vi.mocked(simulateQuery)
    .mockResolvedValueOnce(result(sourceRows))
    .mockRejectedValueOnce(new Error('Missing column'));
  const onOutcome = vi.fn();
  await expect(
    runPipeline({ chain, mode: 'preview', inspectBlocks: true, onOutcome }),
  ).rejects.toThrow('Missing column');
  expect(onOutcome.mock.calls.filter(([id]) => id === 'source').at(-1)?.[1].status).toBe('done');
  expect(onOutcome.mock.calls.at(-1)).toEqual([
    'clean',
    { status: 'failed', message: 'Missing column' },
  ]);
  expect(runNotebook).not.toHaveBeenCalled();
});

it('saves the displayed pinned revision without silently following a changed head', async () => {
  const pinned = chain[2]!;
  expect(await withPinnedNotebooks([pinned])).toEqual([pinned]);
  expect(getNotebook).not.toHaveBeenCalled();
});
