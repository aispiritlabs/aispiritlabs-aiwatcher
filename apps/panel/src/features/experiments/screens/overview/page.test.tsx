import { render, screen, waitFor } from '@testing-library/react';
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
                cost: {
                  currency: 'USD',
                  amount: 0.0004,
                  priced_calls: 4,
                  unpriced_calls: 0,
                  prices: [
                    {
                      model: 'capitals-stand-in',
                      source: 'https://example.com/pricing',
                      as_of: '2026-09-13',
                    },
                  ],
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
          observed: [
            {
              variant_id: 'candidate-variant',
              runs: 20,
              succeeded: 19,
              failed: 1,
              running: 0,
              measured_runs: 4,
              duration_ms: { count: 20, p50: 800, p90: 2_000, p99: 4_000, max: 4_000 },
              call_ms: {
                count: 20,
                p50: 400,
                p90: 900,
                p99: 1_500,
                max: 1_500,
                bucketed: true,
              },
              time_to_first_token_ms: { count: 20, p50: 150, p90: 300, p99: 500, max: 500 },
              llm_calls: 20,
              input_tokens: 2_000,
              output_tokens: 300,
              periods: 3,
              runs_from_periods: 12,
              incomplete_periods: 1,
              late_runs: 2,
              counted_from: '2026-09-13T09:05:00Z',
              missed: [{ events: 40, from: '2026-09-13T09:10:00Z', until: '2026-09-13T09:12:30Z' }],
              lost_events: 3,
              lost_runs: 2,
              window_before_observations: true,
              cost: {
                currency: 'USD',
                amount: 0.008,
                priced_calls: 18,
                priced_before_read: 3,
                unpriced_calls: 2,
                unpriced_models: ['local-llama'],
                prices: [
                  {
                    model: 'gpt-4o',
                    source: 'https://openai.com/api/pricing',
                    as_of: '2026-09-01',
                  },
                ],
              },
            },
            {
              variant_id: 'baseline-variant',
              runs: 0,
              succeeded: 0,
              failed: 0,
              running: 0,
              measured_runs: 4,
              llm_calls: 0,
              input_tokens: 0,
              output_tokens: 0,
              periods: 0,
              runs_from_periods: 0,
              incomplete_periods: 0,
              late_runs: 0,
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
  // A result's cases priced by the model they called, with the day of the price.
  expect(
    screen.getByText('0.0004 USD for 4 priced calls, at capitals-stand-in as of 2026-09-13'),
  ).toBeTruthy();
  expect(screen.getByText('not measured')).toBeTruthy();
  expect(screen.getByText('10.60 s')).toBeTruthy();
  expect(screen.getByText('not in the log')).toBeTruthy();
  expect(screen.getByText('+1.0000')).toBeTruthy();

  // What each variant was observed doing, apart from what it scored — and a
  // variant seen only in its own measurement is not drawn as traffic.
  const observed = screen.getByRole('link', { name: '20 runs · 1 failed' });
  expect(observed.getAttribute('href')).toContain('by=variant');
  expect(screen.getByText('800 ms / 2.00 s / 4.00 s')).toBeTruthy();
  expect(
    screen.getByText(
      /over 20 finished · 2,000 \/ 300 tokens in 20 calls · 4 measured runs left out · 12 runs from 3 written periods, 1 incomplete · counted from 2026-09-13 09:05:00 UTC, nothing observed before · 2 reached the log after their period closed · 40 events from 2026-09-13 09:10:00 to 2026-09-13 09:12:30 UTC never reached the fold, so runs that ended then may be missing · 3 events the runs' clients numbered never reached the fold, so those runs are counted with what did · 2 runs the clients numbered never reached the fold with their start — each lost whole, or counted with what did arrive/,
    ),
  ).toBeTruthy();
  // Each call's time, bucketed where written periods are in it, and a cost
  // that says which prices it rests on and what it could not price.
  expect(screen.getByText('a call ≤ 400 ms / 900 ms / 1.50 s · first token 150 ms')).toBeTruthy();
  expect(
    screen.getByText(
      /0\.008 USD for 18 priced calls, at gpt-4o as of 2026-09-01 · 3 made before any price for their model was read, priced by the earliest · 2 calls unpriced \(local-llama\)/,
    ),
  ).toBeTruthy();
  expect(screen.getByText('no runs outside a measurement (4 measured)')).toBeTruthy();

  await userEvent.click(screen.getByRole('button', { name: '1h' }));
  expect(router.state.location.search).toEqual({ context: CONTEXT, window: 3600 });
  // Another window is another read of the observations.
  await waitFor(() =>
    expect(server.calls.filter((call) => call.url.endsWith(`/experiments/${CONTEXT}`)).length).toBe(
      2,
    ),
  );

  const buttons = await screen.findAllByRole('button', { name: 'Use as baseline' });
  await userEvent.click(buttons[buttons.length - 1] as HTMLElement);
  expect(router.state.location.search).toEqual({
    context: CONTEXT,
    baseline: 'capitals-baseline',
    window: 3600,
  });
  expect(await screen.findByText('baseline', { selector: 'span' })).toBeTruthy();
});
