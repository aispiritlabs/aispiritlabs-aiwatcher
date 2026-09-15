import { render, screen } from '@testing-library/react';
import { createMemoryHistory, createRootRoute, createRoute, createRouter, RouterProvider } from '@tanstack/react-router';
import { afterEach, expect, it, vi } from 'vitest';
import { DatasetExplorer } from './dataset-explorer';
import { serve, withQueries } from '@/test/server';

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });

it('shows sample status, truncation provenance and version label after reopening a dataset', async () => {
  const version = { version: 'sample-pin', row_count: 1, columns: ['value'], created_at: '2026-09-14T10:00:00Z',
    sample: { mode: 'preview' as const, truncated_stages: ['python'] } };
  const dataset = { name: 'output/samples', description: '', latest: version, versions: [version] };
  serve([{ method: 'GET', path: '/dataset-rows', answer: { status: 200, body: {
    name: dataset.name, version, pipeline: 'script', source: 'test-source', engine: 'flow',
    rows: [{ row_index: 0, row: { value: 'one' } }], total_rows: 1, matching_rows: 1, offset: 0, limit: 50,
  } } }]);
  vi.spyOn(window, 'scrollTo').mockImplementation(() => {});
  const root = createRootRoute();
  const route = createRoute({ getParentRoute: () => root, path: '/datasets', component: () =>
    <DatasetExplorer dataset={dataset} versionId="sample-pin" view="rows"
      onVersionChange={() => {}} onViewChange={() => {}} onSearchChange={() => {}} /> });
  const router = createRouter({ routeTree: root.addChildren([route]), history: createMemoryHistory({ initialEntries: ['/datasets'] }) });
  render(withQueries(<RouterProvider router={router} />));
  await screen.findByText('Preview sample');
  expect(screen.getByRole('option', { name: /Sample · sample-pin/ })).toBeTruthy();
  expect(screen.getByText(/Truncated stages: python/)).toBeTruthy();
  expect(screen.getByText(/not a random or representative sample/)).toBeTruthy();
  await screen.findByText('one');
});
