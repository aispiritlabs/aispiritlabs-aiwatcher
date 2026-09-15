import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createBrowserHistory, createMemoryHistory, createRootRoute, createRoute, createRouter, RouterProvider } from '@tanstack/react-router';
import { afterEach, expect, it, vi } from 'vitest';
import { DataCurationPage } from './page';
import { searchSchema } from './search';
import { serve, withQueries } from '@/test/server';

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });
const recipe = { name: 'saved recipe', description: 'Saved description', pipeline: 'saved text',
  engine: 'flow', revision: 'revision-one', saved_at: '2026-09-14T10:00:00Z' };

async function setup(saveStatus = 200, browser = false) {
  serve([
    { method: 'GET', path: '/healthz', answer: { status: 200, body: { engine: 'flow' } } },
    { method: 'GET', path: '/curations', answer: { status: 200, body: { recipes: [recipe] } } },
    { method: 'POST', path: '/check', answer: { status: 200, body: { ok: true, diagnostics: [], checked_by: [] } } },
    { method: 'POST', path: '/curations', answer: { status: saveStatus, body: saveStatus === 200
      ? { recipe, created: true } : { message: 'Save failed' } } },
  ]);
  vi.spyOn(window, 'scrollTo').mockImplementation(() => {});
  const root = createRootRoute();
  const route = createRoute({ getParentRoute: () => root, path: '/data-curation/recipe', validateSearch: searchSchema, component: DataCurationPage });
  const other = createRoute({ getParentRoute: () => root, path: '/account', component: () => <p>Other page</p> });
  if (browser) window.history.replaceState({}, '', '/data-curation/recipe?q=initial');
  const router = createRouter({ routeTree: root.addChildren([route, other]),
    history: browser ? createBrowserHistory() : createMemoryHistory({ initialEntries: ['/data-curation/recipe?q=initial'] }) });
  render(withQueries(<RouterProvider router={router} />));
  await screen.findByRole('button', { name: /saved recipe/ });
  return router;
}

it('protects replacement by another recipe and becomes clean after loading it', async () => {
  const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
  const router = await setup();
  fireEvent.change(screen.getByRole('textbox', { name: 'Description' }), { target: { value: 'my description' } });
  await userEvent.click(screen.getByRole('button', { name: /saved recipe/ }));
  expect((screen.getByRole('textbox', { name: 'Description' }) as HTMLInputElement).value).toBe('my description');
  confirm.mockReturnValue(true);
  await userEvent.click(screen.getByRole('button', { name: /saved recipe/ }));
  expect((screen.getByRole('textbox', { name: 'Flow PHP recipe' }) as HTMLTextAreaElement).value).toBe('saved text');
  confirm.mockClear();
  await act(async () => { void router.navigate({ to: '/account' }); });
  expect(confirm).not.toHaveBeenCalled();
});

it('keeps failed saves dirty and protects the target dataset as part of the draft', async () => {
  const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
  const router = await setup(500);
  fireEvent.change(screen.getByRole('textbox', { name: 'Target dataset' }), { target: { value: 'new target' } });
  await userEvent.click(screen.getByRole('button', { name: 'Save script' }));
  await screen.findByText('Save failed');
  await act(async () => { void router.navigate({ to: '/account' }); });
  expect(confirm).toHaveBeenCalledOnce();
  expect(router.state.location.pathname).toBe('/data-curation/recipe');
});

it.each([false, true])('preserves edits or departure during a successful save (leave: %s)', async (leave) => {
  const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
  const router = await setup();
  const originalFetch = fetch;
  let finish!: (response: Response) => void;
  vi.stubGlobal('fetch', (input: Request | string, init?: RequestInit) =>
    input instanceof Request && input.method === 'POST' && input.url.endsWith('/curations')
      ? new Promise<Response>((resolve) => { finish = resolve; }) : originalFetch(input, init));
  fireEvent.change(screen.getByRole('textbox', { name: 'Description' }), { target: { value: 'submitted' } });
  await userEvent.click(screen.getByRole('button', { name: 'Save script' }));
  await waitFor(() => expect(finish).toBeTypeOf('function'));
  if (leave) {
    confirm.mockReturnValue(true);
    await act(async () => { void router.navigate({ to: '/account' }); });
    await screen.findByText('Other page');
  } else {
    fireEvent.change(screen.getByRole('textbox', { name: 'Description' }), { target: { value: 'newer description' } });
  }
  await act(async () => finish(new Response(JSON.stringify({ recipe, created: true }),
    { status: 200, headers: { 'Content-Type': 'application/json' } })));
  if (leave) {
    expect(router.state.location.pathname).toBe('/account');
    return;
  }
  expect((screen.getByRole('textbox', { name: 'Description' }) as HTMLInputElement).value).toBe('newer description');
  await screen.findByText('Unsaved recipe changes.');
  await act(async () => { void router.navigate({ to: '/account' }); });
  expect(confirm).toHaveBeenCalledOnce();
});


it('restores script context on Back/Forward and asks once before discarding edits', async () => {
  const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
  const router = await setup(200, true);
  try {
    await userEvent.click(screen.getByRole('button', { name: /saved recipe/ }));
    await waitFor(() => expect(router.state.location.search.q).toBe('saved text'));
    expect(router.state.location.search.description).toBe('Saved description');
    fireEvent.change(screen.getByRole('textbox', { name: 'Description' }), { target: { value: 'local edit' } });
    await act(async () => router.history.back());
    await waitFor(() => expect(confirm).toHaveBeenCalledOnce());
    await waitFor(() => expect(window.location.search).toContain('saved'));
    expect(screen.getByRole('textbox', { name: 'Description' })).toHaveProperty('value', 'local edit');
    confirm.mockReturnValue(true);
    await act(async () => router.history.back());
    await waitFor(() => expect(screen.getByRole('textbox', { name: 'Flow PHP recipe' })).toHaveProperty('value', 'initial'));
    expect(screen.getByRole('textbox', { name: 'Description' })).toHaveProperty('value', 'Cases curated from retained production runs.');
    await act(async () => router.history.forward());
    await waitFor(() => expect(screen.getByRole('textbox', { name: 'Flow PHP recipe' })).toHaveProperty('value', 'saved text'));
    expect(screen.getByRole('textbox', { name: 'Description' })).toHaveProperty('value', 'Saved description');
    expect(confirm).toHaveBeenCalledTimes(2);
  } finally { router.history.destroy(); }
});

it('preserves local text for time-only changes, but restores all fields of a new link', async () => {
  const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
  const router = await setup();
  fireEvent.change(screen.getByRole('textbox', { name: 'Flow PHP recipe' }), { target: { value: 'draft' } });
  await act(async () => { void router.navigate({ to: '/data-curation/recipe', search: { q: 'initial', window: 3600 } }); });
  expect(confirm).not.toHaveBeenCalled();
  expect(screen.getByRole('textbox', { name: 'Flow PHP recipe' })).toHaveProperty('value', 'draft');
  const next = { q: 'linked', name: 'linked recipe', dataset: 'linked target', description: 'linked description' };
  await act(async () => { void router.navigate({ to: '/data-curation/recipe', search: next }); });
  expect(confirm).toHaveBeenCalledOnce();
  expect(screen.getByRole('textbox', { name: 'Flow PHP recipe' })).toHaveProperty('value', 'draft');
  confirm.mockReturnValue(true);
  await act(async () => { void router.navigate({ to: '/data-curation/recipe', search: next }); });
  await waitFor(() => expect(screen.getByRole('textbox', { name: 'Flow PHP recipe' })).toHaveProperty('value', 'linked'));
  expect(screen.getByRole('textbox', { name: 'Recipe name' })).toHaveProperty('value', 'linked recipe');
  expect(screen.getByRole('textbox', { name: 'Target dataset' })).toHaveProperty('value', 'linked target');
  expect(screen.getByRole('textbox', { name: 'Description' })).toHaveProperty('value', 'linked description');
});

it('ignores a successful save after navigating to another script in the same route', async () => {
  vi.spyOn(window, 'confirm').mockReturnValue(true);
  const router = await setup();
  const originalFetch = fetch;
  let finish!: (response: Response) => void;
  vi.stubGlobal('fetch', (input: Request | string, init?: RequestInit) =>
    input instanceof Request && input.method === 'POST' && input.url.endsWith('/curations')
      ? new Promise<Response>((resolve) => { finish = resolve; }) : originalFetch(input, init));
  fireEvent.change(screen.getByRole('textbox', { name: 'Flow PHP recipe' }), { target: { value: 'submitted' } });
  await userEvent.click(screen.getByRole('button', { name: 'Save script' }));
  await waitFor(() => expect(finish).toBeTypeOf('function'));
  await act(async () => { void router.navigate({ to: '/data-curation/recipe', search: { q: 'another' } }); });
  await waitFor(() => expect(screen.getByRole('textbox', { name: 'Flow PHP recipe' })).toHaveProperty('value', 'another'));
  await act(async () => finish(new Response(JSON.stringify({ recipe, created: true }),
    { status: 200, headers: { 'Content-Type': 'application/json' } })));
  expect(router.state.location.search.q).toBe('another');
  expect(screen.queryByText('Unsaved recipe changes.')).toBeNull();
});
