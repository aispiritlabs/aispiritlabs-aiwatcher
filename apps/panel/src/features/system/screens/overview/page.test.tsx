import { render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';

import { SystemPage } from './page';
import { refusal, serve, withQueries } from '@/test/server';

afterEach(() => {
  vi.unstubAllGlobals();
});

const inventory = {
  version: '0.1.0',
  capabilities: [
    {
      id: 'prompt-registry',
      label: 'Prompt registry',
      group: 'storage',
      state: 'configured',
      variables: ['AIWATCHER_PROMPT_STORE'],
      settings: [],
      note: 'Authored rather than observed, so a version outlives the runs that used it.',
    },
    {
      id: 'query-engine',
      label: 'Query engine',
      group: 'runtime',
      state: 'configured',
      variables: ['AIWATCHER_QUERY_ENGINE'],
      settings: [{ name: 'engine', value: 'duckdb', variable: 'AIWATCHER_QUERY_ENGINE' }],
      note: 'One per deployment.',
    },
    {
      id: 'dataset-hubs',
      label: 'Dataset hubs',
      group: 'integration',
      state: 'not_configured',
      variables: ['AIWATCHER_HUGGINGFACE_ENABLED', 'AIWATCHER_KAGGLE_KEY'],
      settings: [],
      note: 'The only thing here that reaches a service aiwatcher does not run.',
    },
  ],
};

function open(answer: { status: number; body?: unknown }) {
  serve([{ method: 'GET', path: '/system', answer }]);
  render(withQueries(<SystemPage />));
}

it('draws every capability with its state, its values and the variables that decide it', async () => {
  open({ status: 200, body: inventory });

  expect(await screen.findByText('Prompt registry')).toBeTruthy();
  expect(screen.getAllByText('configured').length).toBe(2);
  expect(screen.getByText('not configured')).toBeTruthy();
  // A value the server said is safe to say, beside the variable that set it —
  // and that variable named once, not again underneath.
  expect(screen.getByText('duckdb')).toBeTruthy();
  expect(screen.getAllByText(/AIWATCHER_QUERY_ENGINE/).length).toBe(1);
  // And the variables, including for the row that has nothing wired — which is
  // the whole reason a `not configured` row is worth reading rather than just
  // worth noticing.
  expect(screen.getByText('AIWATCHER_KAGGLE_KEY')).toBeTruthy();
  expect(screen.getByText('v0.1.0')).toBeTruthy();
});

it('says a refused read needs the admin role rather than drawing an instance with nothing wired', async () => {
  // The failure this exists to catch. The generated client resolves on a 403,
  // so a page that read `data` without checking would render an inventory of
  // no capabilities at all — which, on this screen of all screens, reads as
  // "this deployment has nothing configured".
  open({ status: 403, body: refusal('forbidden', 'admin required') });

  expect(await screen.findByText(/needs the admin role/)).toBeTruthy();
  expect(screen.queryByText('configured')).toBeNull();
  expect(screen.queryByText('not configured')).toBeNull();
});

it('draws a failed read as a failure, never as a deployment with nothing configured', async () => {
  open({ status: 503, body: refusal('unavailable', 'the instance is not answering') });

  expect(await screen.findByRole('alert')).toBeTruthy();
  expect(screen.getByText('the instance is not answering')).toBeTruthy();
  expect(screen.queryByText(/needs the admin role/)).toBeNull();
});
