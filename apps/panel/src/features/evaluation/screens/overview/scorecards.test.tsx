import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';

import { serve, withQueries } from '@/test/server';

import { Scorecards } from './scorecards';

afterEach(() => vi.unstubAllGlobals());

const VERSION = 'c'.repeat(64);
const RUBRIC = 'r'.repeat(64);

const CATALOG = {
  recorded_at: 1,
  recorded_by: 'work-1',
  catalog: {
    contract: 1,
    adapters: [
      {
        name: 'deepeval',
        version: '4.2.2',
        model: { name: 'gemma-4-e2b', version: 'ud-q4-k-xl' },
        metrics: [
          {
            metric: 'answer_relevancy',
            unit: 'score',
            direction: 'higher',
            aggregation: 'mean',
            reads: ['input', 'answer'],
            model_graded: true,
            range: [0, 1],
            parameters: {
              threshold: { kind: 'number' },
              strict_mode: { kind: 'boolean' },
            },
          },
        ],
      },
    ],
  },
};

function serving(catalog: { status: number; body: unknown }) {
  return serve([
    { method: 'GET', path: '/auth/config', answer: { status: 200, body: { enabled: false } } },
    {
      method: 'GET',
      path: '/evaluation-scorecards',
      answer: {
        status: 200,
        body: {
          scorecards: [
            {
              name: 'answer-quality',
              version: VERSION,
              updated_at: 1,
              metrics: [{ name: 'exact', unit: 'ratio', direction: 'higher', aggregation: 'rate' }],
            },
          ],
        },
      },
    },
    { method: 'GET', path: '/evaluation-scorers', answer: catalog },
    {
      method: 'GET',
      path: '/evaluation-rubrics',
      answer: {
        status: 200,
        body: {
          rubrics: [{ name: 'helpful', question: 'Helps?', updated_at: 1, version: RUBRIC }],
        },
      },
    },
    {
      method: 'GET',
      path: '/evaluation-rubrics/helpful',
      answer: {
        status: 200,
        body: {
          version: RUBRIC,
          published_at: 1,
          published_by: 'ada',
          rubric: {
            name: 'helpful',
            question: 'Helps?',
            direction: 'higher',
            scale: { kind: 'ordinal', levels: ['poor', 'fair', 'good'] },
          },
        },
      },
    },
    {
      method: 'POST',
      path: '/evaluation-scorecards',
      answer: {
        status: 200,
        body: {
          version: 'd'.repeat(64),
          published_at: 2,
          published_by: 'ada',
          scorecard: {
            name: 'framework',
            scorers: [
              {
                metric: 'relevancy',
                input_path: '/question',
                scorer: {
                  kind: 'external',
                  adapter: 'deepeval',
                  metric: 'answer_relevancy',
                  declared: {
                    version: '4.2.2',
                    model: { name: 'gemma-4-e2b', version: 'ud-q4-k-xl' },
                    unit: 'score',
                    direction: 'higher',
                    aggregation: 'mean',
                    reads: ['input', 'answer'],
                  },
                },
              },
            ],
          },
        },
      },
    },
  ]);
}

it('lists the cards, and publishes one of the compiled scorers as the server reads it', async () => {
  const server = serving({ status: 200, body: CATALOG });
  render(withQueries(<Scorecards />));
  expect(await screen.findByText('answer-quality')).toBeTruthy();
  expect(screen.getByText(/exact \(ratio, higher\)/)).toBeTruthy();

  await userEvent.type(screen.getByLabelText('Card name'), 'framework');
  await userEvent.type(screen.getByLabelText('Scorer 1 metric'), 'exact');
  await userEvent.type(screen.getByLabelText('Scorer 1 answer path'), '/text');
  await userEvent.click(screen.getByLabelText('Scorer 1 ignores case'));
  await userEvent.click(screen.getByRole('button', { name: 'Publish card' }));

  await waitFor(() => expect(server.countOf('POST', '/evaluation-scorecards')).toBe(1));
  const sent = server.calls.find((call) => call.method === 'POST')?.body as Record<string, any>;
  expect(sent).toEqual({
    name: 'framework',
    scorers: [
      {
        metric: 'exact',
        answer_path: '/text',
        scorer: { kind: 'exact_match', ignore_case: true, trim: false },
      },
    ],
  });
});

it("names a framework's metric from the catalog, with its parameters and people to hold it against", async () => {
  const server = serving({ status: 200, body: CATALOG });
  render(withQueries(<Scorecards />));
  await userEvent.type(await screen.findByLabelText('Card name'), 'framework');
  await userEvent.type(screen.getByLabelText('Scorer 1 metric'), 'relevancy');
  await userEvent.selectOptions(screen.getByLabelText('Scorer 1 kind'), 'external');
  await userEvent.selectOptions(await screen.findByLabelText('Scorer 1 framework'), 'deepeval');
  await userEvent.selectOptions(
    screen.getByLabelText('Scorer 1 framework metric'),
    'answer_relevancy',
  );
  expect(screen.getByText(/graded by gemma-4-e2b @ ud-q4-k-xl/)).toBeTruthy();
  // It reads no expectation, so no path into one is offered.
  expect(screen.queryByLabelText('Scorer 1 expected path')).toBeNull();
  await userEvent.type(screen.getByLabelText('Scorer 1 parameter threshold'), '0.7');
  await userEvent.selectOptions(screen.getByLabelText('Scorer 1 parameter strict_mode'), 'true');
  await userEvent.type(screen.getByLabelText('Scorer 1 input path'), '/question');
  await userEvent.click(screen.getByLabelText('Scorer 1 held against people'));
  await userEvent.selectOptions(
    screen.getByLabelText('Scorer 1 calibration rubric'),
    `helpful@${RUBRIC}`,
  );
  await userEvent.type(screen.getByLabelText('Scorer 1 passes at'), '0.6');
  await userEvent.selectOptions(await screen.findByLabelText('Scorer 1 person passes at'), 'good');
  await userEvent.click(screen.getByRole('button', { name: 'Publish card' }));

  expect(
    await screen.findByText(/Pinned from the catalog: relevancy is deepeval 4\.2\.2/),
  ).toBeTruthy();
  const sent = server.calls.find((call) => call.method === 'POST')?.body as Record<string, any>;
  expect(sent.scorers[0]).toEqual({
    metric: 'relevancy',
    input_path: '/question',
    scorer: {
      kind: 'external',
      adapter: 'deepeval',
      metric: 'answer_relevancy',
      parameters: { threshold: 0.7, strict_mode: true },
      calibration: {
        rubric: { name: 'helpful', version: RUBRIC },
        pass_at: 0.6,
        pass_level: 'good',
      },
    },
  });
  // What the metric is stays the server's word: nothing pinned is sent.
  expect(sent.scorers[0].scorer.declared).toBeUndefined();
});

it('says which variable is missing when no scorer service has described itself', async () => {
  const server = serving({
    status: 404,
    body: { code: 'not_found', message: 'a scorer catalog' },
  });
  render(withQueries(<Scorecards />));
  await userEvent.selectOptions(await screen.findByLabelText('Scorer 1 kind'), 'external');
  expect(await screen.findByText(/AIWATCHER_SCORER_URL/)).toBeTruthy();
  expect(screen.queryByLabelText('Scorer 1 framework')).toBeNull();
  expect(server.countOf('POST', '/evaluation-scorecards')).toBe(0);
});
