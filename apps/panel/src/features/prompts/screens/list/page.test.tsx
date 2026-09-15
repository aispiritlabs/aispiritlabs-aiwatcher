import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createMemoryHistory, createRootRoute, createRoute, createRouter, RouterProvider } from '@tanstack/react-router';
import { afterEach, expect, it, vi } from 'vitest';
import { PromptsPage } from './page';
import { searchSchema } from './search';
import { searchSchema as detailSearch } from '../detail/search';
import { serve, withQueries } from '@/test/server';

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });
async function setup() {
  serve([{ method: 'GET', path: '/prompts', answer: { status: 200, body: { prompts: [], total: 0 } } }]);
  vi.spyOn(window, 'scrollTo').mockImplementation(() => {});
  const root = createRootRoute();
  const route = createRoute({ getParentRoute: () => root, path: '/prompts/', validateSearch: searchSchema, component: PromptsPage });
  const detail = createRoute({ getParentRoute: () => root, path: '/prompts/$name', validateSearch: detailSearch, component: () => <p>Published prompt</p> });
  const account = createRoute({ getParentRoute: () => root, path: '/account', component: () => <p>Account page</p> });
  const router = createRouter({ routeTree: root.addChildren([route, detail, account]),
    history: createMemoryHistory({ initialEntries: ['/prompts/'] }) });
  render(withQueries(<RouterProvider router={router} />));
  await userEvent.click(await screen.findByRole('button', { name: 'New prompt' }));
  await screen.findByRole('textbox', { name: 'Prompt name' });
  return router;
}

it('protects a new prompt on Cancel, toggle and route navigation', async () => {
  const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
  const router = await setup();
  fireEvent.change(screen.getByRole('textbox', { name: 'Prompt name' }), { target: { value: 'triage' } });
  await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  await userEvent.click(screen.getByRole('button', { name: 'New prompt' }));
  await act(async () => { void router.navigate({ to: '/account' }); });
  expect(confirm).toHaveBeenCalledTimes(3);
  expect((screen.getByRole('textbox', { name: 'Prompt name' }) as HTMLInputElement).value).toBe('triage');
  confirm.mockReturnValue(true);
  await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(screen.queryByRole('textbox', { name: 'Prompt name' })).toBeNull();
});

it.each([false, true])('opens the published version unless the user has left (leave: %s)', async (leave) => {
  const confirm = vi.spyOn(window, 'confirm').mockReturnValue(leave);
  const router = await setup();
  const originalFetch = fetch;
  let finish!: (response: Response) => void;
  vi.stubGlobal('fetch', (input: Request | string, init?: RequestInit) =>
    input instanceof Request && input.method === 'POST'
      ? new Promise<Response>((resolve) => { finish = resolve; }) : originalFetch(input, init));
  fireEvent.change(screen.getByRole('textbox', { name: 'Prompt name' }), { target: { value: 'triage' } });
  fireEvent.change(screen.getByRole('textbox', { name: 'Prompt text' }), { target: { value: 'Classify this.' } });
  await userEvent.click(screen.getByRole('button', { name: 'Publish' }));
  await waitFor(() => expect(finish).toBeTypeOf('function'));
  if (leave) {
    await act(async () => { void router.navigate({ to: '/account' }); });
    await screen.findByText('Account page');
  }
  await act(async () => finish(new Response(JSON.stringify({ version: { name: 'triage', version_id: 'new' }, created: true }),
    { status: 201, headers: { 'Content-Type': 'application/json' } })));
  if (leave) {
    expect(router.state.location.pathname).toBe('/account');
  } else {
    await screen.findByText('Published prompt');
    expect(router.state.location.search.version).toBe('new');
    expect(confirm).not.toHaveBeenCalled();
  }
});
