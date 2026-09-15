import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import {
  createMemoryHistory,
  createBrowserHistory,
  createRootRoute,
  createRoute,
  createRouter,
  RouterProvider,
} from '@tanstack/react-router';
import { afterEach, expect, it, vi } from 'vitest';
import { LabelPage } from './page';
import { searchSchema } from './search';
import { serve, withQueries } from '@/test/server';
import type { CanvasProps } from '@/features/annotations/components/annotation-canvas';
import type { Annotation } from '@/api/generated/types.gen';

vi.mock('@/features/annotations/components/annotation-canvas', () => ({
  AnnotationCanvas: (props: CanvasProps) => (
    <div>
      Image canvas
      <output data-testid="drawing">{JSON.stringify(props.annotations)}</output>
      <button onClick={() => props.onCreate({ kind: 'point', at: [10, 20] })}>Create point</button>
      <button onClick={() => props.onDraftChange?.(true)}>Start unfinished shape</button>
      <button onClick={() => props.onDraftChange?.(false)}>Cancel unfinished shape</button>
    </div>
  ),
}));
// jsdom has no layout for virtualized rows; the browser check exercises the real list.
vi.mock('@/shared/components/virtual-list', () => ({
  VirtualList: ({
    items,
    renderRow,
  }: {
    items: unknown[];
    renderRow: (item: unknown, index: number) => React.ReactNode;
  }) => (
    <div>
      {items.map((item, index) => (
        <div key={index}>{renderRow(item, index)}</div>
      ))}
    </div>
  ),
}));
const browserHistories: ReturnType<typeof createBrowserHistory>[] = [];
afterEach(() => {
  browserHistories.splice(0).forEach((history) => history.destroy());
  vi.unstubAllGlobals();
});

it('pins a saved draft revision when an older revision is accepted', async () => {
  vi.stubGlobal('scrollTo', () => {});
  const project = { name: 'demo', schema: { version: '1', classes: [] } };
  const head = {
    project: 'demo',
    accepted: 'accepted-revision',
    review: 'accepted',
    revisions: [],
    image: {
      image_id: 'image-1',
      group_id: 'sample',
      width: 100,
      height: 100,
      uri: '/sample.png',
      rights: { kind: 'owned' },
    },
  };
  const revision = (id: string) => ({
    project: 'demo',
    image_id: 'image-1',
    revision: id,
    annotations: [],
    schema_version: '1',
    created_at: '2026-09-14T10:00:00Z',
  });
  serve([
    {
      method: 'GET',
      path: '/annotation-projects',
      answer: { status: 200, body: { projects: [project] } },
    },
    { method: 'GET', path: '/annotation-project', answer: { status: 200, body: project } },
    {
      method: 'GET',
      path: '/annotation-images',
      answer: { status: 200, body: { images: [head], total: 1, offset: 0, limit: 200 } },
    },
    {
      method: 'GET',
      path: '/annotation-image',
      answer: (call) => ({
        status: 200,
        body: {
          ...head,
          split: 'train',
          revision: revision(call === 1 ? 'accepted-revision' : 'saved-draft'),
        },
      }),
    },
    {
      method: 'POST',
      path: '/annotation-revisions',
      answer: { status: 200, body: { head, revision: revision('saved-draft') } },
    },
  ]);
  const fetchMock = vi.fn(fetch);
  vi.stubGlobal('fetch', fetchMock);
  const root = createRootRoute();
  const route = createRoute({
    getParentRoute: () => root,
    path: '/annotations/label',
    validateSearch: searchSchema,
    component: LabelPage,
  });
  const router = createRouter({
    routeTree: root.addChildren([route]),
    history: createMemoryHistory({
      initialEntries: ['/annotations/label?project=demo&image=image-1'],
    }),
  });
  render(withQueries(<RouterProvider router={router} />));
  await screen.findByText('Image canvas');
  await userEvent.click(screen.getByRole('button', { name: 'Save draft' }));
  await waitFor(() => expect(router.state.location.search.revision).toBe('saved-draft'));
  expect(screen.getByText('saved-draft')).toBeTruthy();
  await act(() => router.load());
  expect(router.state.location.search.revision).toBe('saved-draft');
  const urls = fetchMock.mock.calls.map(([request]) => new URL((request as Request).url));
  await waitFor(() =>
    expect(urls.some((url) => url.searchParams.get('revision') === 'saved-draft')).toBe(true),
  );
});

function workspace(
  initial = '/annotations/label?project=demo&image=image-1&review=draft&split=train&q=family',
  browser = false,
) {
  vi.stubGlobal('scrollTo', () => {});
  const project = {
    name: 'demo',
    schema: {
      version: '1',
      classes: [
        { name: 'point', geometry: 'point', attributes: [{ name: 'score', kind: 'number' }] },
      ],
    },
  };
  const head = (id: number) => ({
    project: 'demo',
    review: 'draft',
    revisions: [],
    image: {
      image_id: `image-${id}`,
      group_id: `family-${id}`,
      width: 100,
      height: 100,
      uri: `/image-${id}.png`,
      rights: { kind: 'owned' },
    },
  });
  const saved = new Map<string, Annotation[]>();
  const state = {
    failSave: false,
    failPage: false,
    pageGate: undefined as Promise<void> | undefined,
    listEmpty: false,
  };
  const calls: URL[] = [];
  const writes: { image_id: string; annotations: Annotation[] }[] = [];
  vi.stubGlobal('fetch', async (request: Request) => {
    const url = new URL(request.url);
    calls.push(url);
    const path = url.pathname;
    let body: unknown;
    let status = 200;
    if (path.endsWith('/annotation-projects')) body = { projects: [project] };
    else if (path.endsWith('/annotation-project')) body = project;
    else if (path.endsWith('/annotation-images')) {
      const offset = Number(url.searchParams.get('offset'));
      if (offset) {
        await state.pageGate;
        if (state.failPage) status = 503;
      }
      body = state.listEmpty
        ? { images: [], total: 0, offset: 0, limit: 50 }
        : {
            images: offset ? [head(2), head(3)] : [head(1), head(2)],
            total: 3,
            offset,
            limit: 50,
            next_offset: offset ? null : 2,
          };
    } else if (path.endsWith('/annotation-image')) {
      const id = Number(url.searchParams.get('image_id')?.split('-')[1]);
      const revision = url.searchParams.get('revision') ?? `base-${id}`;
      body = {
        ...head(id),
        split: 'train',
        revision: { revision, annotations: saved.get(revision) ?? [], schema_version: '1' },
      };
    } else if (path.endsWith('/annotation-revisions')) {
      const sent = (await request.json()) as { image_id: string; annotations: Annotation[] };
      writes.push(sent);
      if (state.failSave) {
        status = 422;
        body = { message: 'Invalid drawing', details: ['point-1: rejected'] };
      } else {
        const revision = `saved-${writes.length}`;
        saved.set(revision, sent.annotations);
        body = {
          head: head(Number(sent.image_id.split('-')[1])),
          revision: { revision, annotations: sent.annotations, schema_version: '1' },
        };
      }
    } else {
      status = 404;
      body = {};
    }
    return new Response(JSON.stringify(body), {
      status,
      headers: { 'Content-Type': 'application/json' },
    });
  });
  const root = createRootRoute();
  const route = createRoute({
    getParentRoute: () => root,
    path: '/annotations/label',
    validateSearch: searchSchema,
    component: LabelPage,
  });
  const away = createRoute({
    getParentRoute: () => root,
    path: '/away',
    component: () => <p>Other work</p>,
  });
  if (browser) window.history.replaceState({}, '', initial);
  const history = browser
    ? createBrowserHistory()
    : createMemoryHistory({ initialEntries: [initial] });
  if (browser) browserHistories.push(history);
  const router = createRouter({
    routeTree: root.addChildren([route, away]),
    history,
  });
  render(withQueries(<RouterProvider router={router} />));
  return { router, calls, writes, state };
}

const drawing = () => JSON.parse(screen.getByTestId('drawing').textContent ?? '[]') as Annotation[];

it('loads the next page with the same filters, deduplicates images, and navigates both ways', async () => {
  const { router, calls } = workspace();
  await screen.findByText('Image canvas');
  expect(screen.getByRole('button', { name: 'Previous image' }).hasAttribute('disabled')).toBe(
    true,
  );
  await userEvent.click(screen.getByRole('button', { name: 'Next image' }));
  await waitFor(() => expect(router.state.location.search.image).toBe('image-2'));
  await userEvent.click(screen.getByRole('button', { name: 'Next image' }));
  await waitFor(() => expect(router.state.location.search.image).toBe('image-3'));
  expect(screen.getByText('3 loaded · 3 matching')).toBeTruthy();
  expect(screen.getAllByRole('button', { name: /Open image family-2/ })).toHaveLength(1);
  const page = calls.find((url) => url.searchParams.get('offset') === '2');
  expect(page?.searchParams.get('review')).toBe('draft');
  expect(page?.searchParams.get('split')).toBe('train');
  expect(page?.searchParams.get('search')).toBe('family');
  expect(screen.getByRole('button', { name: 'Next image' }).hasAttribute('disabled')).toBe(true);
  await userEvent.click(screen.getByRole('button', { name: 'Previous image' }));
  await waitFor(() => expect(router.state.location.search.image).toBe('image-2'));
});

it('retains local changes when navigation is cancelled and saves the correct image before leaving', async () => {
  const { router, writes } = workspace();
  await screen.findByText('Image canvas');
  await userEvent.click(screen.getByRole('button', { name: 'Create point' }));
  await userEvent.click(screen.getByRole('button', { name: 'Next image' }));
  await screen.findByText('This image has unsaved changes.');
  await userEvent.click(screen.getByRole('button', { name: 'Keep editing' }));
  expect(drawing()).toHaveLength(1);
  expect(router.state.location.search.image).toBe('image-1');
  await userEvent.click(screen.getByRole('button', { name: 'Next image' }));
  await userEvent.click(await screen.findByRole('button', { name: 'Save draft & leave' }));
  await waitFor(() => expect(router.state.location.search.image).toBe('image-2'));
  expect(writes[0]?.image_id).toBe('image-1');
  expect(writes[0]?.annotations).toHaveLength(1);
  await waitFor(() => expect(drawing()).toHaveLength(0));
  expect(
    screen.getByRole('button', { name: 'Undo annotation change' }).hasAttribute('disabled'),
  ).toBe(true);
});

it('undoes creation and attribute edits, preserves history after save and does not hijack field undo', async () => {
  const { router } = workspace();
  await screen.findByText('Image canvas');
  await userEvent.click(screen.getByRole('button', { name: 'Create point' }));
  fireEvent.change(screen.getByRole('spinbutton', { name: 'score' }), { target: { value: '7' } });
  fireEvent.keyDown(screen.getByRole('spinbutton', { name: 'score' }), { key: 'z', ctrlKey: true });
  expect(drawing()[0]?.attributes?.score).toBe(7);
  fireEvent.keyDown(window, { key: 'z', ctrlKey: true });
  expect(drawing()[0]?.attributes?.score).toBeUndefined();
  fireEvent.keyDown(window, { key: 'z', ctrlKey: true, shiftKey: true });
  expect(drawing()[0]?.attributes?.score).toBe(7);
  await userEvent.click(screen.getByRole('button', { name: 'Save draft' }));
  await waitFor(() => expect(router.state.location.search.revision).toBe('saved-1'));
  expect(screen.queryByText('unsaved')).toBeNull();
  await userEvent.click(screen.getByRole('button', { name: 'Undo annotation change' }));
  expect(screen.getByText('unsaved')).toBeTruthy();
  await userEvent.click(screen.getByRole('button', { name: 'Redo annotation change' }));
  expect(screen.queryByText('unsaved')).toBeNull();
  await userEvent.click(screen.getByRole('button', { name: 'Undo annotation change' }));
  await userEvent.click(screen.getByRole('button', { name: 'Undo annotation change' }));
  expect(drawing()).toHaveLength(0);
});

it('keeps an unfinished gesture protected and refuses to save a partial drawing', async () => {
  const { router, writes } = workspace();
  await screen.findByText('Image canvas');
  await userEvent.click(screen.getByRole('button', { name: 'Start unfinished shape' }));
  expect(screen.getByRole('button', { name: 'Save draft' }).hasAttribute('disabled')).toBe(true);
  fireEvent.keyDown(window, { key: 's', ctrlKey: true });
  expect(writes).toHaveLength(0);
  await userEvent.click(screen.getByRole('button', { name: 'Next image' }));
  expect(
    (await screen.findByRole('button', { name: 'Save draft & leave' })).hasAttribute('disabled'),
  ).toBe(true);
  await userEvent.click(screen.getByRole('button', { name: 'Discard & leave' }));
  await waitFor(() => expect(router.state.location.search.image).toBe('image-2'));
  expect(writes).toHaveLength(0);
});

it('keeps the draft and requested navigation after a failed save, allowing retry', async () => {
  const { router, state } = workspace();
  state.failSave = true;
  await screen.findByText('Image canvas');
  await userEvent.click(screen.getByRole('button', { name: 'Create point' }));
  await userEvent.click(screen.getByRole('button', { name: 'Next image' }));
  await userEvent.click(await screen.findByRole('button', { name: 'Save draft & leave' }));
  await screen.findByText('point-1: rejected');
  expect(router.state.location.search.image).toBe('image-1');
  expect(drawing()).toHaveLength(1);
  state.failSave = false;
  await userEvent.click(screen.getByRole('button', { name: 'Save draft & leave' }));
  await waitFor(() => expect(router.state.location.search.image).toBe('image-2'));
});

it('pins an implicit image and retains it when filters produce an empty list', async () => {
  const { router, state, calls } = workspace('/annotations/label?project=demo');
  await screen.findByText('Image canvas');
  await waitFor(() => expect(router.state.location.search.image).toBe('image-1'));
  await userEvent.click(screen.getByRole('button', { name: 'Create point' }));
  state.listEmpty = true;
  fireEvent.change(screen.getByRole('textbox', { name: 'Search images' }), {
    target: { value: 'missing' },
  });
  await screen.findByText('No images match these filters');
  expect(router.state.location.search.image).toBe('image-1');
  expect(drawing()).toHaveLength(1);
  expect(
    calls.some(
      (url) =>
        url.searchParams.get('search') === 'missing' && url.searchParams.get('offset') === '0',
    ),
  ).toBe(true);
});

it('ignores a late next-page navigation after the user chooses another image', async () => {
  const { router, state } = workspace('/annotations/label?project=demo&image=image-2');
  let release!: () => void;
  state.pageGate = new Promise<void>((resolve) => {
    release = resolve;
  });
  await screen.findByText('Image canvas');
  await userEvent.click(screen.getByRole('button', { name: 'Next image' }));
  await userEvent.click(screen.getByRole('button', { name: 'Previous image' }));
  await waitFor(() => expect(router.state.location.search.image).toBe('image-1'));
  await act(async () => {
    release();
  });
  await screen.findByText('3 loaded · 3 matching');
  expect(router.state.location.search.image).toBe('image-1');
});

it('retains loaded rows and selection after a failed page request and retries it', async () => {
  const { router, state } = workspace();
  state.failPage = true;
  await screen.findByText('Image canvas');
  await userEvent.click(screen.getByRole('button', { name: 'Load more images' }));
  await screen.findByText('Could not load images.');
  expect(screen.getByText('2 loaded · 3 matching')).toBeTruthy();
  expect(router.state.location.search.image).toBe('image-1');
  state.failPage = false;
  await userEvent.click(screen.getByRole('button', { name: 'Retry images' }));
  await screen.findByText('3 loaded · 3 matching');
});

it('undoes deletion and protects the drawing during Back and browser unload', async () => {
  const { router } = workspace(undefined, true);
  await screen.findByText('Image canvas');
  await userEvent.click(screen.getByRole('button', { name: 'Next image' }));
  await waitFor(() => expect(router.state.location.search.image).toBe('image-2'));
  await screen.findByText('Image canvas');
  await userEvent.click(screen.getByRole('button', { name: 'Create point' }));
  fireEvent.keyDown(window, { key: 'Delete' });
  expect(drawing()).toHaveLength(0);
  await userEvent.click(screen.getByRole('button', { name: 'Undo annotation change' }));
  expect(drawing()).toHaveLength(1);
  const unload = new Event('beforeunload', { cancelable: true });
  window.dispatchEvent(unload);
  expect(unload.defaultPrevented).toBe(true);
  act(() => router.history.back());
  await screen.findByText('This image has unsaved changes.');
  await userEvent.click(screen.getByRole('button', { name: 'Keep editing' }));
  expect(router.state.location.search.image).toBe('image-2');
  expect(drawing()).toHaveLength(1);
});
