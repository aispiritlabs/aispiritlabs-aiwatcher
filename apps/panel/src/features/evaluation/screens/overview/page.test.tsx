import * as React from 'react';
import { render, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { EvaluationPage } from './page';
import { serve, withQueries } from '@/test/server';

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children }: { children: React.ReactNode }) => <a>{children}</a>,
  getRouteApi: () => ({
    useSearch: () => ({ report: 'candidate', baseline: 'failed' }),
    useNavigate: () => () => Promise.resolve(),
  }),
}));
afterEach(() => vi.unstubAllGlobals());

it('sends the explicit baseline and shows incompatibility, coverage and both parameter sets', async () => {
  const summary = {
    evaluation_id: 'candidate',
    suite: 'suite',
    dataset: 'data',
    status: 'succeeded',
    started_at: '2026-09-11T09:00:00Z',
    runtime: 'test',
    params: { lr: '0.1' },
    metrics: { score: 0.8 },
    cases_total: 10,
    cases_passed: 8,
    cases_failed: 2,
    report_bytes: 0,
    report_dropped: false,
    context: {},
  };
  serve([
    { method: 'GET', path: '/auth/config', answer: { status: 200, body: { enabled: false } } },
    { method: 'GET', path: '/evaluation-suites', answer: { status: 200, body: { suites: [] } } },
    {
      method: 'GET',
      path: '/evaluations',
      answer: { status: 200, body: { evaluations: [], total_known: 0 } },
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
                evaluation_id: 'kept-1',
                version: 'ff00',
                variant_id: 'aa11',
                context_id: 'bb22',
                committed_at: 1789200000,
                expires_at: 1791792000,
              },
              state: 'complete',
              metrics: {},
              counts: { selected: 3, scored: 3, failed: 0, unscored: 0 },
            },
          ],
          retention: { ran_at: 1789200000, retired: 0, collected: 0, failures: 0 },
        },
      },
    },
    {
      method: 'GET',
      path: '/evaluations/candidate',
      answer: {
        status: 200,
        body: {
          summary,
          cases: [],
          cases_truncated: true,
          comparison: {
            baseline_id: 'failed',
            baseline_started_at: summary.started_at,
            baseline_summary: {
              ...summary,
              evaluation_id: 'failed',
              status: 'failed',
              params: { lr: '0.2' },
            },
            comparability: 'incompatible',
            reasons: ['Both evaluations must have succeeded'],
            metrics: [{ name: 'score', current: 0.8, baseline: 0.5 }],
            regressed: [],
            fixed: [],
            common_cases: 0,
            current_cases_retained: 0,
            baseline_cases_retained: 2,
            current_cases_complete: false,
            baseline_cases_complete: false,
            details_complete: false,
          },
        },
      },
    },
  ]);
  const fetcher = globalThis.fetch;
  const requests: string[] = [];
  vi.stubGlobal('fetch', (input: Request, init?: RequestInit) => {
    requests.push(input.url);
    return fetcher(input, init);
  });
  render(withQueries(<EvaluationPage />));
  await screen.findByText('Both evaluations must have succeeded');
  expect(requests.some((url) => url.includes('candidate?baseline_id=failed'))).toBe(true);
  expect(screen.getByText(/Partial details/)).toBeTruthy();
  expect(screen.getByText(/Common retained cases: 0/)).toBeTruthy();
  expect(screen.getByText('lr · differs')).toBeTruthy();
  expect(screen.getByText('0.2')).toBeTruthy();
  expect(screen.queryByText('Regressed')).toBeNull();
  // A3's three states are a control, not a fourth sentence in a paragraph, and
  // the delta the server refused to compute has to look refused. An empty cell
  // there reads as "no change".
  const comparability = screen.getByRole('group', { name: 'Comparability' });
  expect(within(comparability).getByText('incompatible').getAttribute('aria-current')).toBe('true');
  expect(within(comparability).getByText('comparable').getAttribute('aria-current')).toBeNull();
  expect(screen.getByText('withheld')).toBeTruthy();
  // Whether retention is running, beside the catalogue it prunes.
  expect(screen.getByText(/Retention last ran/)).toBeTruthy();
});

it('keeps a missing explicit baseline visible as an error instead of claiming an automatic result', async () => {
  serve([
    { method: 'GET', path: '/auth/config', answer: { status: 200, body: { enabled: false } } },
    {
      method: 'GET',
      path: '/evaluations/candidate',
      answer: { status: 404, body: { code: 'not_found' } },
    },
  ]);
  render(withQueries(<EvaluationPage />));
  await screen.findByText('Could not load the report or requested baseline');
  expect((screen.getByLabelText('Baseline ID') as HTMLInputElement).value).toBe('failed');
});
