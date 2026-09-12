import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';

import { refusal, serve, withQueries } from '@/test/server';

import { Measure } from './measure';

afterEach(() => vi.unstubAllGlobals());

const VERSION = 'c'.repeat(64);
const DECLARATION = 'd'.repeat(64);
const APPROVAL = 'a'.repeat(64);

function artifact(name: string) {
  return {
    name,
    uri: `file://${name}`,
    digest: 'e'.repeat(64),
    size_bytes: 10,
    content_type: 'application/json',
  };
}

function published(kind = 'curation') {
  return {
    receipt: {
      evaluation_id: 'baseline-1',
      version: 'v1',
      variant_id: 'aa11',
      context_id: 'bb22',
      committed_at: 1789200000,
      expires_at: 1791792000,
    },
    state: 'complete',
    status: 'succeeded',
    counts: { selected: 3, scored: 3, failed: 0, unscored: 0 },
    metrics: { exact: 1 },
    reproducible: true,
    manifest: {
      schema_version: 1,
      origin: { evaluation_id: 'baseline-1', repetition_id: 'measurement-1' },
      variant: {
        experiment_id: 'baseline',
        dataset: { kind, name: 'questions', version: 'v1' },
        code: artifact('responses.py'),
        generation_config: artifact('generation.json'),
      },
      context: {
        dataset: { kind, name: 'questions', version: 'v1' },
        case_manifest: artifact('cases.json'),
        case_count: 3,
        split: 'test',
        suite: { name: 'answer-quality', version: VERSION },
        scorer: { name: 'aiwatcher.scoring', version: '1' },
        input_schema: artifact('input-schema.json'),
        expectations_schema: artifact('expectations-schema.json'),
        metrics: [{ name: 'exact', unit: 'ratio', direction: 'higher', aggregation: 'rate' }],
      },
    },
  };
}

function card(scorer: unknown, shown?: string) {
  return {
    version: VERSION,
    published_by: 'ada',
    published_at: 1,
    scorecard: {
      name: 'answer-quality',
      scorers: [{ metric: 'exact', scorer, ...(shown === undefined ? {} : { input_path: shown }) }],
    },
  };
}

/** A file jsdom can read the way a browser does. */
function recordingFile(): File {
  const text = '{"answers": []}';
  return Object.assign(new File([text], 'answers.json', { type: 'application/json' }), {
    arrayBuffer: async () => new TextEncoder().encode(text).buffer,
  });
}

function drafting(
  scorer: unknown,
  kind = 'curation',
  extra: Parameters<typeof serve>[0] = [],
  shown?: string,
) {
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
    {
      method: 'GET',
      path: '/evaluation-scorecards/answer-quality',
      answer: { status: 200, body: card(scorer, shown) },
    },
    {
      method: 'GET',
      path: '/evaluation-results',
      answer: { status: 200, body: { evaluations: [published(kind)], next_cursor: null } },
    },
    ...extra,
  ]);
}

async function choose() {
  await userEvent.selectOptions(
    await screen.findByLabelText('Scorecard'),
    await screen.findByRole('option', { name: /answer-quality/ }),
  );
  const like = screen.getByLabelText('Measure like');
  await userEvent.selectOptions(
    like,
    await within(like).findByRole('option', { name: /baseline-1/ }),
  );
  await waitFor(() =>
    expect((screen.getByLabelText('Experiment') as HTMLInputElement).value).toBe('baseline'),
  );
}

it('declares a run like a published one, renamed, over the recording it staged', async () => {
  const onDeclared = vi.fn();
  const server = drafting({ kind: 'exact_match' }, 'curation', [
    {
      method: 'PUT',
      path: '/evaluation-recordings/answers.json',
      answer: { status: 200, body: artifact('answers.json') },
    },
    {
      method: 'POST',
      path: '/evaluation-runs',
      answer: { status: 200, body: { declaration: { id: DECLARATION } } },
    },
  ]);
  render(
    withQueries(
      <Measure
        declaration={undefined}
        measured={undefined}
        onDeclared={onDeclared}
        onStarted={vi.fn()}
        onOpenResult={vi.fn()}
      />,
    ),
  );
  await choose();
  expect(screen.getByText(/measures exact \(ratio, higher\)/)).toBeTruthy();
  const experiment = screen.getByLabelText('Experiment');
  await userEvent.clear(experiment);
  await userEvent.type(experiment, 'candidate');
  await userEvent.type(screen.getByLabelText('Evaluation ID'), 'candidate-1');
  await userEvent.upload(screen.getByLabelText('Recording'), recordingFile());
  await userEvent.click(screen.getByRole('button', { name: 'Declare' }));

  await waitFor(() => expect(onDeclared).toHaveBeenCalledWith(DECLARATION));
  const declared = server.calls.find(
    (call) => call.method === 'POST' && call.url.endsWith('/evaluation-runs'),
  )?.body as Record<string, any>;
  expect(declared.variant.experiment_id).toBe('candidate');
  expect(declared.cohort.case_count).toBe(3);
  expect(declared.answers.name).toBe('answers.json');
  // Nothing the server derives is sent: no metrics, and no judge the card did not ask for.
  expect(declared.metrics).toBeUndefined();
  expect(declared.judge).toBeUndefined();
});

it('answers a conversation cohort from the archive, and offers nothing else', async () => {
  const server = drafting({ kind: 'forbidden', text: 'pesel' }, 'conversations', [
    {
      method: 'POST',
      path: '/evaluation-runs',
      answer: { status: 200, body: { declaration: { id: DECLARATION } } },
    },
  ]);
  render(
    withQueries(
      <Measure
        declaration={undefined}
        measured={undefined}
        onDeclared={vi.fn()}
        onStarted={vi.fn()}
        onOpenResult={vi.fn()}
      />,
    ),
  );
  await choose();
  expect((screen.getByLabelText('Recording') as HTMLInputElement).disabled).toBe(true);
  expect(screen.getByText(/the only source for a conversation cohort/)).toBeTruthy();
  await userEvent.type(screen.getByLabelText('Evaluation ID'), 'archive-1');
  await userEvent.click(screen.getByRole('button', { name: 'Declare' }));
  await waitFor(() => expect(server.countOf('POST', '/evaluation-runs')).toBe(1));
  const declared = server.calls.find((call) => call.url.endsWith('/evaluation-runs'))
    ?.body as Record<string, unknown>;
  expect(declared.answers).toBe('archive');
  expect(server.countOf('PUT', '/evaluation-recordings/answers.json')).toBe(0);
});

it('asks for a calibration set before declaring a run whose card asks a judge', async () => {
  const rubric = { name: 'helpful', version: 'r'.repeat(64) };
  const server = drafting({ kind: 'judge', rubric }, 'curation', [
    {
      method: 'POST',
      path: '/evaluation-calibrations',
      answer: {
        status: 200,
        body: {
          version: 'f'.repeat(64),
          taken_by: 'ada',
          taken_at: 1,
          calibration: {
            name: 'people-on-baseline-1',
            result: { name: 'baseline-1', version: 'v1' },
            items: [{}, {}],
          },
        },
      },
    },
  ]);
  render(
    withQueries(
      <Measure
        declaration={undefined}
        measured={undefined}
        onDeclared={vi.fn()}
        onStarted={vi.fn()}
        onOpenResult={vi.fn()}
      />,
    ),
  );
  await choose();
  expect(await screen.findByText(/This card asks a model about helpful/)).toBeTruthy();
  expect(screen.getByText(/never what the case asked/)).toBeTruthy();
  await userEvent.type(screen.getByLabelText('Evaluation ID'), 'judged-1');
  await userEvent.upload(screen.getByLabelText('Recording'), recordingFile());
  await userEvent.click(screen.getByRole('button', { name: 'Declare' }));
  expect(await screen.findByText(/take its calibration set first/)).toBeTruthy();
  expect(server.countOf('POST', '/evaluation-runs')).toBe(0);

  await userEvent.selectOptions(
    screen.getByLabelText('Calibration result'),
    screen.getAllByRole('option', { name: 'baseline-1' }).at(-1)!,
  );
  await userEvent.click(screen.getByRole('button', { name: 'Take calibration set' }));
  expect(await screen.findByText(/2 human judgements of baseline-1/)).toBeTruthy();
  const taken = server.calls.find((call) => call.url.endsWith('/evaluation-calibrations'))
    ?.body as Record<string, unknown>;
  expect(taken.rubrics).toEqual([rubric]);
});

it('says who has to admit a declared pair, and follows the run once it starts', async () => {
  const onStarted = vi.fn();
  const view = (admitted: boolean) => ({
    admitted,
    approval_id: APPROVAL,
    manifest: published().manifest,
    declaration: {
      id: DECLARATION,
      declared_by: 'ada',
      declared_at: 1,
      run: {
        evaluation_id: 'candidate-1',
        repetition_id: 'measurement-1',
        variant: published().manifest.variant,
        cohort: {},
        scorecard: { name: 'answer-quality', version: VERSION },
        answers: artifact('answers.json'),
      },
    },
  });
  serve([
    { method: 'GET', path: '/auth/config', answer: { status: 200, body: { enabled: false } } },
    {
      method: 'GET',
      path: `/evaluation-runs/${DECLARATION}`,
      answer: { status: 200, body: view(false) },
    },
    {
      method: 'POST',
      path: `/evaluation-runs/${DECLARATION}/start`,
      answer: (call) =>
        call === 1
          ? {
              status: 409,
              body: refusal(
                'pair_not_admitted',
                `no operator has admitted this pair yet: approval ${APPROVAL}`,
              ),
            }
          : {
              status: 202,
              body: { declaration: DECLARATION, created: true, execution: { execution_id: 'e-1' } },
            },
    },
  ]);
  render(
    withQueries(
      <Measure
        declaration={DECLARATION}
        measured={undefined}
        onDeclared={vi.fn()}
        onStarted={onStarted}
        onOpenResult={vi.fn()}
      />,
    ),
  );
  expect(await screen.findByText('not admitted')).toBeTruthy();
  expect(screen.getByLabelText('Pinned files')).toBeTruthy();
  await userEvent.click(screen.getByRole('button', { name: 'Start' }));
  expect(await screen.findByText(/Nobody has admitted this pair yet/)).toBeTruthy();
  expect(onStarted).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: 'Start' }));
  await waitFor(() => expect(onStarted).toHaveBeenCalledWith('e-1'));
});

it("says what of a case's question a judge is shown, as the card points", async () => {
  drafting(
    { kind: 'judge', rubric: { name: 'helpful', version: 'r'.repeat(64) } },
    'curation',
    [],
    '/question',
  );
  render(
    withQueries(
      <Measure
        declaration={undefined}
        measured={undefined}
        onDeclared={vi.fn()}
        onStarted={vi.fn()}
        onOpenResult={vi.fn()}
      />,
    ),
  );
  await choose();
  expect(await screen.findByText(/exact sees the case's input at \/question\./)).toBeTruthy();
});
