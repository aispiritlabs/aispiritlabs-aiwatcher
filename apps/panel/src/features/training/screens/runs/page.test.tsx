import { render, screen, waitFor, act } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import {
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
  RouterProvider,
} from '@tanstack/react-router';
import { afterEach, expect, it, vi } from 'vitest';
import { RunsPage } from './page';
import { searchSchema } from './search';
import { serve, withQueries } from '@/test/server';

afterEach(() => vi.unstubAllGlobals());

it('compares selected details beyond the list and restores selection through Back and reload', async () => {
  const storage = new Map<string, string>();
  vi.stubGlobal('localStorage', {
    getItem: (key: string) => storage.get(key) ?? null,
    setItem: (key: string, value: string) => storage.set(key, value),
  });
  vi.stubGlobal('IS_REACT_ACT_ENVIRONMENT', true);
  vi.stubGlobal('scrollTo', () => {});
  vi.stubGlobal(
    'ResizeObserver',
    class {
      observe() {}
      disconnect() {}
    },
  );
  const run = (id: string) => ({
    run_id: id,
    model: 'model',
    dataset: 'data',
    reproducible: false,
    status: 'succeeded',
    params: { lr: id === 'a' ? 0.1 : 0.2 },
    started_at: '2026-09-11T09:00:00Z',
    last_heard_from: '2026-09-11T09:01:00Z',
    epochs: [
      { epoch: 0, metrics: { loss: 0.4 } },
      { epoch: 1, metrics: id === 'a' ? { loss: 0.2 } : {} },
    ],
  });
  const server = serve([
    { method: 'GET', path: '/auth/config', answer: { status: 200, body: { enabled: false } } },
    {
      method: 'GET',
      path: '/training-runs',
      answer: { status: 200, body: { runs: [{ ...run('a'), epochs: 2 }], total: 101 } },
    },
    ...['a', 'b'].map((id) => ({
      method: 'GET',
      path: `/training-runs/${id}`,
      answer: { status: 200, body: run(id) },
    })),
    {
      method: 'GET',
      path: '/training-runs/missing',
      answer: { status: 404, body: { code: 'not_found' } },
    },
  ]);
  const root = createRootRoute();
  const route = createRoute({
    getParentRoute: () => root,
    path: '/training/runs',
    validateSearch: searchSchema,
    component: RunsPage,
  });
  const router = createRouter({
    routeTree: root.addChildren([route]),
    history: createMemoryHistory({
      initialEntries: ['/training/runs?runs=%5B%22a%22%2C%22b%22%2C%22missing%22%5D'],
    }),
  });
  render(withQueries(<RouterProvider router={router} />));
  await screen.findByText('Training comparison');
  await screen.findByText(/Could not load missing/);
  expect(screen.getByText(/not the full history/)).toBeTruthy();
  expect(server.countOf('GET', '/training-runs/b')).toBe(1);
  expect(server.countOf('GET', '/training-runs/missing')).toBe(1);
  await userEvent.click(screen.getByRole('button', { name: 'Remove missing' }));
  await waitFor(() => expect(router.state.location.search.runs).toEqual(['a', 'b']));
  await act(() => router.history.back());
  await waitFor(() => expect(screen.getByRole('button', { name: 'Remove missing' })).toBeTruthy());
  await act(() => router.load());
  expect(router.state.location.search.runs).toEqual(['a', 'b', 'missing']);
  await userEvent.type(screen.getByLabelText('View name'), 'Two runs');
  await userEvent.click(screen.getByRole('button', { name: 'Save view' }));
  await screen.findByText('Saved “Two runs” on this device.');
});

it('deduplicates URL IDs and enforces the five-run ceiling', () => {
  expect(searchSchema.parse({ runs: ['a', 'a', 'b', 'c', 'd', 'e', 'f'] }).runs).toEqual([
    'a',
    'b',
    'c',
    'd',
    'e',
  ]);
});
