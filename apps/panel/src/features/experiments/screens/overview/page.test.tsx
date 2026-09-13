import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import {
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
  RouterProvider,
} from '@tanstack/react-router';
import { afterEach, expect, it, vi } from 'vitest';

import { serve, withQueries } from '@/test/server';

import { ExperimentsPage } from './page';
import { searchSchema } from './search';

afterEach(() => vi.unstubAllGlobals());

const CONTEXT = 'c'.repeat(64);

function row(
  id: string,
  experiment: string,
  accuracy: number,
  extra: Record<string, unknown> = {},
) {
  return {
    evaluation_id: id,
    variant_id: `${experiment}-variant`,
    state: 'complete',
    committed_at: 1_800_000_000,
    variant: {
      schema_version: 1,
      experiment_id: experiment,
      dataset: { kind: 'curation', name: 'capitals', version: 'v'.repeat(64) },
      prompt: { name: 'capitals', version: 'p'.repeat(64) },
      code: { name: 'code', uri: 'file://a', digest: 'd'.repeat(64) },
      generation_config: { name: 'config', uri: 'file://b', digest: 'e'.repeat(64) },
    },
    origin: {
      evaluation_id: id,
      repetition_id: 'measurement-1',
      execution_id: `exec-${experiment}`,
      step_id: 'score',
    },
    status: 'succeeded',
    counts: { selected: 4, scored: 4, failed: 0, unscored: 0 },
    metrics: { exact: accuracy },
    reproducible: true,
    ...extra,
  };
}

function serving() {
  return serve([
    { method: 'GET', path: '/auth/config', answer: { status: 200, body: { enabled: false } } },
    {
      method: 'GET',
      path: '/experiments',
      answer: {
        status: 200,
        body: {
          truncated: false,
          experiments: [
            {
              context_id: CONTEXT,
              results: 2,
              variants: 2,
              latest_committed_at: 1_800_000_000,
              suite: { name: 'capitals-exact', version: 's'.repeat(64) },
              dataset: { kind: 'curation', name: 'capitals', version: 'v'.repeat(64) },
              split: 'test',
              case_count: 4,
            },
          ],
        },
      },
    },
    {
      method: 'GET',
      path: `/experiments/${CONTEXT}`,
      answer: () => ({
        status: 200,
        body: {
          experiment: {
            context_id: CONTEXT,
            metrics: [{ name: 'exact', unit: 'ratio', direction: 'higher', aggregation: 'rate' }],
            truncated: false,
            rows: [
              row('capitals-candidate', 'candidate', 1, {
                usage: {
                  latency_ms: { cases: 4, p50: 120, p90: 400, p99: 1500, max: 1500 },
                  output_tokens: { cases: 4, total: 40 },
                },
                comparison: {
                  comparability: 'comparable',
                  reasons: [],
                  same_variant: false,
                  judged: false,
                  metrics: [
                    { name: 'exact', direction: 'higher', current: 1, baseline: 0, delta: 1 },
                  ],
                },
              }),
              row('capitals-baseline', 'baseline', 0),
            ],
          },
          executions: [
            {
              workflow_run_id: 'exec-candidate',
              workflow_id: 'evaluation-run',
              status: 'succeeded',
              started_at: '2026-09-13T10:00:00Z',
              last_activity_at: '2026-09-13T10:00:10Z',
              duration_ms: 10_600,
              nodes_total: 3,
              nodes_pending: 0,
            },
          ],
        },
      }),
    },
  ]);
}

function at(url: string) {
  const root = createRootRoute();
  const route = createRoute({
    getParentRoute: () => root,
    path: '/experiments',
    validateSearch: searchSchema,
    component: ExperimentsPage,
  });
  return createRouter({
    routeTree: root.addChildren([route]),
    history: createMemoryHistory({ initialEntries: [url] }),
  });
}

it('lists the contexts, and opens one as its variants beside the chosen baseline', async () => {
  vi.stubGlobal('scrollTo', () => {});
  const server = serving();
  const router = at('/experiments');
  render(withQueries(<RouterProvider router={router} />));

  await userEvent.click(await screen.findByRole('button', { name: /curation capitals/ }));
  expect(await screen.findByText('capitals-candidate')).toBeTruthy();
  expect(router.state.location.search).toEqual({ context: CONTEXT });

  expect(screen.getByText('120 ms / 400 ms / 1.50 s')).toBeTruthy();
  expect(screen.getByText('over 4 of 4 cases')).toBeTruthy();
  expect(screen.getByText('— / 40')).toBeTruthy();
  expect(screen.getByText('not measured')).toBeTruthy();
  expect(screen.getByText('10.60 s')).toBeTruthy();
  expect(screen.getByText('not in the log')).toBeTruthy();
  expect(screen.getByText('+1.0000')).toBeTruthy();

  const buttons = screen.getAllByRole('button', { name: 'Use as baseline' });
  await userEvent.click(buttons[buttons.length - 1] as HTMLElement);
  expect(router.state.location.search).toEqual({ context: CONTEXT, baseline: 'capitals-baseline' });
  expect(await screen.findByText('baseline', { selector: 'span' })).toBeTruthy();
  expect(server.calls.filter((call) => call.url.endsWith(`/experiments/${CONTEXT}`)).length).toBe(
    2,
  );
});
