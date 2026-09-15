import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createMemoryHistory, createRootRoute, createRoute, createRouter, RouterProvider } from '@tanstack/react-router';
import { afterEach, expect, it, vi } from 'vitest';
import { PipelinePage } from './pipeline/page';
import { DataCurationPage } from './recipe/page';
import { searchSchema as pipelineSearch } from './pipeline/search';
import { searchSchema as recipeSearch } from './recipe/search';
import { serve, withQueries } from '@/test/server';

vi.mock('@/features/data-curation/components/pipeline-canvas', () => ({ PipelineCanvas: () => <p>Canvas</p>, blockLabel: (kind: string) => kind }));
afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });
const rows = [{ value: 'one' }, { value: 'two' }];
const result = { columns: ['value'], rows, row_count: 2, truncated: false, truncate_cells: false,
  dataset: 'runs', grain: 'run', source: 'test-source', window_seconds: 3600, took_ms: 1 };
const pipeline = { name: 'sample-flow', description: 'Sample flow', revision: 'flow-revision', saved_at: '2026-09-14T10:00:00Z',
  blocks: [
    { id: 'source', title: 'Source', spec: { kind: 'source', dataset: 'runs', arguments: {} } },
    { id: 'python', title: 'Python', spec: { kind: 'notebook', notebook: 'one', params: {} } },
    { id: 'after', title: 'After', spec: { kind: 'notebook', notebook: 'two', params: {} } },
    { id: 'view', title: 'Output', spec: { kind: 'view', dataset: 'production/output' } },
  ], edges: [{ from: 'source', to: 'python' }, { from: 'python', to: 'after' }, { from: 'after', to: 'view' }] };
const summary = { version: 'sample-version', row_count: 2, columns: ['value'], created_at: '2026-09-14T10:00:00Z',
  sample: { mode: 'preview', truncated_stages: [] } };
const receipt = { created: true, dataset: { name: 'production/output/samples', description: '', latest: summary, versions: [summary] } };

async function setup({ recipe = false, truncated = false, failPublish = false, unsupportedSamples = false, notebookView = false } = {}) {
  const server = serve([
    { method: 'GET', path: '/healthz', answer: { status: 200, body: { engine: 'flow' } } },
    { method: 'GET', path: '/auth/config', answer: { status: 200, body: { enabled: false, mode: 'none' } } },
    { method: 'GET', path: '/curations', answer: { status: 200, body: { recipes: [] } } },
    { method: 'GET', path: '/curation-pipelines', answer: { status: 200, body: { pipelines: [pipeline] } } },
    { method: 'GET', path: '/revisions/flow-revision', answer: { status: 200, body: pipeline } },
    { method: 'POST', path: '/curation-pipelines', answer: { status: 200, body: { pipeline, created: false } } },
    { method: 'POST', path: '/simulate', answer: { status: 200, body: result } },
    { method: 'POST', path: '/query', answer: { status: 200, body: { ...result, truncated: recipe && truncated } } },
    { method: 'POST', path: '/run', answer: (call) => ({ status: 200, body: { notebook: call % 2 ? 'one' : 'two',
      revision: call % 2 ? 'used-one' : 'used-two', columns: ['value'], rows, row_count: 2,
      truncated: truncated && call % 2 === 1, stdout: '', took_ms: 1, app_url: '/notebook-app' } }) },
    { method: 'POST', path: '/datasets', answer: { status: 200, body: receipt } },
    { method: 'POST', path: '/dataset-samples', answer: (call) => unsupportedSamples
      ? { status: 404, body: { message: 'Sample publication is unavailable' } } : failPublish && call === 1
      ? { status: 500, body: { message: 'Publication failed' } } : { status: 200, body: receipt } },
  ]);
  vi.spyOn(window, 'scrollTo').mockImplementation(() => {});
  const root = createRootRoute();
  const p = createRoute({ getParentRoute: () => root, path: '/data-curation/pipeline', validateSearch: pipelineSearch, component: PipelinePage });
  const r = createRoute({ getParentRoute: () => root, path: '/data-curation/recipe', validateSearch: recipeSearch, component: DataCurationPage });
  const router = createRouter({ routeTree: root.addChildren([p, r]), history: createMemoryHistory({ initialEntries: [recipe
    ? '/data-curation/recipe?q=sample-script&dataset=production/output'
    : `/data-curation/pipeline?name=sample-flow${notebookView ? '&view=notebook' : ''}`] }) });
  render(withQueries(<RouterProvider router={router} />));
  await waitFor(() => expect(screen.getByRole('button', { name: recipe ? 'Simulate 25 rows' : 'Preview 25 rows' })).toHaveProperty('disabled', false));
  return { server, router };
}

it('publishes a preview only on explicit sample action, with exact notebook provenance', async () => {
  const { server } = await setup();
  await userEvent.click(screen.getByRole('button', { name: 'Preview 25 rows' }));
  await screen.findByRole('button', { name: 'Publish sample' });
  expect(server.countOf('POST', '/dataset-samples')).toBe(0);
  expect(screen.getByRole('button', { name: /^Publish$/ })).toHaveProperty('disabled', true);
  await userEvent.click(screen.getByRole('button', { name: 'Publish sample' }));
  await screen.findByText(/Sample published:/);
  const sent = server.calls.find((call) => call.method === 'POST' && call.url.endsWith('/dataset-samples'))!.body;
  expect(sent).toMatchObject({ name: 'production/output/samples', items: rows, source: 'test-source', window_seconds: 3600,
    sample: { mode: 'preview', truncated_stages: [] }, produced_by: 'sample-flow@flow-revision' });
  const saved = server.calls.find((call) => call.method === 'POST' && call.url.endsWith('/curation-pipelines'))!.body as typeof pipeline;
  expect(saved.blocks[1]!.spec).toMatchObject({ notebook: 'one', revision: 'used-one' });
  expect(saved.blocks[2]!.spec).toMatchObject({ notebook: 'two', revision: 'used-two' });
  expect(server.countOf('GET', '/notebooks/one')).toBe(0);
});

it.each([false, true])('does not fall back to ordinary publication on an older backend (recipe: %s)', async (recipe) => {
  const { server } = await setup({ recipe, unsupportedSamples: true });
  await userEvent.click(screen.getByRole('button', { name: recipe ? 'Simulate 25 rows' : 'Preview 25 rows' }));
  await userEvent.click(await screen.findByRole('button', { name: 'Publish sample' }));
  await screen.findByText('Sample publication is unavailable');
  expect(server.countOf('POST', '/dataset-samples')).toBe(1);
  expect(server.countOf('POST', '/datasets')).toBe(0);
});

it('marks downstream truncation even if the last notebook itself did not truncate', async () => {
  const { server } = await setup({ truncated: true });
  await userEvent.click(screen.getByRole('button', { name: /^Run$/ }));
  await screen.findByRole('button', { name: 'Publish sample' });
  expect(screen.getByRole('button', { name: /^Publish$/ })).toHaveProperty('disabled', true);
  await userEvent.click(screen.getByRole('button', { name: 'Publish sample' }));
  await waitFor(() => expect(server.countOf('POST', '/dataset-samples')).toBe(1));
  expect(server.calls.find((call) => call.url.endsWith('/dataset-samples'))!.body).toMatchObject({ sample: { mode: 'truncated', truncated_stages: ['python'] } });
});

it('keeps ordinary complete publication on its original name without sample metadata', async () => {
  const { server } = await setup();
  await userEvent.click(screen.getByRole('button', { name: /^Run$/ }));
  await waitFor(() => expect(screen.getByRole('button', { name: /^Publish$/ })).toHaveProperty('disabled', false));
  expect(screen.queryByRole('button', { name: 'Publish sample' })).toBeNull();
  await userEvent.click(screen.getByRole('button', { name: /^Publish$/ }));
  await waitFor(() => expect(server.countOf('POST', '/datasets')).toBe(1));
  const sent = server.calls.find((call) => call.url.endsWith('/datasets'))!.body;
  expect(sent).toMatchObject({ name: 'production/output' });
  expect(sent).not.toHaveProperty('sample');
});

it('retains a sample for retry after publication failure and invalidates it on time changes', async () => {
  const { server } = await setup({ failPublish: true });
  await userEvent.click(screen.getByRole('button', { name: 'Preview 25 rows' }));
  await userEvent.click(await screen.findByRole('button', { name: 'Publish sample' }));
  await screen.findAllByText('Publication failed');
  await userEvent.click(screen.getByRole('button', { name: 'Publish sample' }));
  await screen.findByText(/Sample published:/);
  expect(server.countOf('POST', '/simulate')).toBe(1);
  expect(server.countOf('POST', '/dataset-samples')).toBe(2);
  await userEvent.click(screen.getByRole('button', { name: 'Preview 25 rows' }));
  await screen.findByRole('button', { name: 'Publish sample' });
  expect(screen.queryByText(/Sample published:/)).toBeNull();
  await userEvent.click(screen.getByRole('button', { name: /^1h$/ }));
  await waitFor(() => expect(screen.queryByRole('button', { name: 'Publish sample' })).toBeNull());
});

it('does not publish a notebook prefix as the output of the whole pipeline', async () => {
  const { server } = await setup({ notebookView: true });
  const cell = await screen.findByRole('region', { name: 'Cell 1: Source' });
  await userEvent.click(within(cell).getByRole('button', { name: 'Run to here' }));
  await waitFor(() => expect(server.countOf('POST', '/simulate')).toBe(1));
  expect(screen.queryByRole('button', { name: 'Publish sample' })).toBeNull();
  expect(server.countOf('POST', '/dataset-samples')).toBe(0);
});

it.each([false, true])('publishes recipe preview/truncated results only through a sample action (truncated: %s)', async (truncated) => {
  const { server } = await setup({ recipe: true, truncated });
  await userEvent.click(screen.getByRole('button', { name: truncated ? 'Execute & save dataset' : 'Simulate 25 rows' }));
  await screen.findByRole('button', { name: 'Publish sample' });
  expect(server.countOf('POST', '/dataset-samples')).toBe(0);
  await userEvent.click(screen.getByRole('button', { name: 'Publish sample' }));
  await screen.findByText(/Sample published:/);
  expect(server.calls.find((call) => call.url.endsWith('/dataset-samples'))!.body).toMatchObject({ name: 'production/output/samples', pipeline: 'sample-script', items: rows,
    sample: { mode: truncated ? 'truncated' : 'preview', truncated_stages: truncated ? ['query'] : [] } });
});

it('refuses a stale recipe sample and preserves a current result for retry', async () => {
  const { server } = await setup({ recipe: true, failPublish: true });
  await userEvent.click(screen.getByRole('button', { name: 'Simulate 25 rows' }));
  await screen.findByRole('button', { name: 'Publish sample' });
  fireEvent.change(screen.getByRole('textbox', { name: 'Flow PHP recipe' }), { target: { value: 'changed script' } });
  expect(screen.getByRole('button', { name: 'Publish sample' })).toHaveProperty('disabled', true);
  await userEvent.click(screen.getByRole('button', { name: 'Simulate 25 rows' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Publish sample' })).toHaveProperty('disabled', false));
  await userEvent.click(screen.getByRole('button', { name: 'Publish sample' }));
  await screen.findByText('Publication failed');
  await userEvent.click(screen.getByRole('button', { name: 'Publish sample' }));
  await screen.findByText(/Sample published:/);
  expect(server.countOf('POST', '/simulate')).toBe(2);
  expect(server.countOf('POST', '/dataset-samples')).toBe(2);
});


it('keeps a complete recipe execution on its ordinary dataset path', async () => {
  const { server } = await setup({ recipe: true });
  await userEvent.click(screen.getByRole('button', { name: 'Execute & save dataset' }));
  await waitFor(() => expect(server.countOf('POST', '/datasets')).toBe(1));
  expect(server.calls.find((call) => call.url.endsWith('/datasets'))!.body).toMatchObject({ name: 'production/output', pipeline: 'sample-script' });
  expect(server.calls.find((call) => call.url.endsWith('/datasets'))!.body).not.toHaveProperty('sample');
  expect(screen.queryByRole('button', { name: 'Publish sample' })).toBeNull();
});
