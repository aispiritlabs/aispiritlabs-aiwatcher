import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createMemoryHistory, createRootRoute, createRoute, createRouter, RouterProvider } from '@tanstack/react-router';
import { afterEach, expect, it, vi } from 'vitest';
import { EvaluationPage } from './page';
import { searchSchema } from './search';
import { serve, withQueries } from '@/test/server';

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });
const item = { id: 'case-1', dataset: 'capitals', question: 'Capital?', expected: 'Paris', split: 'test',
  target: { kind: 'trace', trace_id: 'trace-1' }, content: 'written', state: 'ready', proposed_by: 'reviewer' };
async function setup() {
  serve([
    { method: 'GET', path: '/auth/config', answer: { status: 200, body: { enabled: false } } },
    { method: 'GET', path: '/evaluation-suites', answer: { status: 200, body: { suites: [] } } },
    { method: 'GET', path: '/evaluations', answer: { status: 200, body: { evaluations: [], total_known: 0 } } },
    { method: 'GET', path: '/evaluation-results', answer: { status: 200, body: { evaluations: [] } } },
    { method: 'GET', path: '/evaluation-reviews', answer: { status: 200, body: { dataset: 'capitals', items: [item] } } },
  ]);
  vi.spyOn(window, 'scrollTo').mockImplementation(() => {});
  const root = createRootRoute();
  const route = createRoute({ getParentRoute: () => root, path: '/evaluation', validateSearch: searchSchema, component: EvaluationPage });
  const account = createRoute({ getParentRoute: () => root, path: '/account', component: () => <p>Account page</p> });
  const router = createRouter({ routeTree: root.addChildren([route, account]),
    history: createMemoryHistory({ initialEntries: ['/evaluation?reviews=true&review_dataset=capitals'] }) });
  render(withQueries(<RouterProvider router={router} />));
  await screen.findByRole('textbox', { name: 'Expected answer for Capital?' });
  return router;
}

it('guards all dirty review forms once, including hiding review and changing its dataset', async () => {
  const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
  const router = await setup();
  fireEvent.change(screen.getByRole('textbox', { name: 'Question' }), { target: { value: 'New case?' } });
  fireEvent.change(screen.getByRole('textbox', { name: 'Expected answer for Capital?' }), { target: { value: 'London' } });
  expect((screen.getByRole('button', { name: 'Approve' }) as HTMLButtonElement).disabled).toBe(true);
  await act(async () => { void router.navigate({ to: '/evaluation', search: { reviews: false } }); });
  expect(confirm).toHaveBeenCalledOnce();
  expect(screen.getByRole('textbox', { name: 'Question' })).toBeTruthy();
  fireEvent.change(screen.getByRole('textbox', { name: 'Review dataset' }), { target: { value: 'other' } });
  await userEvent.click(screen.getByRole('button', { name: 'Open' }));
  expect(confirm).toHaveBeenCalledTimes(2);
  expect(router.state.location.search.review_dataset).toBe('capitals');
  confirm.mockReturnValue(true);
  await userEvent.click(screen.getByRole('button', { name: 'Open' }));
  await waitFor(() => expect(router.state.location.search.review_dataset).toBe('other'));
  expect((screen.getByRole('textbox', { name: 'Question' }) as HTMLInputElement).value).toBe('');
});

it.each(['proposal', 'expected'] as const)('keeps newer %s edits after an older successful save', async (kind) => {
  const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
  const router = await setup();
  const originalFetch = fetch;
  let finish!: (response: Response) => void;
  vi.stubGlobal('fetch', (input: Request | string, init?: RequestInit) =>
    input instanceof Request && input.method === 'POST' && input.url.includes('/evaluation-reviews')
      ? new Promise<Response>((resolve) => { finish = resolve; }) : originalFetch(input, init));
  const field = screen.getByRole('textbox', { name: kind === 'proposal' ? 'Question' : 'Expected answer for Capital?' });
  fireEvent.change(field, { target: { value: 'submitted' } });
  await userEvent.click(screen.getByRole('button', { name: kind === 'proposal' ? 'Propose as a case' : 'Save expected' }));
  await waitFor(() => expect(finish).toBeTypeOf('function'));
  fireEvent.change(field, { target: { value: 'newer text' } });
  await act(async () => finish(new Response(JSON.stringify(kind === 'proposal'
    ? { review: item, created: true } : { ...item, expected: 'submitted' }),
  { status: 200, headers: { 'Content-Type': 'application/json' } })));
  await waitFor(() => expect((field as HTMLInputElement).value).toBe('newer text'));
  await act(async () => { void router.navigate({ to: '/account' }); });
  expect(confirm).toHaveBeenCalledOnce();
  expect(router.state.location.pathname).toBe('/evaluation');
});
