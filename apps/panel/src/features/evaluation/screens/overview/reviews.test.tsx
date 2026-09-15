import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';

import { serve, withQueries } from '@/test/server';

import { Reviews } from './reviews';

afterEach(() => vi.unstubAllGlobals());

const ID = 'a'.repeat(64);

function item(state: string, extra: Record<string, unknown> = {}) {
  return {
    id: ID,
    dataset: 'capitals',
    target: { kind: 'trace', trace_id: '4bf92f3577b34da6a3ce929d0e0e4736' },
    question: 'What is the capital of Kenya?',
    answer: 'Mombasa',
    content: 'written',
    proposed_by: 'ada',
    proposed_at: 1,
    state,
    revision: 1,
    recorded_by: 'ada',
    recorded_at: 1,
    ...extra,
  };
}

it('proposes a case from a trace, and approves and publishes what people wrote', async () => {
  let listed = 0;
  const server = serve([
    { method: 'GET', path: '/auth/config', answer: { status: 200, body: { enabled: false } } },
    {
      method: 'GET',
      path: '/evaluation-reviews',
      answer: () => {
        listed += 1;
        return {
          status: 200,
          body: {
            dataset: 'capitals',
            items: [
              listed === 1
                ? item('proposed')
                : item('approved', { expected: 'Nairobi', decided_by: 'grace' }),
            ],
          },
        };
      },
    },
    {
      method: 'POST',
      path: '/evaluation-reviews',
      answer: { status: 200, body: { review: item('proposed'), created: false } },
    },
    {
      method: 'POST',
      path: `/evaluation-reviews/${ID}/actions`,
      answer: { status: 200, body: item('ready', { expected: 'Nairobi', split: 'dev' }) },
    },
    {
      method: 'POST',
      path: '/evaluation-reviews/publish',
      answer: {
        status: 200,
        body: {
          dataset: {
            created: true,
            dataset: {
              name: 'capitals',
              versions: [],
              latest: {
                version: 'v'.repeat(64),
                created_at: '2026-09-13T10:00:00Z',
                row_count: 5,
                columns: [],
              },
            },
          },
          published: [item('published', { published_in: 'v'.repeat(64) })],
        },
      },
    },
  ]);
  const onDataset = vi.fn();
  render(
    withQueries(
      <Reviews
        seed={{ dataset: 'capitals', trace: '4bf92f3577b34da6a3ce929d0e0e4736' }}
        onDataset={onDataset}
      />,
    ),
  );

  expect(await screen.findByText('What is the capital of Kenya?')).toBeTruthy();
  expect((screen.getByLabelText('Trace ID') as HTMLInputElement).value).toBe(
    '4bf92f3577b34da6a3ce929d0e0e4736',
  );
  await userEvent.type(screen.getByLabelText('Question'), 'What is the capital of Kenya?');
  await userEvent.type(screen.getByLabelText('Split'), 'test');
  await userEvent.click(screen.getByRole('button', { name: 'Propose as a case' }));
  expect(await screen.findByText(/Already under review/)).toBeTruthy();
  const proposed = server.calls.find(
    (call) => call.method === 'POST' && call.url.endsWith('/evaluation-reviews'),
  );
  expect(proposed?.body).toMatchObject({
    dataset: 'capitals',
    target: { kind: 'trace', trace_id: '4bf92f3577b34da6a3ce929d0e0e4736' },
    content: 'written',
    split: 'test',
  });
  // A case naming no split says it joins every split's cohort.
  expect(screen.getByText(/joins every split/)).toBeTruthy();

  await userEvent.type(
    screen.getByLabelText('Expected answer for What is the capital of Kenya?'),
    'Nairobi',
  );
  await userEvent.type(screen.getByLabelText('Split for What is the capital of Kenya?'), 'dev');
  await userEvent.click(screen.getByRole('button', { name: 'Save expected' }));
  await waitFor(() =>
    expect(
      server.calls.find((call) => call.url.endsWith(`/evaluation-reviews/${ID}/actions`))?.body,
    ).toEqual({ action: 'expect', expected: 'Nairobi', split: 'dev' }),
  );

  const publish = await screen.findByRole('button', {
    name: 'Publish 1 approved as a new version',
  });
  await userEvent.click(publish);
  expect(await screen.findByText(/with 5 rows/)).toBeTruthy();
});
