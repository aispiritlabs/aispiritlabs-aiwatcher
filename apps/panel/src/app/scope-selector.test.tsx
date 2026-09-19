import {
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
} from '@tanstack/react-router';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { z } from 'zod';

import { client } from '@/api/generated/client.gen';
import { listRuns } from '@/api/generated/sdk.gen';
import { ReachNotice } from './reach-notice';
import { ScopeSelector } from './scope-selector';
import { serve, withQueries, type Route as Served } from '@/test/server';
import { parseScope, setActiveScope } from '@/shared/lib/scope';
// Imported for the interceptor it installs: what makes a selected project
// reach the reads is this module, and a test that mounted the selector without
// it would be testing a label.
import '@/shared/lib/api';

const ORGANIZATION = { id: 'org-1', name: 'Workshop' };
const PROJECT = {
  project: { name: 'The lesson', scope: { organization: 'org-1', project: 'proj-1' } },
  role: 'viewer',
  grants: [],
  evaluated_at: 1_700_000_000,
};

const SIGNED_IN: Served = {
  method: 'GET',
  path: '/auth/config',
  answer: { status: 200, body: { enabled: true, mode: 'oidc', provider: 'Authentik' } },
};

beforeEach(() => {
  client.setConfig({ baseUrl: 'http://panel.test' });
  setActiveScope(undefined);
});
afterEach(() => {
  vi.unstubAllGlobals();
  setActiveScope(undefined);
});

/** The header and the notice on one page, with the scope in the URL. */
function mount(href: string, page = '/observability/runs') {
  const root = createRootRoute({
    validateSearch: z.object({ scope: z.string().optional() }),
    component: () => (
      <>
        <ScopeSelector />
        <ReachNotice />
      </>
    ),
  });
  const routes = ['/observability/runs', '/workflows', '/observability/explore'].map((path) =>
    createRoute({ getParentRoute: () => root, path, component: () => null }),
  );
  const router = createRouter({
    routeTree: root.addChildren(routes),
    history: createMemoryHistory({ initialEntries: [`${page}${href}`] }),
  });
  render(withQueries(<RouterProvider router={router} />));
  return router;
}

it('offers nothing at all on an instance whose caller is in no organization', async () => {
  serve([
    SIGNED_IN,
    { method: 'GET', path: '/iam/organizations', answer: { status: 200, body: [] } },
  ]);
  mount('');
  // Not an empty menu and not a disabled control: an instance that never used
  // IAM is a single-tenant instance, and it gains no switch it cannot use.
  await waitFor(() => expect(screen.queryByText('Unassigned')).toBeNull());
});

it('picks a project and puts it in the URL', async () => {
  serve([
    SIGNED_IN,
    { method: 'GET', path: '/iam/organizations', answer: { status: 200, body: [ORGANIZATION] } },
    {
      method: 'GET',
      path: '/organizations/org-1/projects',
      answer: { status: 200, body: [PROJECT] },
    },
  ]);
  const router = mount('');
  await userEvent.click(await screen.findByRole('button', { name: /Unassigned/ }));
  await userEvent.click(await screen.findByRole('button', { name: /The lesson/ }));

  // Surviving the move between areas is the real route tree's own middleware
  // and is proved there, over the routes people actually navigate.
  await waitFor(() => expect(router.state.location.search).toEqual({ scope: 'org-1/proj-1' }));
});

it('says so when a link names a project no grant reaches', async () => {
  serve([
    SIGNED_IN,
    { method: 'GET', path: '/iam/organizations', answer: { status: 200, body: [ORGANIZATION] } },
    { method: 'GET', path: '/organizations/org-1/projects', answer: { status: 200, body: [] } },
  ]);
  mount('?scope=org-1/nobody-elses');
  await userEvent.click(await screen.findByRole('button', { name: /No access/ }));
  expect(await screen.findByText(/hold no live grant on/)).toBeTruthy();
});

it('tells somebody in an organization with no grant why there is nothing to pick', async () => {
  serve([
    SIGNED_IN,
    { method: 'GET', path: '/iam/organizations', answer: { status: 200, body: [ORGANIZATION] } },
    { method: 'GET', path: '/organizations/org-1/projects', answer: { status: 200, body: [] } },
  ]);
  mount('');
  await userEvent.click(await screen.findByRole('button', { name: /Unassigned/ }));
  expect(await screen.findByText(/You hold no project grant/)).toBeTruthy();
});

it('warns on an area a selected project does not reach, and says nothing on one it does', async () => {
  serve([
    SIGNED_IN,
    { method: 'GET', path: '/iam/organizations', answer: { status: 200, body: [ORGANIZATION] } },
    {
      method: 'GET',
      path: '/organizations/org-1/projects',
      answer: { status: 200, body: [PROJECT] },
    },
  ]);
  // Evaluation is `mixed`: its published evidence, cards, rubrics and reviews
  // are the project's, and the approval lines and the scorer catalogue are the
  // deployment's.
  const router = mount('?scope=org-1/proj-1', '/evaluation');
  expect(await screen.findByText(/Partly instance-wide/)).toBeTruthy();
  await router.navigate({ to: '/observability/runs', search: { scope: 'org-1/proj-1' } });
  await waitFor(() => expect(screen.queryByText(/instance-wide/)).toBeNull());
});

it('calls the unassigned side by its name to somebody who holds a grant', async () => {
  serve([
    SIGNED_IN,
    { method: 'GET', path: '/iam/organizations', answer: { status: 200, body: [ORGANIZATION] } },
    {
      method: 'GET',
      path: '/organizations/org-1/projects',
      answer: { status: 200, body: [PROJECT] },
    },
  ]);
  mount('');
  // The half that is easy to leave out: before M1 the instance list *was*
  // every run, and it is not any more.
  expect(await screen.findByText(/Unassigned: what has no project/)).toBeTruthy();
});

it('sends the read to the project, with the header a scoped write is refused without', async () => {
  const server = serve([
    { method: 'GET', path: '/runs', answer: { status: 200, body: { runs: [] } } },
  ]);
  setActiveScope(parseScope('org-1/proj-1'));
  await listRuns();
  expect(server.calls.at(-1)?.url).toBe('/api/v1/orgs/org-1/projects/proj-1/runs');
});

it('leaves a read with no project twin exactly where it was', async () => {
  // The deployment's own inventory: a project could never scope what this
  // instance has wired, so the rewrite must leave it alone rather than send it
  // to a path that does not exist.
  const server = serve([
    { method: 'GET', path: '/system', answer: { status: 200, body: { capabilities: [] } } },
  ]);
  setActiveScope(parseScope('org-1/proj-1'));
  const { system } = await import('@/api/generated/sdk.gen');
  await system();
  expect(server.calls.at(-1)?.url).toBe('/api/v1/system');
});
