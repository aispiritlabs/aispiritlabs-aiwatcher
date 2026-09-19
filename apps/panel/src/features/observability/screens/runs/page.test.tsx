import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createMemoryHistory, createRootRoute, createRoute, createRouter, RouterProvider } from '@tanstack/react-router';
import { afterEach, expect, it, vi } from 'vitest';
import { RunsPage } from './page';
import { searchSchema } from './search';
import { serve, withQueries } from '@/test/server';

afterEach(() => vi.unstubAllGlobals());

it('loads older runs using the server cursor without replacing the first page', async () => {
  vi.stubGlobal('scrollTo', () => {});
  const run = (id: string) => ({ run_id: id, status: 'succeeded', agents: [], trace_id: id,
    started_at: '2026-09-14T10:00:00Z', last_event_at: '2026-09-14T10:01:00Z', duration_ms: 60_000,
    llm_calls: 1, tool_calls: 0, input_tokens: 1, output_tokens: 2, cached_tokens: 0 });
  serve([{ method: 'GET', path: '/runs', answer: (call) => ({ status: 200, body: {
    runs: [run(call === 1 ? 'newer-run' : 'older-run')], total_known: 2,
    next_cursor: call === 1 ? 'opaque-cursor' : null,
  } }) }]);
  const fetchMock = vi.fn(fetch);
  vi.stubGlobal('fetch', fetchMock);
  const root = createRootRoute();
  const route = createRoute({ getParentRoute: () => root, path: '/observability/runs', validateSearch: searchSchema, component: RunsPage });
  const router = createRouter({ routeTree: root.addChildren([route]), history: createMemoryHistory({ initialEntries: ['/observability/runs?status=succeeded&window=3600'] }) });
  render(withQueries(<RouterProvider router={router} />));
  await screen.findByText('1 loaded · 2 matching');
  await userEvent.click(screen.getByRole('button', { name: 'Load more runs' }));
  await screen.findByText('2 loaded · 2 matching');
  expect(screen.getByRole('link', { name: 'newer-run' })).toBeTruthy();
  expect(screen.getByRole('link', { name: 'older-run' })).toBeTruthy();
  const url = fetchMock.mock.calls.map(([request]) => new URL((request as Request).url)).find((url) => url.searchParams.has('before'));
  expect(url?.searchParams.get('before')).toBe('opaque-cursor');
  expect(url?.searchParams.get('status')).toBe('succeeded');
  expect(url?.searchParams.get('window_seconds')).toBe('3600');
  expect(screen.queryByRole('button', { name: 'Load more runs' })).toBeNull();
});

/**
 * The runs route takes nine axes and this page used to send three, under the
 * API's parameter names rather than the dimensions'. What it sends now is the
 * shared filter, translated once in `shared/lib/object-filter.ts`.
 */
it('sends every axis of the shared filter, and takes one off when its chip is clicked', async () => {
  vi.stubGlobal('scrollTo', () => {});
  serve([
    {
      method: 'GET',
      path: '/runs',
      answer: { status: 200, body: { runs: [], total_known: 0, next_cursor: null } },
    },
  ]);
  const fetchMock = vi.fn(fetch);
  vi.stubGlobal('fetch', fetchMock);
  const root = createRootRoute();
  const route = createRoute({
    getParentRoute: () => root,
    path: '/observability/runs',
    validateSearch: searchSchema,
    component: RunsPage,
  });
  const router = createRouter({
    routeTree: root.addChildren([route]),
    history: createMemoryHistory({
      initialEntries: ['/observability/runs?workflow=house-import&model=opus&session=s-1'],
    }),
  });
  render(withQueries(<RouterProvider router={router} />));

  await screen.findByText('No runs match this filter');
  const first = new URL((fetchMock.mock.calls[0]![0] as Request).url);
  expect(first.searchParams.get('workflow')).toBe('house-import');
  expect(first.searchParams.get('model')).toBe('opus');
  expect(first.searchParams.get('conversation_id')).toBe('s-1');
  // A model selects the runs and narrows nothing inside them: every count on a
  // row is the run's own total, folded when it was ingested. The page says so,
  // because the metrics page's sentence is the opposite and a reader moving
  // between the two is entitled to the difference.
  expect(screen.getByText(/the run's own totals/)).toBeTruthy();

  await userEvent.click(screen.getByRole('button', { name: 'Remove the model filter opus' }));
  // Cleared, not left as an empty list: the patch writes every axis, so the
  // one just taken off leaves the URL rather than lingering in it.
  expect(router.state.location.search).toEqual({
    workflow: ['house-import'],
    session: ['s-1'],
  });
});
