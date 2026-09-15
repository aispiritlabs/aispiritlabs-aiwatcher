import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createMemoryHistory, createRootRoute, createRoute, createRouter, RouterProvider } from '@tanstack/react-router';
import { afterEach, expect, it, vi } from 'vitest';
import { PromptPage } from './page';
import { searchSchema } from './search';
import { serve, withQueries } from '@/test/server';

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });
const version = (id: string) => ({ name: 'triage', version_id: id, origin: 'authored', text: `Prompt ${id}`,
  variables: [], created_at: '2026-09-14T10:00:00Z' });
async function setup() {
  serve([
    { method: 'GET', path: '/auth/config', answer: { status: 200, body: { enabled: false } } },
    { method: 'GET', path: '/prompts/triage', answer: { status: 200, body: {
      head: { name: 'triage', labels: { production: 'one' }, versions: [version('one'), version('two')],
        optimizations: [], updated_at: '2026-09-14T10:00:00Z' }, current: version('one'),
    } } },
    { method: 'GET', path: '/versions/two', answer: { status: 200, body: version('two') } },
    { method: 'GET', path: '/versions/one', answer: { status: 200, body: version('one') } },
    { method: 'GET', path: '/versions/new', answer: { status: 200, body: version('new') } },
  ]);
  vi.spyOn(window, 'scrollTo').mockImplementation(() => {});
  const root = createRootRoute();
  const route = createRoute({ getParentRoute: () => root, path: '/prompts/$name', validateSearch: searchSchema, component: PromptPage });
  const account = createRoute({ getParentRoute: () => root, path: '/account', component: () => <p>Account page</p> });
  const router = createRouter({ routeTree: root.addChildren([route, account]),
    history: createMemoryHistory({ initialEntries: ['/prompts/triage?version=one'] }) });
  render(withQueries(<RouterProvider router={router} />));
  const open = await screen.findByRole('button', { name: 'New version' });
  await waitFor(() => expect((open as HTMLButtonElement).disabled).toBe(false));
  await userEvent.click(open);
  await screen.findByRole('textbox', { name: 'Prompt text' });
  return router;
}

it('protects Cancel, the editor toggle and version navigation, and loads the accepted target', async () => {
  const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
  const router = await setup();
  fireEvent.change(screen.getByRole('textbox', { name: 'Prompt text' }), { target: { value: 'my draft' } });
  await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  await userEvent.click(screen.getByRole('button', { name: 'New version' }));
  await act(async () => { void router.navigate({ to: '/prompts/$name', params: { name: 'triage' }, search: { version: 'two' } }); });
  await act(async () => { void router.navigate({ to: '/prompts/$name', params: { name: 'triage' }, search: {} }); });
  expect(confirm).toHaveBeenCalledTimes(4);
  expect((screen.getByRole('textbox', { name: 'Prompt text' }) as HTMLTextAreaElement).value).toBe('my draft');
  confirm.mockReturnValue(true);
  await act(async () => { void router.navigate({ to: '/prompts/$name', params: { name: 'triage' }, search: { version: 'two' } }); });
  await waitFor(() => expect((screen.getByRole('textbox', { name: 'Prompt text' }) as HTMLTextAreaElement).value).toBe('Prompt two'));
});

it.each(['refused', 'leave', 'saved'])('handles publication without losing context (%s)', async (outcome) => {
  const leave = outcome === 'leave';
  const confirm = vi.spyOn(window, 'confirm').mockReturnValue(leave);
  const router = await setup();
  const originalFetch = fetch;
  let finish!: (response: Response) => void;
  vi.stubGlobal('fetch', (input: Request | string, init?: RequestInit) =>
    input instanceof Request && input.method === 'POST' && input.url.endsWith('/prompts')
      ? new Promise<Response>((resolve) => { finish = resolve; }) : originalFetch(input, init));
  fireEvent.change(screen.getByRole('textbox', { name: 'Prompt text' }), { target: { value: 'new version' } });
  await userEvent.click(screen.getByRole('button', { name: 'Publish version' }));
  await waitFor(() => expect(finish).toBeTypeOf('function'));
  expect(screen.getByRole('textbox', { name: 'Prompt text' }).closest('fieldset')?.disabled).toBe(true);
  if (leave) {
    await act(async () => { void router.navigate({ to: '/account' }); });
    await screen.findByText('Account page');
  }
  await act(async () => finish(new Response(JSON.stringify(outcome !== 'refused'
    ? { version: version('new'), created: true } : { message: 'Publication refused' }),
  { status: outcome !== 'refused' ? 201 : 422, headers: { 'Content-Type': 'application/json' } })));
  if (leave) {
    expect(router.state.location.pathname).toBe('/account');
  } else if (outcome === 'saved') {
    await waitFor(() => expect(router.state.location.search.version).toBe('new'));
    expect(screen.queryByRole('textbox', { name: 'Prompt text' })).toBeNull();
    expect(confirm).not.toHaveBeenCalled();
  } else {
    await screen.findByText('Publication refused');
    expect((screen.getByRole('textbox', { name: 'Prompt text' }) as HTMLTextAreaElement).value).toBe('new version');
    await act(async () => { void router.navigate({ to: '/account' }); });
    expect(confirm).toHaveBeenCalledOnce();
    expect(router.state.location.pathname).toBe('/prompts/triage');
  }
});
