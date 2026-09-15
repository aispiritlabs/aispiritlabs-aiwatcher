import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createMemoryHistory, createRootRoute, createRoute, createRouter, RouterProvider } from '@tanstack/react-router';
import { afterEach, expect, it, vi } from 'vitest';
import { ReviewPage } from './page';
import { searchSchema } from './search';
import { serve, withQueries } from '@/test/server';

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });
const turn = (id: string) => ({ conversation_id: 'first', turn_id: id, message_id: id, role: 'assistant',
  state: 'archived', content_bytes: 10, policy: {}, parts: [], review: { state: 'pending', note: '', preference: null } });
async function setup() {
  serve([
    { method: 'GET', path: '/conversation-policy', answer: { status: 200, body: { mode: 'protected', key_ids: [], max_ttl_days: 30 } } },
    { method: 'GET', path: '/conversation-archive', answer: { status: 200, body: { conversations: [
      { conversation_id: 'first', findings: {} }, { conversation_id: 'second', findings: {} },
    ] } } },
    { method: 'GET', path: '/conversation-turns', answer: { status: 200, body: { turns: [turn('one'), turn('two')] } } },
  ]);
  vi.spyOn(window, 'scrollTo').mockImplementation(() => {});
  const root = createRootRoute();
  const route = createRoute({ getParentRoute: () => root, path: '/conversations/review', validateSearch: searchSchema, component: ReviewPage });
  const account = createRoute({ getParentRoute: () => root, path: '/account', component: () => <p>Account page</p> });
  const router = createRouter({ routeTree: root.addChildren([route, account]),
    history: createMemoryHistory({ initialEntries: ['/conversations/review'] }) });
  render(withQueries(<RouterProvider router={router} />));
  await screen.findByRole('textbox', { name: 'Review note for one' });
  return router;
}

it('pins the conversation and protects multiple notes with a single decision per transition', async () => {
  const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
  const router = await setup();
  await waitFor(() => expect(router.state.location.search.conversation).toBe('first'));
  fireEvent.change(screen.getByRole('textbox', { name: 'Review note for one' }), { target: { value: 'first draft' } });
  fireEvent.change(screen.getByRole('textbox', { name: 'Review note for two' }), { target: { value: 'second draft' } });
  await act(async () => { void router.navigate({ to: '/conversations/review', search: { conversation: 'second' } }); });
  expect(confirm).toHaveBeenCalledOnce();
  expect(router.state.location.search.conversation).toBe('first');
  await act(async () => { void router.navigate({ to: '/conversations/review', search: { conversation: 'first', review: 'approved' } }); });
  expect(confirm).toHaveBeenCalledTimes(2);
  confirm.mockReturnValue(true);
  await act(async () => { void router.navigate({ to: '/account' }); });
  await screen.findByText('Account page');
});

it.each([200, 422])('keeps notes after a review response and reports the right dirty state (%s)', async (status) => {
  const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
  const router = await setup();
  const originalFetch = fetch;
  let finish!: (response: Response) => void;
  vi.stubGlobal('fetch', (input: Request | string, init?: RequestInit) =>
    input instanceof Request && input.method === 'POST'
      ? new Promise<Response>((resolve) => { finish = resolve; }) : originalFetch(input, init));
  const field = screen.getByRole('textbox', { name: 'Review note for one' });
  fireEvent.change(field, { target: { value: 'review note' } });
  await userEvent.click(screen.getAllByRole('button', { name: 'Approve' })[0]!);
  await waitFor(() => expect(finish).toBeTypeOf('function'));
  expect(field.closest('fieldset')?.disabled).toBe(true);
  await act(async () => finish(new Response(JSON.stringify(status === 200 ? turn('one') : { message: 'Review refused' }),
    { status, headers: { 'Content-Type': 'application/json' } })));
  await waitFor(() => expect(field.closest('fieldset')?.disabled).toBe(false));
  expect((field as HTMLInputElement).value).toBe('review note');
  await act(async () => { void router.navigate({ to: '/account' }); });
  if (status === 200) {
    await screen.findByText('Account page');
    expect(confirm).not.toHaveBeenCalled();
  } else {
    expect(confirm).toHaveBeenCalledOnce();
    expect(router.state.location.pathname).toBe('/conversations/review');
  }
});
