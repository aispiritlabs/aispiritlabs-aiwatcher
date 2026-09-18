import * as React from 'react';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createMemoryHistory, createRootRoute, createRoute, createRouter, RouterProvider } from '@tanstack/react-router';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { RootLayout } from '@/app/shell';
import { WorkspacePage } from '@/app/workspace';
import { isPanelHref, readNavigationPreferences, shellPolicy } from '@/app/navigation-preferences';
import { useUnsavedChanges } from '@/shared/lib/unsaved-changes';

const scope = vi.hoisted(() => ({ value: 'test-instance:alice' as string | undefined, listeners: new Set<() => void>() }));
vi.mock('@/shared/lib/local-views', async () => {
  const { useSyncExternalStore } = await import('react');
  return { useLocalScope: () => useSyncExternalStore((listener) => {
    scope.listeners.add(listener);
    return () => { scope.listeners.delete(listener); };
  }, () => scope.value) };
});
vi.mock('@/shared/components/appearance', () => ({ Appearance: () => null }));
vi.mock('@/shared/components/user-menu', () => ({ UserMenu: () => null }));
vi.mock('@/app/command-panel', () => ({ CommandPanel: () => null, useCommandPanel: () => [false, vi.fn()] }));

let storage: Map<string, string>;
const key = 'test-instance:alice:navigation';
beforeEach(() => {
  scope.value = 'test-instance:alice';
  storage = new Map();
  vi.stubGlobal('localStorage', {
    getItem: vi.fn((key: string) => storage.get(key) ?? null),
    setItem: vi.fn((key: string, value: string) => { storage.set(key, value); }),
  });
  vi.spyOn(window, 'scrollTo').mockImplementation(() => {});
});
afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); vi.unstubAllEnvs(); });

function Editor() {
  const [draft, setDraft] = React.useState('');
  useUnsavedChanges({ dirty: !!draft, message: 'Discard draft?' });
  return <input aria-label="Draft" value={draft} onChange={(event) => setDraft(event.target.value)} />;
}

async function setup(href = '/?start=workspace') {
  const root = createRootRoute({ component: RootLayout });
  const index = createRoute({ getParentRoute: () => root, path: '/', component: WorkspacePage });
  const pipeline = createRoute({ getParentRoute: () => root, path: '/data-curation/pipeline', component: Editor });
  const runs = createRoute({ getParentRoute: () => root, path: '/observability/runs', component: () => <h1>Run history</h1> });
  const datasets = createRoute({ getParentRoute: () => root, path: '/datasets', component: () => <h1>Dataset catalog</h1> });
  const account = createRoute({ getParentRoute: () => root, path: '/account', component: () => <h1>Profile</h1> });
  const router = createRouter({ routeTree: root.addChildren([index, pipeline, runs, datasets, account]),
    history: createMemoryHistory({ initialEntries: [href] }) });
  const rendered = render(<React.StrictMode><RouterProvider router={router} /></React.StrictMode>);
  await waitFor(() => expect(router.state.isLoading).toBe(false));
  await screen.findByRole('main');
  return { router, ...rendered };
}

function savePreferences(change = {}) {
  storage.set(key, JSON.stringify({ ...readNavigationPreferences(null), ...change }));
}

describe('navigation preference contract', () => {
  it('imports the old sidebar preference without changing existing saved views or the source', async () => {
    storage.set('aiwatcher.sidebar', 'collapsed');
    storage.set('test-instance:alice:views', '[{"schema_version":1}]');
    await setup();
    await userEvent.click(screen.getByRole('button', { name: 'Expand the sidebar' }));
    expect(JSON.parse(storage.get(key)!)).toMatchObject({ schema_version: 1, collapsed: false });
    expect(storage.get('aiwatcher.sidebar')).toBe('collapsed');
    expect(storage.get('test-instance:alice:views')).toBe('[{"schema_version":1}]');
  });

  it.each(['{broken', '{"schema_version":99}'])('preserves unreadable/future schema %s and allows a session rollback', async (raw) => {
    storage.set(key, raw);
    await setup();
    expect(screen.getByRole('status').textContent).toContain('Stored data was left unchanged');
    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Navigation layout' }), 'classic');
    expect(storage.get(key)).toBe(raw);
    expect((screen.getByRole('combobox', { name: 'Navigation layout' }) as HTMLSelectElement).value).toBe('classic');
    expect(screen.getByRole('status').textContent).toContain('Applied for this session');
  });

  it('keeps consecutive session edits when local storage is blocked', async () => {
    vi.mocked(window.localStorage.setItem).mockImplementation(() => { throw new Error('quota'); });
    await setup();
    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Navigation layout' }), 'classic');
    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Start page' }), '/datasets');
    expect((screen.getByRole('combobox', { name: 'Navigation layout' }) as HTMLSelectElement).value).toBe('classic');
    expect((screen.getByRole('combobox', { name: 'Start page' }) as HTMLSelectElement).value).toBe('/datasets');
    expect(storage.has(key)).toBe(false);
  });

  it('rejects external URLs, unknown starts and duplicate pins', () => {
    for (const href of ['https://other.test/', '//other.test/', '/\\other.test/', '/api/v1/auth/logout', '/unknown', '/runs/abc\n']) {
      expect(isPanelHref(href), href).toBe(false);
    }
    expect(isPanelHref('/prompts/team%2Fprompt?version=v1&as_of=123#details')).toBe(true);
    expect(() => readNavigationPreferences(JSON.stringify({ ...readNavigationPreferences(null), start: '//other.test' }))).toThrow();
    expect(() => readNavigationPreferences(JSON.stringify({ ...readNavigationPreferences(null), pins: [{ href: '/datasets', label: 'Data' }, { href: '/datasets', label: 'Again' }] }))).toThrow();
  });

  it('merges unrelated changes from another tab and reloads storage events', async () => {
    await setup();
    savePreferences({ pins: [{ label: 'Runs', href: '/observability/runs?status=failed' }] });
    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Start page' }), '/datasets');
    expect(JSON.parse(storage.get(key)!).pins).toHaveLength(1);
    savePreferences({ start: '/observability/runs', shell: 'classic' });
    act(() => window.dispatchEvent(new StorageEvent('storage', { key })));
    expect((screen.getByRole('combobox', { name: 'Start page' }) as HTMLSelectElement).value).toBe('/observability/runs');
    expect((screen.getByRole('combobox', { name: 'Navigation layout' }) as HTMLSelectElement).value).toBe('classic');
  });

  it('clears pins and preferences immediately when identity changes or becomes unavailable', async () => {
    savePreferences({ pins: [{ label: 'Alice private view', href: '/datasets?name=alice' }] });
    await setup();
    expect(screen.getAllByText('Alice private view').length).toBeGreaterThan(0);
    act(() => { scope.value = 'test-instance:bob'; [...scope.listeners].forEach((listener) => listener()); });
    expect(screen.queryByText('Alice private view')).toBeNull();
    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Start page' }), '/datasets');
    expect(JSON.parse(storage.get(key)!).start).toBe('/');
    expect(JSON.parse(storage.get('test-instance:bob:navigation')!).start).toBe('/datasets');
    act(() => { scope.value = undefined; [...scope.listeners].forEach((listener) => listener()); });
    expect((screen.getByRole('combobox', { name: 'Start page' }) as HTMLSelectElement).disabled).toBe(true);
  });
});

describe('shell rollout and navigation', () => {
  it('uses the preferred start on root entry, preserves deep links, and leaves an explicit route back to Your work', async () => {
    savePreferences({ start: '/datasets' });
    const first = await setup('/');
    await screen.findByRole('heading', { name: 'Dataset catalog' });
    expect(first.router.state.location.pathname).toBe('/datasets');
    await userEvent.click(within(screen.getByRole('navigation', { name: 'Main navigation' })).getByRole('link', { name: 'Your work' }));
    await screen.findByRole('heading', { name: 'Your work' });
    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Start page' }), '/observability/runs');
    expect(first.router.state.location.pathname).toBe('/');
    cleanup();
    const second = await setup('/observability/runs?status=failed&window=3600#case');
    expect(second.router.state.location.href).toBe('/observability/runs?status=failed&window=3600#case');
  });

  it('rolls back and forward without remounting a dirty editor or changing its URL', async () => {
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
    const { router } = await setup('/data-curation/pipeline?name=demo&revision=v1&as_of=123');
    const input = await screen.findByRole('textbox', { name: 'Draft' });
    fireEvent.change(input, { target: { value: 'my unsaved pipeline' } });
    await userEvent.click(screen.getByText('Navigation layout', { selector: 'summary' }));
    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Navigation layout' }), 'classic');
    expect(screen.getByRole('textbox', { name: 'Draft' })).toBe(input);
    expect((input as HTMLInputElement).value).toBe('my unsaved pipeline');
    expect(router.state.location.href).toBe('/data-curation/pipeline?name=demo&revision=v1&as_of=123');
    expect(within(screen.getByRole('navigation', { name: 'Main navigation' })).queryByRole('link', { name: 'Prompts' })).toBeNull();
    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Navigation layout' }), 'new');
    expect(screen.getByRole('textbox', { name: 'Draft' })).toBe(input);
    expect(within(screen.getByRole('navigation', { name: 'Main navigation' })).getByRole('link', { name: 'Prompts' })).toBeDefined();
    expect(confirm).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('link', { name: 'Navigation preferences' }));
    expect(confirm).toHaveBeenCalledOnce();
    expect(router.state.location.pathname).toBe('/data-curation/pipeline');
  });

  it('honours a forced deployment rollback without overwriting the user preference', async () => {
    vi.stubEnv('VITE_AIWATCHER_SHELL', 'classic');
    savePreferences({ shell: 'new' });
    await setup();
    const select = screen.getByRole('combobox', { name: 'Navigation layout' }) as HTMLSelectElement;
    expect(select.value).toBe('classic');
    expect(select.disabled).toBe(true);
    expect(JSON.parse(storage.get(key)!).shell).toBe('new');
    expect(shellPolicy('new')).toBe('new');
    expect(shellPolicy('invalid')).toBe('user');
  });

  it('restores a named pin with revision, filters and hash after reload in either layout', async () => {
    const href = '/data-curation/pipeline?name=team%2Fdemo&revision=v1&window=3600&as_of=123#evidence';
    await setup(href);
    await userEvent.click(screen.getByText('Pin this view', { selector: 'summary' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Pinned view name' }), { target: { value: 'September baseline' } });
    await userEvent.click(screen.getByRole('button', { name: 'Pin view' }));
    expect(JSON.parse(storage.get(key)!).pins).toEqual([{ href, label: 'September baseline' }]);
    cleanup();
    savePreferences({ ...JSON.parse(storage.get(key)!), shell: 'classic' });
    const { router } = await setup();
    await userEvent.click(within(screen.getByRole('main')).getByRole('link', { name: /September baseline/ }));
    await screen.findByRole('textbox', { name: 'Draft' });
    expect(router.state.location.href).toBe(href);
    await userEvent.click(screen.getByRole('button', { name: 'Unpin this view' }));
    expect(JSON.parse(storage.get(key)!).pins).toEqual([]);
  });

  it('brings a focused navigation link into view, because the rows scroll sideways on a phone', async () => {
    // Found by tabbing through Data's areas at 375 px in a real browser: focus
    // landed on Conversations at x=373..505 with the row still at scrollLeft 0,
    // so the focus ring was entirely off the right edge. jsdom cannot scroll,
    // so what is pinned here is that the handler is still wired — dropping it
    // is the way this comes back.
    const into = vi.fn();
    vi.spyOn(Element.prototype, 'scrollIntoView').mockImplementation(into);
    await setup('/datasets');
    const rows = screen.getAllByRole('navigation');
    const link = within(rows[rows.length - 1] as HTMLElement).getAllByRole('link').at(-1) as HTMLElement;
    act(() => link.focus());
    expect(into).toHaveBeenCalledWith({ block: 'nearest', inline: 'nearest' });
  });

  it('counts completed area transitions once and omits blocked navigation and raw object context', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(false);
    const { router } = await setup('/datasets?name=private-dataset');
    await act(async () => { await router.navigate({ to: '/data-curation/pipeline', search: { name: 'private-pipeline' } }); });
    const input = await screen.findByRole('textbox', { name: 'Draft' });
    fireEvent.change(input, { target: { value: 'draft' } });
    await act(async () => { void router.navigate({ to: '/account' }); });
    expect(router.state.location.pathname).toBe('/data-curation/pipeline');
    fireEvent.change(input, { target: { value: '' } });
    await act(async () => { await router.navigate({ to: '/', search: { start: 'workspace' } }); });
    await userEvent.click(screen.getByText('Navigation diagnostics · this session'));
    const diagnostics = screen.getByText('Navigation diagnostics · this session').parentElement!;
    await waitFor(() => expect(diagnostics.textContent).toContain('Datasets → Data Curation · All work areas: 1'));
    expect(diagnostics.textContent).toContain('Data Curation → Your work · All work areas: 1');
    expect(diagnostics.textContent).not.toMatch(/private|Account/);
  });
});
