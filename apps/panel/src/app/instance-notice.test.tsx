import {
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
} from '@tanstack/react-router';
import { render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { z } from 'zod';

import { client } from '@/api/generated/client.gen';
import { InstanceOnly } from './instance-notice';
import { serve, withQueries, type Route as Served } from '@/test/server';
import { setActiveScope } from '@/shared/lib/scope';

const SIGNED_IN: Served = {
  method: 'GET',
  path: '/auth/config',
  answer: { status: 200, body: { enabled: true, mode: 'oidc', provider: 'Authentik' } },
};

/** A session holding the roles a deployment's mapping gave it. */
const session = (roles: string[]): Served => ({
  method: 'GET',
  path: '/auth/me',
  answer: { status: 200, body: { subject: 'somebody', roles, credential: 'session' } },
});

const ORGANIZATIONS: Served = {
  method: 'GET',
  path: '/iam/organizations',
  answer: { status: 200, body: [{ id: 'org-1', name: 'AI Spirit' }] },
};

const PROJECTS: Served = {
  method: 'GET',
  path: '/iam/organizations/org-1/projects',
  answer: {
    status: 200,
    body: [
      {
        project: { name: 'Demo for a client', scope: { organization: 'org-1', project: 'p-1' } },
        role: 'editor',
        grants: [],
        evaluated_at: 1_700_000_000,
      },
    ],
  },
};

beforeEach(() => {
  client.setConfig({ baseUrl: 'http://panel.test' });
  setActiveScope(undefined);
});
afterEach(() => {
  vi.unstubAllGlobals();
  setActiveScope(undefined);
});

function mount(page: string) {
  const root = createRootRoute({
    validateSearch: z.object({ scope: z.string().optional() }),
    component: () => (
      <InstanceOnly>
        <p>the page itself</p>
      </InstanceOnly>
    ),
  });
  const routes = ['/observability/runs', '/system', '/prompts'].map((path) =>
    createRoute({ getParentRoute: () => root, path, component: () => null }),
  );
  render(
    withQueries(
      <RouterProvider
        router={createRouter({
          routeTree: root.addChildren(routes),
          history: createMemoryHistory({ initialEntries: [page] }),
        })}
      />,
    ),
  );
}

it('tells a client with no instance role that the page is the deployment’s, and where theirs is', async () => {
  // The alternative is what they saw before: a page of failed reads, which
  // says aiwatcher is broken rather than this part is not yours.
  serve([SIGNED_IN, session([]), ORGANIZATIONS, PROJECTS]);
  mount('/system');
  await waitFor(() =>
    expect(screen.getByText(/belongs to the deployment, not to you/i)).toBeTruthy(),
  );
  expect(screen.getByText(/Demo for a client/)).toBeTruthy();
  expect(screen.queryByText('the page itself')).toBeNull();
});

it('draws a project area for the same caller, because that one is theirs', async () => {
  serve([SIGNED_IN, session([]), ORGANIZATIONS, PROJECTS]);
  mount('/prompts');
  await waitFor(() => expect(screen.getByText('the page itself')).toBeTruthy());
});

it('draws an instance area for somebody who holds a role on the instance', async () => {
  serve([SIGNED_IN, session(['admin']), ORGANIZATIONS, PROJECTS]);
  mount('/system');
  await waitFor(() => expect(screen.getByText('the page itself')).toBeTruthy());
});

it('draws everything on a deployment with no sign-in, where there is nobody to refuse', async () => {
  serve([
    { method: 'GET', path: '/auth/config', answer: { status: 200, body: { enabled: false } } },
  ]);
  mount('/system');
  await waitFor(() => expect(screen.getByText('the page itself')).toBeTruthy());
});
