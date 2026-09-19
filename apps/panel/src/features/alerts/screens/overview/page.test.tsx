import {
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
} from '@tanstack/react-router';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';

import { AlertsPage } from './page';
import { searchSchema } from './search';
import { serve, withQueries } from '@/test/server';
import type { Route as ServerRoute } from '@/test/server';

afterEach(() => {
  vi.unstubAllGlobals();
});

const channel = {
  channel: { kind: 'webhook', variable: 'AIWATCHER_ALERT_WEBHOOK_URL', signed: true },
};

const rules = {
  total: 1,
  rules: [
    {
      name: 'nightly',
      enabled: true,
      versions: 2,
      updated_at: 1_758_000_000,
      current: {
        version_id: 'a'.repeat(64),
        trigger: 'execution_failed',
        description: 'the nightly import must not die quietly',
        published_at: 1_758_000_000,
      },
    },
  ],
};

function delivery(over: Record<string, unknown> = {}) {
  return {
    dedup_key: 'b'.repeat(64),
    rule: 'nightly',
    rule_version: 'a'.repeat(64),
    state: 'completed',
    attempts: 1,
    created_at: 1_758_000_000,
    available_at: 1_758_000_000,
    delivered_at: 1_758_000_010,
    payload: {
      dedup_key: 'b'.repeat(64),
      rule: 'nightly',
      rule_version: 'a'.repeat(64),
      trigger: 'execution_failed',
      subject: 'exec-1',
      title: 'house-import failed',
      description: 'the nightly import must not die quietly',
      occurred_at: 1_758_000_000,
      facts: [{ label: 'workflow', value: 'house-import' }],
      links: [],
    },
    ...over,
  };
}

function open(routes: ServerRoute[], at = '/alerts') {
  const server = serve(routes);
  const root = createRootRoute();
  const route = createRoute({
    getParentRoute: () => root,
    path: '/alerts',
    validateSearch: searchSchema,
    component: AlertsPage,
  });
  const router = createRouter({
    routeTree: root.addChildren([route]),
    history: createMemoryHistory({ initialEntries: [at] }),
  });
  render(withQueries(<RouterProvider router={router} />));
  return server;
}

const READS: ServerRoute[] = [
  { method: 'GET', path: '/alert-channel', answer: { status: 200, body: channel } },
  { method: 'GET', path: '/alert-rules', answer: { status: 200, body: rules } },
];

it('names the variable that decides where a notification goes, and never the address', async () => {
  open([
    ...READS,
    {
      method: 'GET',
      path: '/alert-deliveries',
      answer: { status: 200, body: { total: 0, deliveries: [] } },
    },
  ]);

  expect(await screen.findByText('webhook')).toBeTruthy();
  expect(screen.getByText('signed')).toBeTruthy();
  expect(screen.getByText('AIWATCHER_ALERT_WEBHOOK_URL')).toBeTruthy();
});

it('says a deployment with no channel keeps its rules rather than drawing an empty list', async () => {
  open([
    { method: 'GET', path: '/alert-channel', answer: { status: 200, body: { channel: null } } },
    { method: 'GET', path: '/alert-rules', answer: { status: 200, body: rules } },
    {
      method: 'GET',
      path: '/alert-deliveries',
      answer: { status: 200, body: { total: 0, deliveries: [] } },
    },
  ]);

  expect(await screen.findByText(/Nothing is configured, so nothing is sent/)).toBeTruthy();
  // And the rule is still there, because a rule with nowhere to go is still a
  // rule somebody wrote.
  expect(screen.getByText('nightly')).toBeTruthy();
});

it('draws what a delivery became and how many attempts it took, from the record', async () => {
  open([
    ...READS,
    {
      method: 'GET',
      path: '/alert-deliveries',
      answer: {
        status: 200,
        body: {
          total: 2,
          deliveries: [
            delivery({ state: 'failed', attempts: 3, last_error: 'the channel answered 503' }),
            delivery({ dedup_key: 'c'.repeat(64), attempts: 2 }),
          ],
        },
      },
    },
  ]);

  expect(await screen.findByText('the channel answered 503')).toBeTruthy();
  expect(screen.getByText('3 attempts')).toBeTruthy();
  // A row that was retried says so rather than claiming it went first time.
  expect(screen.getByText('2 attempts')).toBeTruthy();
  // Twice each: the badge on the row, and the button that filters by it.
  expect(screen.getAllByText('given up on').length).toBe(2);
  expect(screen.getAllByText('delivered').length).toBe(2);
});

it('offers Send again only for the one that failed', async () => {
  open([
    ...READS,
    {
      method: 'GET',
      path: '/alert-deliveries',
      answer: {
        status: 200,
        body: {
          total: 2,
          deliveries: [
            delivery({ state: 'failed', attempts: 3, last_error: 'gone' }),
            delivery({ dedup_key: 'c'.repeat(64) }),
          ],
        },
      },
    },
  ]);

  expect((await screen.findAllByRole('button', { name: 'Send again' })).length).toBe(1);
});

it('keeps the chosen state in the URL, so a filtered history is a link', async () => {
  const server = open([
    ...READS,
    {
      method: 'GET',
      path: '/alert-deliveries',
      answer: { status: 200, body: { total: 0, deliveries: [] } },
    },
  ]);
  await screen.findByText('webhook');

  await userEvent.click(screen.getByRole('button', { name: 'given up on' }));

  await vi.waitFor(() => {
    expect(
      server.calls.some(
        (call) => call.url.endsWith('/alert-deliveries') && call.search.includes('state=failed'),
      ),
    ).toBe(true);
  });
});

it('reports a refused read as a role rather than as nothing configured', async () => {
  open([
    { method: 'GET', path: '/alert-channel', answer: { status: 403 } },
    { method: 'GET', path: '/alert-rules', answer: { status: 403 } },
    { method: 'GET', path: '/alert-deliveries', answer: { status: 403 } },
  ]);

  expect(
    await screen.findByText('Reading what this deployment sends needs the admin role'),
  ).toBeTruthy();
});

it('reports a deployment with no object store by the variable that gives it one', async () => {
  open([
    { method: 'GET', path: '/alert-channel', answer: { status: 200, body: { channel: null } } },
    {
      method: 'GET',
      path: '/alert-rules',
      answer: { status: 501, body: { code: 'registry_disabled', message: 'not configured' } },
    },
    { method: 'GET', path: '/alert-deliveries', answer: { status: 501 } },
  ]);

  expect(await screen.findByText('This deployment keeps no alert rules')).toBeTruthy();
  expect(screen.getByText('AIWATCHER_PROMPT_STORE')).toBeTruthy();
});

it('reports what the receiver said to a test rather than that a test ran', async () => {
  open([
    ...READS,
    {
      method: 'GET',
      path: '/alert-deliveries',
      answer: { status: 200, body: { total: 0, deliveries: [] } },
    },
    {
      method: 'POST',
      path: '/alert-channel/test',
      answer: {
        status: 200,
        body: {
          delivered: false,
          error: 'the alert channel answered 404 Not Found',
          delivery: delivery({ state: 'failed', attempts: 1 }),
        },
      },
    },
  ]);
  await screen.findByText('webhook');

  await userEvent.click(screen.getByRole('button', { name: 'Send a test' }));

  expect(await screen.findByText('the receiver did not take it')).toBeTruthy();
  expect(screen.getByText('the alert channel answered 404 Not Found')).toBeTruthy();
});
