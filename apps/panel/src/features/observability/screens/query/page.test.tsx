import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createMemoryHistory, createRootRoute, createRoute, createRouter, RouterProvider } from '@tanstack/react-router';
import { afterEach, expect, it, vi } from 'vitest';
import { QueryPage } from './page';
import { searchSchema } from './search';
import { serve, withQueries } from '@/test/server';

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });

async function setup() {
  serve([
    { method: 'GET', path: '/healthz', answer: { status: 200, body: { engine: 'flow' } } },
    { method: 'GET', path: '/datasets', answer: { status: 200, body: { datasets: [], source: 'test', max_rows: 1000 } } },
    { method: 'POST', path: '/check', answer: { status: 200, body: { ok: true, diagnostics: [], checked_by: [] } } },
  ]);
  vi.spyOn(window, 'scrollTo').mockImplementation(() => {});
  const root = createRootRoute();
  const route = createRoute({ getParentRoute: () => root, path: '/observability/query', validateSearch: searchSchema, component: QueryPage });
  const other = createRoute({ getParentRoute: () => root, path: '/account', component: () => <p>Other page</p> });
  const router = createRouter({ routeTree: root.addChildren([route, other]),
    history: createMemoryHistory({ initialEntries: ['/observability/query?mode=write&q=initial'] }) });
  render(withQueries(<RouterProvider router={router} />));
  await screen.findByRole('textbox', { name: 'Flow PHP query' });
  return router;
}

it('keeps written text across view changes and guards a different query link', async () => {
  const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
  const router = await setup();
  fireEvent.change(screen.getByRole('textbox'), { target: { value: 'my query' } });
  await act(async () => { void router.navigate({ to: '/observability/query', search: { mode: 'write', q: 'initial', window: 86400 } }); });
  expect(confirm).not.toHaveBeenCalled();
  expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('my query');
  await act(async () => { void router.navigate({ to: '/observability/query', search: { mode: 'write', q: 'different' } }); });
  expect(confirm).toHaveBeenCalledOnce();
  expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('my query');
  confirm.mockReturnValue(true);
  await act(async () => { void router.navigate({ to: '/observability/query', search: { mode: 'write', q: 'different' } }); });
  await waitFor(() => expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('different'));
});

it.each([false, true])('preserves edits or departure while Run is waiting (leave: %s)', async (leave) => {
  const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
  const router = await setup();
  const originalFetch = fetch;
  let resolveRun!: (response: Response) => void;
  vi.stubGlobal('fetch', (input: Request | string, init?: RequestInit) =>
    String(input).endsWith('/query/query')
      ? new Promise<Response>((resolve) => { resolveRun = resolve; }) : originalFetch(input, init));
  fireEvent.change(screen.getByRole('textbox'), { target: { value: 'submitted' } });
  await userEvent.click(screen.getByRole('button', { name: /^Run / }));
  await waitFor(() => expect(resolveRun).toBeTypeOf('function'));
  if (leave) {
    confirm.mockReturnValue(true);
    await act(async () => { void router.navigate({ to: '/account' }); });
    await screen.findByText('Other page');
  } else {
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'newer draft' } });
  }
  await act(async () => resolveRun(new Response(JSON.stringify({ columns: [], rows: [], row_count: 0,
    truncated: false, truncate_cells: false, dataset: null, grain: null, source: 'test', took_ms: 1 }),
  { status: 200, headers: { 'Content-Type': 'application/json' } })));
  if (leave) {
    expect(router.state.location.pathname).toBe('/account');
    return;
  }
  expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('newer draft');
  expect(router.state.location.search.q).toBe('initial');
  await act(async () => { void router.navigate({ to: '/account' }); });
  expect(confirm).toHaveBeenCalledOnce();
  expect(router.state.location.pathname).toBe('/observability/query');
});
