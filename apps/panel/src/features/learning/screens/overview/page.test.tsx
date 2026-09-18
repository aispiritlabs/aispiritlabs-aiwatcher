import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import {
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
  RouterProvider,
} from '@tanstack/react-router';
import { afterEach, expect, it, vi } from 'vitest';

import { LearningPage } from './page';
import { searchSchema } from './search';
import { refusal, serve, withQueries, type Route } from '@/test/server';

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

const DAY = 24 * 60 * 60;
const MONDAY = 1_800_000_000;
const WEDNESDAY = MONDAY + 2 * DAY;

const project = { name: 'Retrieval workshop', scope: { organization: 'org', project: 'ret' } };
const other = { name: 'Vision workshop', scope: { organization: 'org', project: 'vis' } };

const grant = (id: string, parts: Record<string, unknown> = {}) => ({
  id,
  role: 'editor',
  scope: project.scope,
  grantee: { kind: 'user', value: { provider: 'authentik', subject: 'student-subject' } },
  window: { valid_from: MONDAY, edit_until: null, read_until: null },
  ...parts,
});

/** The three reads every rendering of this page makes, with sensible defaults. */
function server(routes: Route[]) {
  // A workshop with no labs is the default, so every test about grants and
  // invitations keeps answering what it used to. A test about labs says so by
  // serving `/labs` itself, and its route wins because this one is not added.
  const labs: Route[] = routes.some((route) => route.path === '/labs')
    ? []
    : [{ method: 'GET', path: '/labs', answer: { status: 200, body: { labs: [], total: 0 } } }];
  return serve([
    ...labs,
    { method: 'GET', path: '/auth/config', answer: { status: 200, body: { enabled: true } } },
    {
      method: 'GET',
      path: '/auth/me',
      answer: {
        status: 200,
        body: { credential: 'session', roles: ['viewer'], subject: 'teacher-subject' },
      },
    },
    {
      method: 'GET',
      path: '/iam/organizations',
      answer: { status: 200, body: [{ id: 'org', name: 'AI Spirit Labs' }] },
    },
    ...routes,
  ]);
}

async function open(entry: string) {
  vi.setSystemTime(new Date(WEDNESDAY * 1000));
  vi.stubGlobal('scrollTo', () => {});
  const root = createRootRoute();
  const route = createRoute({
    getParentRoute: () => root,
    path: '/learning',
    validateSearch: searchSchema,
    component: LearningPage,
  });
  const router = createRouter({
    routeTree: root.addChildren([route]),
    history: createMemoryHistory({ initialEntries: [entry] }),
  });
  render(withQueries(<RouterProvider router={router} />));
  return router;
}

it('separates the workshops a grant reaches from the ones only administered', async () => {
  // Two reads that answer different questions: `projects` is the caller's own
  // grants, a roster's `projects` is every project in the organization. Mixing
  // them would put a role badge on a workshop nobody can open.
  vi.useFakeTimers({ shouldAdvanceTime: true });
  server([
    {
      method: 'GET',
      path: '/projects',
      answer: {
        status: 200,
        body: [
          {
            project,
            role: 'editor',
            evaluated_at: WEDNESDAY,
            grants: [{ grant: grant('g1'), role: 'editor' }],
          },
        ],
      },
    },
    {
      method: 'GET',
      path: '/roster',
      answer: {
        status: 200,
        body: {
          organization: { id: 'org', name: 'AI Spirit Labs' },
          members: [],
          teams: [],
          projects: [project, other],
        },
      },
    },
  ]);
  await open('/learning?organization=org');

  const mine = (await screen.findByText('Open to you now')).closest('section') as HTMLElement;
  expect(within(mine).getByText('Retrieval workshop')).toBeTruthy();
  expect(within(mine).queryByText('Vision workshop')).toBeNull();

  // A live grant is what puts a workshop on the first list, so the second one
  // is not "administered": a place that opens next week sits there too, and
  // the page says all three reasons rather than picking one.
  const theirs = screen.getByText('Also in this organization').closest('section') as HTMLElement;
  expect(within(theirs).getByText('Vision workshop')).toBeTruthy();
  expect(within(theirs).getByText('nothing of yours in force')).toBeTruthy();
});

it('tells a participant what the list cannot show them', async () => {
  // The gap a browser found: `Policy::projects` keeps only the projects a
  // grant reaches *now*, so a place opening next Monday is simply absent — and
  // a participant cannot read the roster to discover it. Saying so is the
  // whole fix available here; there is no route for "what am I enrolled on".
  vi.useFakeTimers({ shouldAdvanceTime: true });
  server([
    {
      method: 'GET',
      path: '/projects',
      answer: {
        status: 200,
        body: [
          {
            project,
            role: 'editor',
            evaluated_at: WEDNESDAY,
            grants: [{ grant: grant('g1'), role: 'editor' }],
          },
        ],
      },
    },
    {
      method: 'GET',
      path: '/roster',
      answer: { status: 403, body: refusal('forbidden', 'not an administrator') },
    },
  ]);
  await open('/learning?organization=org');

  await screen.findByText('Retrieval workshop');
  expect(screen.getByText(/A place that opens later is not here yet/)).toBeTruthy();
  expect(screen.queryByText('Also in this organization')).toBeNull();
  expect(screen.queryByRole('alert')).toBeNull();
});

it('keeps one person’s two grants on one row and shows the closed one beside the live one', async () => {
  // The case that decides whether this page can be trusted: a workshop grant
  // ending must never read as that person having lost access, because the
  // permanent grant beside it is still in force.
  vi.useFakeTimers({ shouldAdvanceTime: true });
  server([
    {
      method: 'GET',
      path: '/projects',
      answer: {
        status: 200,
        body: [
          {
            project,
            role: 'admin',
            evaluated_at: WEDNESDAY,
            grants: [{ grant: grant('mine', { role: 'admin' }), role: 'admin' }],
          },
        ],
      },
    },
    {
      method: 'GET',
      path: '/roster',
      answer: { status: 403, body: refusal('forbidden', 'not an administrator') },
    },
    {
      method: 'GET',
      path: '/access',
      answer: {
        status: 200,
        body: {
          project,
          role: 'admin',
          evaluated_at: WEDNESDAY,
          grants: [{ grant: grant('mine', { role: 'admin' }), role: 'admin' }],
        },
      },
    },
    {
      method: 'GET',
      path: '/grants',
      answer: {
        status: 200,
        body: [
          grant('lapsed', {
            role: 'editor',
            window: { valid_from: MONDAY, edit_until: null, read_until: MONDAY + DAY },
          }),
          grant('standing', { role: 'viewer' }),
        ],
      },
    },
    { method: 'GET', path: '/invitations', answer: { status: 200, body: [] } },
  ]);
  await open('/learning?organization=org&project=ret');

  const person = (await screen.findByText(/^at authentik$/)).closest('li') as HTMLElement;
  expect(within(person).getByText('2 grants, each in force on its own')).toBeTruthy();
  expect(within(person).getByText('closed')).toBeTruthy();
  expect(within(person).getByText('open')).toBeTruthy();
});

it('reads an editor whose editing has ended as read-only, and a viewer as unchanged', async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  const ended = { valid_from: MONDAY, edit_until: MONDAY + DAY, read_until: null };
  server([
    { method: 'GET', path: '/projects', answer: { status: 200, body: [] } },
    {
      method: 'GET',
      path: '/roster',
      answer: {
        status: 200,
        body: {
          organization: { id: 'org', name: 'AI Spirit Labs' },
          members: [],
          teams: [],
          projects: [project],
        },
      },
    },
    {
      method: 'GET',
      path: '/access',
      answer: { status: 404, body: refusal('not_found', 'no grant') },
    },
    {
      method: 'GET',
      path: '/grants',
      answer: {
        status: 200,
        body: [
          grant('editing-over', { role: 'editor', window: ended }),
          grant('reader', {
            role: 'viewer',
            window: ended,
            grantee: { kind: 'user', value: { provider: 'authentik', subject: 'reader-subject' } },
          }),
        ],
      },
    },
    { method: 'GET', path: '/invitations', answer: { status: 200, body: [] } },
  ]);
  await open('/learning?organization=org&project=ret');

  await screen.findByText('read-only');
  expect(screen.getByText('open')).toBeTruthy();
  expect(screen.getAllByText(/editing ended/)).toHaveLength(1);
});

it('renders an absent grant of your own as the answer it is, not as a failure', async () => {
  // `Policy::access` reports nothing when the caller holds no live grant, which
  // is the ordinary state of an administrator looking at a workshop they run
  // and are not on. Drawing that 404 in red told them a read had broken.
  vi.useFakeTimers({ shouldAdvanceTime: true });
  server([
    { method: 'GET', path: '/projects', answer: { status: 200, body: [] } },
    {
      method: 'GET',
      path: '/roster',
      answer: {
        status: 200,
        body: {
          organization: { id: 'org', name: 'AI Spirit Labs' },
          members: [],
          teams: [],
          projects: [project],
        },
      },
    },
    {
      method: 'GET',
      path: '/access',
      answer: { status: 404, body: refusal('not_found', 'no grant') },
    },
    { method: 'GET', path: '/grants', answer: { status: 200, body: [] } },
    { method: 'GET', path: '/invitations', answer: { status: 200, body: [] } },
  ]);
  await open('/learning?organization=org&project=ret');

  await screen.findByText(/You hold no grant on this workshop/);
  expect(screen.queryByRole('alert')).toBeNull();
  // Administering it is still enough to enrol somebody, which is the whole
  // reason the roster answers about projects no grant of theirs reaches.
  expect(screen.getByRole('button', { name: 'Create invitation' })).toBeTruthy();
});

it('offers no enrolment control to somebody who may not enrol', async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  server([
    { method: 'GET', path: '/projects', answer: { status: 200, body: [] } },
    {
      method: 'GET',
      path: '/roster',
      answer: { status: 403, body: refusal('forbidden', 'not an administrator') },
    },
    {
      method: 'GET',
      path: '/access',
      answer: {
        status: 200,
        body: {
          project,
          role: 'editor',
          evaluated_at: WEDNESDAY,
          grants: [{ grant: grant('g1'), role: 'editor' }],
        },
      },
    },
    {
      method: 'GET',
      path: '/grants',
      answer: { status: 403, body: refusal('forbidden', 'not an administrator') },
    },
    { method: 'GET', path: '/invitations', answer: { status: 200, body: [] } },
  ]);
  await open('/learning?organization=org&project=ret');

  await screen.findByText(/Enrolling somebody needs admin on this workshop/);
  expect(screen.queryByRole('button', { name: 'Create invitation' })).toBeNull();
  // A 403 on the participant list is the server answering about the reader.
  expect(screen.getByText(/The list of participants is for whoever may enrol one/)).toBeTruthy();
  expect(screen.queryByRole('alert')).toBeNull();
});

it('shows an enrolment token once and never asks the server for it again', async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  const issued = {
    token: 'the-secret-token',
    invitation: {
      id: 'inv-1',
      scope: project.scope,
      role: 'viewer',
      created_at: MONDAY,
      expires_at: MONDAY + DAY,
      created_by: { provider: 'authentik', subject: 'teacher-subject' },
      window: { valid_from: MONDAY, edit_until: null, read_until: null },
      redeemed: null,
    },
  };
  const calls = server([
    { method: 'GET', path: '/projects', answer: { status: 200, body: [] } },
    {
      method: 'GET',
      path: '/roster',
      answer: {
        status: 200,
        body: {
          organization: { id: 'org', name: 'AI Spirit Labs' },
          members: [],
          teams: [],
          projects: [project],
        },
      },
    },
    {
      method: 'GET',
      path: '/access',
      answer: { status: 404, body: refusal('not_found', 'no grant') },
    },
    { method: 'GET', path: '/grants', answer: { status: 200, body: [] } },
    {
      method: 'GET',
      path: '/invitations',
      answer: (call) => ({ status: 200, body: call === 1 ? [] : [issued.invitation] }),
    },
    { method: 'POST', path: '/invitations', answer: { status: 201, body: issued } },
  ]);
  await open('/learning?organization=org&project=ret');

  await userEvent.click(await screen.findByRole('button', { name: 'Create invitation' }));
  await screen.findByRole('button', { name: /the-secret-token/ });
  // The offer comes back in the list afterwards, and it carries no token: only
  // a digest is kept, so nothing can show the secret a second time.
  await screen.findByText('open');
  expect(calls.calls.filter((call) => call.method === 'POST')).toHaveLength(1);
  expect(JSON.stringify(calls.calls.filter((call) => call.method === 'GET'))).not.toContain(
    'the-secret-token',
  );
});

/** The grants half of a participant's workshop page, which every lab test needs. */
function participant(extra: Route[]) {
  const held = {
    project,
    role: 'viewer',
    evaluated_at: WEDNESDAY,
    grants: [{ grant: grant('g1', { role: 'viewer' }), role: 'viewer' }],
  };
  return server([
    { method: 'GET', path: '/projects', answer: { status: 200, body: [held] } },
    {
      method: 'GET',
      path: '/roster',
      answer: { status: 403, body: refusal('forbidden', 'not an administrator') },
    },
    { method: 'GET', path: '/access', answer: { status: 200, body: held } },
    {
      method: 'GET',
      path: '/grants',
      answer: { status: 403, body: refusal('forbidden', 'not an administrator') },
    },
    { method: 'GET', path: '/invitations', answer: { status: 200, body: [] } },
    ...extra,
  ]);
}

const CONTEXT = 'c'.repeat(64);

const summary = (parts: Record<string, unknown> = {}) => ({
  name: 'lab-03',
  is_published: true,
  versions: 2,
  updated_at: MONDAY,
  current: {
    version_id: 'a'.repeat(64),
    title: 'Answer the support questions',
    position: 3,
    has_tests: true,
    published_at: MONDAY,
  },
  ...parts,
});

const detail = (brief: string, tests: unknown) => ({
  head: {
    name: 'lab-03',
    labels: { published: 'a'.repeat(64) },
    versions: [],
    updated_at: MONDAY,
  },
  current: {
    version_id: 'a'.repeat(64),
    name: 'lab-03',
    title: 'Answer the support questions',
    brief,
    position: 3,
    tests,
    published_at: MONDAY,
  },
});

it('draws no lab slot it cannot fill', async () => {
  // The nine empty slots are gone. A workshop with no labs says it has none;
  // a placeholder exercise reads as one somebody forgot to write, which is the
  // same failure the nine slots were drawn to avoid the other way round.
  vi.useFakeTimers({ shouldAdvanceTime: true });
  participant([]);
  await open('/learning?organization=org&project=ret');

  const labs = (await screen.findByText('Labs')).closest('div[class*="rounded-lg"]') as HTMLElement;
  expect(await within(labs).findByText('This workshop has no labs yet')).toBeTruthy();
  expect(within(labs).queryAllByRole('listitem')).toHaveLength(0);
  // Nothing on this card may be a number: a progress figure nobody measured is
  // the fake this area exists not to draw.
  expect(within(labs).queryByText(/%/)).toBeNull();
});

it("reads a lab's tests and marks from the server, and computes no context id", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  const stub = participant([
    {
      method: 'GET',
      path: '/labs',
      answer: { status: 200, body: { labs: [summary()], total: 1 } },
    },
    {
      method: 'GET',
      path: '/labs/lab-03',
      answer: {
        status: 200,
        body: detail('Build an agent that answers these.', {
          scorecard: { name: 'support-quality', version: 'b'.repeat(64) },
          cases: 'd'.repeat(64),
        }),
      },
    },
    {
      method: 'GET',
      path: '/labs/lab-03/measurement',
      answer: {
        status: 200,
        body: {
          name: 'lab-03',
          version_id: 'a'.repeat(64),
          measurement: {
            context_id: CONTEXT,
            context: {
              dataset: { kind: 'curation', name: 'support-cases', version: 'e'.repeat(64) },
              case_manifest: { name: 'cases', uri: 's3://cases', digest: 'd'.repeat(64) },
              case_count: 12,
              split: 'test',
              suite: { name: 'support-quality', version: 'b'.repeat(64) },
              scorer: { name: 'aiwatcher.scoring', version: '2' },
              input_schema: { name: 'input', uri: 's3://in', digest: '1'.repeat(64) },
              expectations_schema: { name: 'expected', uri: 's3://out', digest: '2'.repeat(64) },
              metrics: [
                { name: 'exact', unit: 'ratio', direction: 'higher', aggregation: 'mean' },
                { name: 'cost', unit: 'usd', direction: 'lower', aggregation: 'sum' },
              ],
            },
          },
        },
      },
    },
    {
      method: 'GET',
      path: '/evaluation-results',
      answer: {
        status: 200,
        body: {
          evaluations: [
            {
              receipt: {
                evaluation_id: 'student-one',
                variant_id: 'f'.repeat(64),
                context_id: CONTEXT,
                committed_at: WEDNESDAY,
                expires_at: WEDNESDAY + DAY,
                version: '1',
              },
              metrics: { exact: 0.75 },
              state: 'complete',
              reproducible: true,
            },
          ],
        },
      },
    },
  ]);
  await open('/learning?organization=org&project=ret');

  const labs = (await screen.findByText('Labs')).closest('div[class*="rounded-lg"]') as HTMLElement;
  await userEvent.click(await within(labs).findByRole('button', { name: /Answer the support/ }));

  expect(await within(labs).findByText('Build an agent that answers these.')).toBeTruthy();
  expect(within(labs).getByText(/support-quality/)).toBeTruthy();
  expect(within(labs).getByText(/12 from/)).toBeTruthy();
  // The direction is the card's, drawn as the server sent it and never worked
  // out here from the metric's name.
  expect(within(labs).getByText('higher is better')).toBeTruthy();
  expect(within(labs).getByText('lower is better')).toBeTruthy();
  expect(within(labs).getByText('0.75')).toBeTruthy();

  // The one thing this page must not do: the results were asked for by the
  // context id the *server* answered, never by one composed here.
  const results = stub.calls.find((call) => call.url.endsWith('/evaluation-results'));
  expect(results?.search).toContain(`context_id=${CONTEXT}`);
  expect(stub.countOf('GET', '/labs/lab-03/measurement')).toBe(1);
});

it("says in the server's words why a lab has no measurement, and shows no marks", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  const stub = participant([
    {
      method: 'GET',
      path: '/labs',
      answer: {
        status: 200,
        body: {
          labs: [summary({ current: { ...summary().current, has_tests: false } })],
          total: 1,
        },
      },
    },
    {
      method: 'GET',
      path: '/labs/lab-03',
      answer: { status: 200, body: detail('Still being written.', null) },
    },
    {
      method: 'GET',
      path: '/labs/lab-03/measurement',
      answer: {
        status: 200,
        body: {
          name: 'lab-03',
          version_id: 'a'.repeat(64),
          unavailable: 'this lab pins no tests yet',
        },
      },
    },
  ]);
  await open('/learning?organization=org&project=ret');

  const labs = (await screen.findByText('Labs')).closest('div[class*="rounded-lg"]') as HTMLElement;
  await userEvent.click(await within(labs).findByRole('button', { name: /Answer the support/ }));

  expect(await within(labs).findByText('this lab pins no tests yet')).toBeTruthy();
  // No results heading at all: a lab with no measurement has no context to
  // list marks under, and an empty "Results" would read as nobody having done
  // the work rather than as nothing being measurable.
  expect(within(labs).queryByText('Results')).toBeNull();
  expect(stub.countOf('GET', '/evaluation-results')).toBe(0);
});

it('keeps the workshop being looked at in the URL', async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  server([
    {
      method: 'GET',
      path: '/projects',
      answer: {
        status: 200,
        body: [
          {
            project,
            role: 'viewer',
            evaluated_at: WEDNESDAY,
            grants: [{ grant: grant('g1', { role: 'viewer' }), role: 'viewer' }],
          },
        ],
      },
    },
    {
      method: 'GET',
      path: '/roster',
      answer: { status: 403, body: refusal('forbidden', 'not an administrator') },
    },
    {
      method: 'GET',
      path: '/access',
      answer: {
        status: 200,
        body: {
          project,
          role: 'viewer',
          evaluated_at: WEDNESDAY,
          grants: [{ grant: grant('g1', { role: 'viewer' }), role: 'viewer' }],
        },
      },
    },
    {
      method: 'GET',
      path: '/grants',
      answer: { status: 403, body: refusal('forbidden', 'not an administrator') },
    },
    { method: 'GET', path: '/invitations', answer: { status: 200, body: [] } },
  ]);
  const router = await open('/learning?organization=org');

  await userEvent.click(await screen.findByRole('button', { name: /Retrieval workshop/ }));
  expect(router.state.location.search).toEqual({ organization: 'org', project: 'ret' });
  await userEvent.click(await screen.findByRole('button', { name: 'All workshops' }));
  expect(router.state.location.search).toEqual({ organization: 'org' });
});

it('says why there is nothing to enrol anybody in when no provider is configured', async () => {
  serve([
    { method: 'GET', path: '/auth/config', answer: { status: 200, body: { enabled: false } } },
  ]);
  await open('/learning');
  await screen.findByText('This instance has no identity provider');
  expect(screen.queryByText('Organizations')).toBeNull();
});
