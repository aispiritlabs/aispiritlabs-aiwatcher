import * as React from 'react';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createBrowserHistory, createMemoryHistory, createRootRoute, createRoute, createRouter, RouterProvider } from '@tanstack/react-router';
import { afterEach, expect, it, vi } from 'vitest';
import { useUnsavedChanges } from './unsaved-changes';

afterEach(() => vi.restoreAllMocks());

function Editor() {
  const [text, setText] = React.useState('');
  const confirmDiscard = useUnsavedChanges({ dirty: text !== '', message: 'Unsaved text.' });
  return <>
    <input aria-label="Draft" value={text} onChange={(event) => setText(event.target.value)} />
    <button onClick={() => { if (confirmDiscard()) setText('replacement'); }}>Replace</button>
  </>;
}

async function setup(browser = false) {
  vi.spyOn(window, 'scrollTo').mockImplementation(() => {});
  const root = createRootRoute();
  const editor = createRoute({ getParentRoute: () => root, path: '/editor', component: Editor });
  const other = createRoute({ getParentRoute: () => root, path: '/account', component: () => <p>Other page</p> });
  if (browser) window.history.replaceState({}, '', '/account');
  const history = browser ? createBrowserHistory()
    : createMemoryHistory({ initialEntries: ['/account', '/editor'], initialIndex: 1 });
  if (browser) { history.push('/editor'); history.flush(); }
  const router = createRouter({ routeTree: root.addChildren([editor, other]), history });
  render(<RouterProvider router={router} />);
  await screen.findByRole('textbox', { name: 'Draft' });
  return router;
}

it('keeps the draft on cancelled replacements and navigation, then allows an explicit discard', async () => {
  const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
  const router = await setup();
  await userEvent.type(screen.getByRole('textbox'), 'my draft');
  await userEvent.click(screen.getByRole('button', { name: 'Replace' }));
  expect((screen.getByRole('textbox') as HTMLInputElement).value).toBe('my draft');
  await act(async () => { void router.navigate({ to: '/account' }); });
  expect(router.state.location.pathname).toBe('/editor');
  expect(confirm).toHaveBeenCalledTimes(2);
  confirm.mockReturnValue(true);
  await act(async () => { void router.navigate({ to: '/account' }); });
  await screen.findByText('Other page');
});

it('protects history back only while a local draft exists', async () => {
  const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
  const router = await setup(true);
  fireEvent.change(screen.getByRole('textbox'), { target: { value: 'draft' } });
  await act(async () => router.history.back());
  await waitFor(() => expect(confirm).toHaveBeenCalledOnce());
  await waitFor(() => expect(window.location.pathname).toBe('/editor'));
  expect(router.state.location.pathname).toBe('/editor');
  fireEvent.change(screen.getByRole('textbox'), { target: { value: '' } });
  await act(async () => router.history.back());
  await waitFor(() => expect(router.state.location.pathname).toBe('/account'));
  expect(confirm).toHaveBeenCalledOnce();
  router.history.destroy();
});

it('registers reload protection with browser history and removes it for a clean draft', async () => {
  const router = await setup(true);
  const unload = () => {
    const event = new Event('beforeunload', { cancelable: true });
    window.dispatchEvent(event);
    return event.defaultPrevented;
  };
  expect(unload()).toBe(false);
  fireEvent.change(screen.getByRole('textbox'), { target: { value: 'draft' } });
  expect(unload()).toBe(true);
  fireEvent.change(screen.getByRole('textbox'), { target: { value: '' } });
  expect(unload()).toBe(false);
  router.history.destroy();
});
