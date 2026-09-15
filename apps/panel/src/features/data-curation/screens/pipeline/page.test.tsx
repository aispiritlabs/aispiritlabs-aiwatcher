import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createBrowserHistory, createMemoryHistory, createRootRoute, createRoute, createRouter, RouterProvider } from '@tanstack/react-router';
import { afterEach, expect, it, vi } from 'vitest';
import { PipelinePage } from './page';
import { searchSchema } from './search';
import { serve, withQueries } from '@/test/server';

// Canvas rendering is independent of loading/saving drafts; keep real HTTP and router behavior.
vi.mock('@/features/data-curation/components/pipeline-canvas', () => ({ PipelineCanvas: () => <p>Canvas</p>, blockLabel: (kind: string) => kind }));
afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });
const pipeline = { name: 'saved-pipeline', description: 'saved description', blocks: [
  { id: 'source', title: 'Source', position: { x: 0, y: 0 }, spec: { kind: 'source', dataset: 'runs', arguments: {} } },
], edges: [], revision: 'saved-revision', saved_at: '2026-09-14T10:00:00Z' };

const second = { ...pipeline, name: 'second-pipeline', description: 'second description', revision: 'second-revision' };
const notebookBlock = { id: 'python', title: 'Python', position: { x: 200, y: 0 },
  spec: { kind: 'notebook', notebook: 'test.py', revision: 'code-one', params: {} } };
const notebookPipeline = { ...pipeline, name: 'notebook-pipeline', revision: 'notebook-revision',
  blocks: [...pipeline.blocks, notebookBlock], edges: [{ from: 'source', to: 'python' }] };
const notebook = { name: 'test.py', title: 'Test', revision: 'code-one', size: 10,
  modified_at: '2026-09-14T10:00:00Z', source: 'original code', app_url: '/notebook-app' };

async function setup(entry = '/data-curation/pipeline?name=saved-pipeline', browser = false) {
  serve([
    { method: 'GET', path: '/saved-pipeline/revisions/saved-revision', answer: { status: 200, body: pipeline } },
    { method: 'GET', path: '/second-pipeline/revisions/second-revision', answer: { status: 200, body: second } },
    { method: 'GET', path: '/notebook-pipeline/revisions/notebook-revision', answer: { status: 200, body: notebookPipeline } },
    { method: 'GET', path: '/saved-pipeline/revisions/older-revision', answer: { status: 404, body: { message: 'Missing revision' } } },
    { method: 'GET', path: '/saved-pipeline/revisions/historical-revision', answer: { status: 200, body: { ...pipeline, revision: 'historical-revision', description: 'Historic layout and metadata' } } },
    { method: 'GET', path: '/notebooks', answer: { status: 200, body: { notebooks: [notebook] } } },
    { method: 'GET', path: '/revisions/code-one', answer: { status: 200, body: notebook } },
    { method: 'GET', path: '/revisions/code-two', answer: { status: 200, body: { ...notebook, revision: 'code-two', source: 'submitted code' } } },
    { method: 'GET', path: '/healthz', answer: { status: 200, body: { engine: 'flow' } } },
    { method: 'GET', path: '/curation-pipelines', answer: { status: 200, body: { pipelines: [pipeline, second, notebookPipeline] } } },
    { method: 'GET', path: '/auth/config', answer: { status: 200, body: { enabled: false, mode: 'none' } } },
    { method: 'GET', path: '/saved-pipeline/schedule', answer: { status: 200, body: {
      schedule: { definition_name: 'saved-pipeline', schedule: {
        cadence: { every: 'daily', hour: 7, minute: 30 }, timezone: 'Europe/Warsaw', enabled: true, overlap: 'skip',
      } },
    } } },
  ]);
  vi.spyOn(window, 'scrollTo').mockImplementation(() => {});
  const root = createRootRoute();
  const route = createRoute({ getParentRoute: () => root, path: '/data-curation/pipeline', validateSearch: searchSchema, component: PipelinePage });
  const other = createRoute({ getParentRoute: () => root, path: '/account', component: () => <p>Other page</p> });
  if (browser) window.history.replaceState({}, '', entry);
  const router = createRouter({ routeTree: root.addChildren([route, other]),
    history: browser ? createBrowserHistory() : createMemoryHistory({ initialEntries: [entry] }) });
  render(withQueries(<RouterProvider router={router} />));
  await screen.findByRole('button', { name: /saved-pipeline/ });
  return router;
}

it('guards metadata-only edits and replacement by a saved pipeline', async () => {
  const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
  const router = await setup();
  fireEvent.change(screen.getByRole('textbox', { name: 'Pipeline description' }), { target: { value: 'my draft' } });
  await userEvent.click(screen.getByRole('button', { name: /saved-pipeline/ }));
  expect((screen.getByRole('textbox', { name: 'Pipeline description' }) as HTMLInputElement).value).toBe('my draft');
  await act(async () => { void router.navigate({ to: '/account' }); });
  expect(confirm).toHaveBeenCalledTimes(2);
  expect(router.state.location.pathname).toBe('/data-curation/pipeline');
  confirm.mockReturnValue(true);
  await userEvent.click(screen.getByRole('button', { name: /saved-pipeline/ }));
  confirm.mockClear();
  await act(async () => { void router.navigate({ to: '/account' }); });
  expect(confirm).not.toHaveBeenCalled();
});

it('keeps a schedule tied to the saved name and guards replacing its local edits', async () => {
  const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
  const router = await setup();
  await waitFor(() => expect((screen.getByRole('spinbutton', { name: 'Schedule hour' }) as HTMLInputElement).value).toBe('7'));
  fireEvent.change(screen.getByRole('spinbutton', { name: 'Schedule hour' }), { target: { value: '8' } });
  await userEvent.click(screen.getByRole('button', { name: /saved-pipeline/ }));
  expect(confirm).toHaveBeenCalledOnce();
  expect((screen.getByRole('spinbutton', { name: 'Schedule hour' }) as HTMLInputElement).value).toBe('8');
  fireEvent.change(screen.getByRole('textbox', { name: 'Pipeline name' }), { target: { value: 'renamed-pipeline' } });
  expect(screen.getByTitle('saved-pipeline')).toBeTruthy();
  await userEvent.click(screen.getByRole('button', { name: 'Save pipeline' }));
  await screen.findByText('Save or discard the schedule changes before saving the pipeline under a different name.');
  confirm.mockReturnValue(true);
  await userEvent.click(screen.getByRole('button', { name: /saved-pipeline/ }));
  await waitFor(() => expect((screen.getByRole('spinbutton', { name: 'Schedule hour' }) as HTMLInputElement).value).toBe('7'));
  confirm.mockClear();
  await act(async () => { void router.navigate({ to: '/account' }); });
  await screen.findByText('Other page');
  expect(confirm).not.toHaveBeenCalled();
});

it('keeps metadata edited during a save and clears the guard after saving that edit', async () => {
  const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
  const router = await setup();
  const originalFetch = fetch;
  let finish!: (response: Response) => void;
  vi.stubGlobal('fetch', (input: Request | string, init?: RequestInit) =>
    input instanceof Request && input.method === 'POST' && input.url.endsWith('/curation-pipelines')
      ? new Promise<Response>((resolve) => { finish = resolve; }) : originalFetch(input, init));
  fireEvent.change(screen.getByRole('textbox', { name: 'Pipeline description' }), { target: { value: 'submitted' } });
  await userEvent.click(screen.getByRole('button', { name: 'Save pipeline' }));
  await waitFor(() => expect(finish).toBeTypeOf('function'));
  fireEvent.change(screen.getByRole('textbox', { name: 'Pipeline description' }), { target: { value: 'newer description' } });
  await act(async () => finish(new Response(JSON.stringify({ pipeline: { ...pipeline, description: 'submitted' }, created: true }),
    { status: 200, headers: { 'Content-Type': 'application/json' } })));
  await screen.findByText('Unsaved pipeline changes.');
  expect((screen.getByRole('textbox', { name: 'Pipeline description' }) as HTMLInputElement).value).toBe('newer description');
  await act(async () => { void router.navigate({ to: '/account' }); });
  expect(confirm).toHaveBeenCalledOnce();
  finish = undefined!;
  await userEvent.click(screen.getByRole('button', { name: 'Save pipeline' }));
  await waitFor(() => expect(finish).toBeTypeOf('function'));
  await act(async () => finish(new Response(JSON.stringify({ pipeline: { ...pipeline, description: 'newer description' }, created: true }),
    { status: 200, headers: { 'Content-Type': 'application/json' } })));
  await waitFor(() => expect(screen.queryByText('Unsaved pipeline changes.')).toBeNull());
  confirm.mockClear();
  await act(async () => { void router.navigate({ to: '/account' }); });
  expect(confirm).not.toHaveBeenCalled();
});


it('restores pipeline name, revision and metadata with browser Back/Forward', async () => {
  const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
  const router = await setup(undefined, true);
  try {
    await waitFor(() => expect(router.state.location.search.revision).toBe('saved-revision'));
    await userEvent.click(screen.getByRole('button', { name: /second-pipeline/ }));
    await waitFor(() => expect(router.state.location.search.revision).toBe('second-revision'));
    fireEvent.change(screen.getByRole('textbox', { name: 'Pipeline description' }), { target: { value: 'local edit' } });
    await act(async () => router.history.back());
    await waitFor(() => expect(confirm).toHaveBeenCalledOnce());
    await waitFor(() => expect(window.location.search).toContain('second-pipeline'));
    expect(screen.getByRole('textbox', { name: 'Pipeline description' })).toHaveProperty('value', 'local edit');
    confirm.mockReturnValue(true);
    await act(async () => router.history.back());
    await waitFor(() => expect(screen.getByRole('textbox', { name: 'Pipeline name' })).toHaveProperty('value', 'saved-pipeline'));
    expect(screen.getByRole('textbox', { name: 'Pipeline description' })).toHaveProperty('value', 'saved description');
    await act(async () => router.history.forward());
    await waitFor(() => expect(screen.getByRole('textbox', { name: 'Pipeline name' })).toHaveProperty('value', 'second-pipeline'));
    expect(screen.getByRole('textbox', { name: 'Pipeline description' })).toHaveProperty('value', 'second description');
    expect(confirm).toHaveBeenCalledTimes(2);
  } finally { router.history.destroy(); }
});

it('keeps metadata through time/view changes and clears the canvas for a missing context', async () => {
  const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
  const router = await setup();
  await waitFor(() => expect(router.state.location.search.revision).toBe('saved-revision'));
  fireEvent.change(screen.getByRole('textbox', { name: 'Pipeline description' }), { target: { value: 'local edit' } });
  await act(async () => { void router.navigate({ to: '/data-curation/pipeline', search: { ...router.state.location.search, window: 3600 } }); });
  expect(confirm).not.toHaveBeenCalled();
  expect(screen.getByRole('textbox', { name: 'Pipeline description' })).toHaveProperty('value', 'local edit');
  await act(async () => { void router.navigate({ to: '/data-curation/pipeline', search: { name: 'missing' } }); });
  expect(confirm).toHaveBeenCalledOnce();
  confirm.mockReturnValue(true);
  await act(async () => { void router.navigate({ to: '/data-curation/pipeline', search: { name: 'missing' } }); });
  await screen.findByText('The linked pipeline revision is unavailable. Open an available saved pipeline.');
  expect(screen.getByRole('textbox', { name: 'Pipeline name' })).toHaveProperty('value', 'curation/untitled');
  expect(screen.getByRole('button', { name: 'Save pipeline' })).toHaveProperty('disabled', true);
});

it('does not silently load head for a missing pinned revision', async () => {
  await setup('/data-curation/pipeline?name=saved-pipeline&revision=older-revision');
  await screen.findByText('The linked pipeline revision is unavailable. Open an available saved pipeline.');
  expect(screen.getByRole('textbox', { name: 'Pipeline name' })).toHaveProperty('value', 'curation/untitled');
});

it('does not attach an old successful save to another pipeline', async () => {
  vi.spyOn(window, 'confirm').mockReturnValue(true);
  const router = await setup();
  const originalFetch = fetch;
  let finish!: (response: Response) => void;
  vi.stubGlobal('fetch', (input: Request | string, init?: RequestInit) =>
    input instanceof Request && input.method === 'POST' && input.url.endsWith('/curation-pipelines')
      ? new Promise<Response>((resolve) => { finish = resolve; }) : originalFetch(input, init));
  fireEvent.change(screen.getByRole('textbox', { name: 'Pipeline description' }), { target: { value: 'submitted' } });
  await userEvent.click(screen.getByRole('button', { name: 'Save pipeline' }));
  await waitFor(() => expect(finish).toBeTypeOf('function'));
  await act(async () => { void router.navigate({ to: '/data-curation/pipeline', search: { name: 'second-pipeline' } }); });
  await waitFor(() => expect(screen.getByRole('textbox', { name: 'Pipeline name' })).toHaveProperty('value', 'second-pipeline'));
  await act(async () => finish(new Response(JSON.stringify({ pipeline: { ...pipeline, description: 'submitted' }, created: true }),
    { status: 200, headers: { 'Content-Type': 'application/json' } })));
  expect(router.state.location.search.name).toBe('second-pipeline');
  expect(screen.getByRole('textbox', { name: 'Pipeline description' })).toHaveProperty('value', 'second description');
  expect(screen.queryByText('Unsaved pipeline changes.')).toBeNull();
});

it('protects nested code on selection changes, replacement by the same block id, and deletion', async () => {
  const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
  const router = await setup('/data-curation/pipeline?name=notebook-pipeline&block=python');
  const code = await screen.findByRole('textbox', { name: 'Code' });
  await waitFor(() => expect(code).toHaveProperty('value', 'original code'));
  fireEvent.change(code, { target: { value: 'local code' } });
  await userEvent.click(screen.getByRole('button', { name: 'Remove this block' }));
  expect(confirm).toHaveBeenCalledOnce();
  expect(code).toHaveProperty('value', 'local code');
  await act(async () => { void router.navigate({ to: '/data-curation/pipeline', search: { ...router.state.location.search, block: 'source' } }); });
  expect(confirm).toHaveBeenCalledTimes(2);
  expect(screen.getByRole('textbox', { name: 'Code' })).toHaveProperty('value', 'local code');
  await userEvent.click(screen.getByRole('button', { name: /notebook-pipeline/ }));
  expect(confirm).toHaveBeenCalledTimes(3);
  confirm.mockReturnValue(true);
  await userEvent.click(screen.getByRole('button', { name: /notebook-pipeline/ }));
  await act(async () => { void router.navigate({ to: '/data-curation/pipeline', search: { ...router.state.location.search, block: 'python' } }); });
  await waitFor(() => expect(screen.getByRole('textbox', { name: 'Code' })).toHaveProperty('value', 'original code'));
  expect(screen.queryByText('Unsaved notebook changes.')).toBeNull();
  fireEvent.change(screen.getByRole('textbox', { name: 'Code' }), { target: { value: 'delete me' } });
  confirm.mockClear();
  await userEvent.click(screen.getByRole('button', { name: 'Remove this block' }));
  expect(confirm).toHaveBeenCalledOnce();
  expect(screen.queryByRole('textbox', { name: 'Code' })).toBeNull();
});

it('guards invalid notebook settings and allows discarding them before changing views', async () => {
  const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
  await setup('/data-curation/pipeline?name=notebook-pipeline&block=python');
  const settings = await screen.findByRole('textbox', { name: /^Settings/ });
  fireEvent.change(settings, { target: { value: '{broken' } });
  await userEvent.click(screen.getByRole('button', { name: 'Notebook view' }));
  expect(confirm).toHaveBeenCalledOnce();
  expect(settings).toHaveProperty('value', '{broken');
  await userEvent.click(screen.getByRole('button', { name: 'Discard invalid settings' }));
  expect(settings).toHaveProperty('value', '{}');
  confirm.mockClear();
  await userEvent.click(screen.getByRole('button', { name: 'Notebook view' }));
  await screen.findByRole('region', { name: 'Cell 2: Python' });
  expect(confirm).not.toHaveBeenCalled();
});

it.each([false, true])('preserves new notebook code/settings or departure during a save (leave: %s)', async (leave) => {
  vi.spyOn(window, 'confirm').mockReturnValue(true);
  const router = await setup('/data-curation/pipeline?name=notebook-pipeline&block=python');
  const code = await screen.findByRole('textbox', { name: 'Code' });
  await waitFor(() => expect(code).toHaveProperty('value', 'original code'));
  const originalFetch = fetch;
  let finish!: (response: Response) => void;
  vi.stubGlobal('fetch', (input: Request | string, init?: RequestInit) =>
    typeof input === 'string' && init?.method === 'PUT' && input.endsWith('/notebooks/test.py')
      ? new Promise<Response>((resolve) => { finish = resolve; }) : originalFetch(input, init));
  fireEvent.change(code, { target: { value: 'submitted code' } });
  await userEvent.click(screen.getByRole('button', { name: 'Save notebook' }));
  await waitFor(() => expect(finish).toBeTypeOf('function'));
  if (leave) {
    await act(async () => { void router.navigate({ to: '/data-curation/pipeline', search: { name: 'second-pipeline' } }); });
    await waitFor(() => expect(screen.getByRole('textbox', { name: 'Pipeline name' })).toHaveProperty('value', 'second-pipeline'));
  } else {
    fireEvent.change(code, { target: { value: 'newer code' } });
    fireEvent.change(screen.getByRole('textbox', { name: /^Settings/ }), { target: { value: '{"new":1}' } });
    fireEvent.change(screen.getByRole('textbox', { name: 'Block title' }), { target: { value: 'New title' } });
  }
  await act(async () => finish(new Response(JSON.stringify({ ...notebook, revision: 'code-two', source: 'submitted code' }),
    { status: 200, headers: { 'Content-Type': 'application/json' } })));
  if (leave) {
    expect(router.state.location.search.name).toBe('second-pipeline');
    expect(screen.queryByText('Unsaved pipeline changes.')).toBeNull();
  } else {
    expect(screen.getByRole('textbox', { name: 'Code' })).toHaveProperty('value', 'newer code');
    expect(screen.getByRole('textbox', { name: /^Settings/ })).toHaveProperty('value', '{"new":1}');
    expect(screen.getByRole('textbox', { name: 'Block title' })).toHaveProperty('value', 'New title');
    expect(screen.getByText('Unsaved notebook changes.')).toBeTruthy();
    await userEvent.click(screen.getByRole('button', { name: 'Discard code edits' }));
    expect(screen.getByRole('textbox', { name: 'Code' })).toHaveProperty('value', 'submitted code');
  }
});


it('loads a historical revision from a deep link even after the registry head changes', async () => {
  const router = await setup('/data-curation/pipeline?name=saved-pipeline&revision=historical-revision');
  await waitFor(() => expect(screen.getByRole('textbox', { name: 'Pipeline description' })).toHaveProperty('value', 'Historic layout and metadata'));
  expect(router.state.location.search.revision).toBe('historical-revision');
  expect(screen.queryByText('Unsaved pipeline changes.')).toBeNull();
});

it('restores an imported flow through history without loading a saved flow with the same name', async () => {
  vi.spyOn(window, 'confirm').mockReturnValue(true);
  const router = await setup(undefined, true);
  try {
    const bundle = { format: 'aiwatcher.curation', version: 1,
      requirements: { python: '>=3.14', packages: [] },
      pipeline: { name: pipeline.name, description: 'Imported flow', blocks: pipeline.blocks, edges: [] }, notebooks: [] };
    const file = new File([JSON.stringify(bundle)], 'import.flow.json', { type: 'application/json' });
    Object.defineProperty(file, 'text', { value: async () => JSON.stringify(bundle) });
    await userEvent.upload(screen.getByLabelText('Import flow file'), file);
    await waitFor(() => expect(screen.getByRole('textbox', { name: 'Pipeline description' })).toHaveProperty('value', 'Imported flow'));
    expect(router.state.location.search.revision).toBeUndefined();
    const draftId = router.state.location.search.draft;
    expect(draftId).toBeTypeOf('string');
    await userEvent.click(screen.getByRole('button', { name: /second-pipeline/ }));
    await waitFor(() => expect(router.state.location.search.name).toBe('second-pipeline'));
    await act(async () => router.history.back());
    await waitFor(() => expect(screen.getByRole('textbox', { name: 'Pipeline description' })).toHaveProperty('value', 'Imported flow'));
    expect(router.state.location.search.draft).toBe(draftId);
    expect(screen.getByText('Unsaved pipeline changes.')).toBeTruthy();
  } finally { router.history.destroy(); }
});

it('reports an unavailable local flow instead of borrowing a saved flow after reload', async () => {
  await setup('/data-curation/pipeline?name=saved-pipeline&draft=expired-local-draft');
  await screen.findByText('This local flow is no longer available. Import it again or open a saved pipeline.');
  expect(screen.getByRole('textbox', { name: 'Pipeline name' })).toHaveProperty('value', 'curation/untitled');
});

it('retains notebook code after a failed save and keeps navigation protected', async () => {
  const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
  const router = await setup('/data-curation/pipeline?name=notebook-pipeline&block=python');
  const code = await screen.findByRole('textbox', { name: 'Code' });
  await waitFor(() => expect(code).toHaveProperty('value', 'original code'));
  const originalFetch = fetch;
  vi.stubGlobal('fetch', (input: Request | string, init?: RequestInit) =>
    typeof input === 'string' && init?.method === 'PUT' && input.endsWith('/notebooks/test.py')
      ? Promise.resolve(new Response(JSON.stringify({ error: { message: 'Notebook save failed' } }),
        { status: 422, headers: { 'Content-Type': 'application/json' } })) : originalFetch(input, init));
  fireEvent.change(code, { target: { value: 'recoverable code' } });
  await userEvent.click(screen.getByRole('button', { name: 'Save notebook' }));
  await screen.findByText('Notebook save failed');
  expect(code).toHaveProperty('value', 'recoverable code');
  await act(async () => { void router.navigate({ to: '/account' }); });
  expect(confirm).toHaveBeenCalledOnce();
  expect(router.state.location.pathname).toBe('/data-curation/pipeline');
});
