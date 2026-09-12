import * as React from 'react';
import { useQuery } from '@tanstack/react-query';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';

import { refusal, serve, withQueries } from '@/test/server';

import { Measure } from './measure';

// The run card links to the waterfall, and a `Link` outside a router throws.
vi.mock('@tanstack/react-router', () => ({
  Link: ({ children }: { children: React.ReactNode }) => <a href="#waterfall">{children}</a>,
}));

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
  await userEvent.type(screen.getByLabelText('Timeout in minutes'), '10');
  // A card that asks nothing has nothing to put at once.
  expect(screen.queryByLabelText('Judge concurrency')).toBeNull();
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
  expect(declared.settings).toEqual({ timeout_seconds: 600 });
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

it('follows the run it started, and re-reads the catalogue as the run moves', async () => {
  const onStarted = vi.fn();
  let read = 0;
  /** Stands in for the catalogue on the page beside the form. */
  function Catalogue() {
    useQuery({
      queryKey: ['evaluation-evidence', 'probe'],
      queryFn: async () => {
        read += 1;
        return read;
      },
    });
    return null;
  }
  serve([
    { method: 'GET', path: '/auth/config', answer: { status: 200, body: { enabled: false } } },
    {
      method: 'GET',
      path: `/evaluation-runs/${DECLARATION}`,
      answer: {
        status: 200,
        body: {
          admitted: true,
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
        },
      },
    },
    {
      method: 'GET',
      path: '/api/v1/executions/e-1',
      answer: {
        status: 200,
        body: {
          allowed: [],
          execution: {
            created_at: '2026-09-12T09:00:00Z',
            definition_name: 'candidate-1',
            execution_id: 'e-1',
            last_message_version: 4,
            mode: 'compiled',
            owner: 'Local',
            plan_id: 'plan-1',
            requested_by: 'ada',
            state: { state_type: 'completed' },
            steps: [
              {
                step_id: 'score',
                runtime: 'judge_evaluation',
                current_attempt: 2,
                state: { state_type: 'completed' },
              },
            ],
          },
        },
      },
    },
  ]);
  render(
    withQueries(
      <>
        <Catalogue />
        <Measure
          declaration={DECLARATION}
          measured="e-1"
          onDeclared={vi.fn()}
          onStarted={onStarted}
          onOpenResult={vi.fn()}
        />
      </>,
    ),
  );
  expect(await screen.findByText('judge_evaluation')).toBeTruthy();
  expect(screen.getByText('attempt 2')).toBeTruthy();
  await waitFor(() => expect(read).toBeGreaterThan(1));
});

it("holds admitting and starting until the server's warning about the archive is acknowledged", async () => {
  const onStarted = vi.fn();
  const warning =
    "This run's judge (llamacpp · gemma) is sent words from the conversation archive: each assistant response this run scores.";
  const manifest = published('conversations').manifest;
  const judged = {
    ...manifest,
    context: {
      ...manifest.context,
      judge: {
        provider: 'llamacpp',
        model: { name: 'gemma', version: 'q4' },
        configuration: artifact('judge-settings.json'),
        calibration_dataset: { kind: 'assessments', name: 'people', version: 'f'.repeat(64) },
        reads_archive: true,
      },
    },
  };
  const server = serve([
    { method: 'GET', path: '/auth/config', answer: { status: 200, body: { enabled: false } } },
    {
      method: 'GET',
      path: `/evaluation-runs/${DECLARATION}`,
      answer: {
        status: 200,
        body: {
          admitted: false,
          approval_id: APPROVAL,
          manifest: judged,
          warnings: [warning],
          declaration: {
            id: DECLARATION,
            declared_by: 'ada',
            declared_at: 1,
            run: {
              evaluation_id: 'archive-judged',
              repetition_id: 'measurement-1',
              variant: judged.variant,
              cohort: {},
              scorecard: { name: 'answer-quality', version: VERSION },
              answers: 'archive',
              judge: {
                provider: 'llamacpp',
                model: { name: 'gemma', version: 'q4' },
                calibration: { name: 'people', version: 'f'.repeat(64) },
              },
            },
          },
        },
      },
    },
    {
      method: 'POST',
      path: `/evaluation-runs/${DECLARATION}/start`,
      answer: {
        status: 202,
        body: { declaration: DECLARATION, created: true, execution: { execution_id: 'e-2' } },
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
  expect(await screen.findByText(warning)).toBeTruthy();
  expect(screen.getByText('judge reads the archive')).toBeTruthy();
  const start = screen.getByRole('button', { name: 'Start' }) as HTMLButtonElement;
  const admit = screen.getByRole('button', { name: 'Stage and admit' }) as HTMLButtonElement;
  expect(start.disabled).toBe(true);
  expect(admit.disabled).toBe(true);
  expect(screen.getByText('Acknowledge the warnings first.')).toBeTruthy();

  await userEvent.click(screen.getByLabelText('Acknowledge the warnings'));
  expect(start.disabled).toBe(false);
  await userEvent.click(start);
  await waitFor(() => expect(onStarted).toHaveBeenCalledWith('e-2'));
  expect(server.countOf('POST', `/evaluation-runs/${DECLARATION}/start`)).toBe(1);
});

it("takes a cohort from a curation version's first cases, and pins the variant to that dataset", async () => {
  const derivedCohort = {
    case_manifest: { ...artifact('cases.json'), digest: 'f'.repeat(64) },
    case_count: 2,
    split: 'held-out',
    input_schema: artifact('input-schema.json'),
    expectations_schema: artifact('expectations-schema.json'),
  };
  const server = drafting({ kind: 'exact_match' }, 'curation', [
    {
      method: 'GET',
      path: '/datasets',
      answer: {
        status: 200,
        body: {
          datasets: [
            {
              name: 'questions-v2',
              latest: { version: 'b'.repeat(64), row_count: 40, columns: [], created_at: 'x' },
              versions: [{ version: 'b'.repeat(64), row_count: 40, columns: [], created_at: 'x' }],
            },
          ],
        },
      },
    },
    {
      method: 'POST',
      path: '/evaluation-cohorts',
      answer: {
        status: 200,
        body: {
          request: {},
          cohort: derivedCohort,
          available: 40,
          derived_by: 'ada',
          derived_at: 1,
        },
      },
    },
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
        onDeclared={vi.fn()}
        onStarted={vi.fn()}
        onOpenResult={vi.fn()}
      />,
    ),
  );
  await choose();
  await userEvent.click(screen.getByLabelText('A dataset version this deployment owns'));
  const dataset = screen.getByLabelText('Dataset');
  await userEvent.selectOptions(
    dataset,
    await within(dataset).findByRole('option', { name: 'questions-v2' }),
  );
  await userEvent.selectOptions(
    screen.getByLabelText('Dataset version'),
    screen.getByRole('option', { name: /40 rows/ }),
  );
  const split = screen.getByLabelText('Split');
  await userEvent.clear(split);
  await userEvent.type(split, 'held-out');
  await userEvent.type(screen.getByLabelText('Case limit'), '2');
  await userEvent.type(screen.getByLabelText('Evaluation ID'), 'candidate-2');
  await userEvent.upload(screen.getByLabelText('Recording'), recordingFile());
  await userEvent.click(screen.getByRole('button', { name: 'Declare' }));

  await waitFor(() =>
    expect(server.calls.some((call) => call.url.endsWith('/evaluation-runs'))).toBe(true),
  );
  const asked = server.calls.find((call) => call.url.endsWith('/evaluation-cohorts'))
    ?.body as Record<string, any>;
  expect(asked).toEqual({
    dataset: { kind: 'curation', name: 'questions-v2', version: 'b'.repeat(64) },
    split: 'held-out',
    limit: 2,
  });
  const declared = server.calls.find(
    (call) => call.method === 'POST' && call.url.endsWith('/evaluation-runs'),
  )?.body as Record<string, any>;
  // The pins are the server's, never worked out here.
  expect(declared.cohort).toEqual(derivedCohort);
  expect(declared.variant.dataset).toEqual(asked.dataset);
});

it("a limit on the result's own cohort is taken from that result's dataset and split", async () => {
  const server = drafting({ kind: 'exact_match' }, 'curation', [
    {
      method: 'POST',
      path: '/evaluation-cohorts',
      answer: {
        status: 200,
        body: {
          request: {},
          cohort: { ...published().manifest.context, case_count: 1 },
          available: 3,
          derived_by: 'ada',
          derived_at: 1,
        },
      },
    },
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
        onDeclared={vi.fn()}
        onStarted={vi.fn()}
        onOpenResult={vi.fn()}
      />,
    ),
  );
  await choose();
  await userEvent.type(screen.getByLabelText('Case limit'), '1');
  await userEvent.type(screen.getByLabelText('Evaluation ID'), 'smoke');
  await userEvent.upload(screen.getByLabelText('Recording'), recordingFile());
  await userEvent.click(screen.getByRole('button', { name: 'Declare' }));
  await waitFor(() =>
    expect(server.calls.some((call) => call.url.endsWith('/evaluation-cohorts'))).toBe(true),
  );
  expect(server.calls.find((call) => call.url.endsWith('/evaluation-cohorts'))?.body).toEqual({
    dataset: { kind: 'curation', name: 'questions', version: 'v1' },
    split: 'test',
    limit: 1,
  });
});

it('says a derived cohort is the first cases of how many, and that its files are not brought', async () => {
  serve([
    { method: 'GET', path: '/auth/config', answer: { status: 200, body: { enabled: false } } },
    {
      method: 'GET',
      path: `/evaluation-runs/${DECLARATION}`,
      answer: {
        status: 200,
        body: {
          admitted: false,
          approval_id: APPROVAL,
          manifest: {
            ...published().manifest,
            context: { ...published().manifest.context, case_count: 2 },
          },
          cohort: {
            request: {
              dataset: { kind: 'curation', name: 'questions', version: 'v1' },
              split: 'test',
              limit: 2,
            },
            cohort: {},
            available: 3,
            derived_by: 'ada',
            derived_at: 1,
          },
          declaration: {
            id: DECLARATION,
            declared_by: 'ada',
            declared_at: 1,
            run: {
              evaluation_id: 'smoke',
              repetition_id: 'measurement-1',
              variant: published().manifest.variant,
              cohort: {},
              scorecard: { name: 'answer-quality', version: VERSION },
              answers: artifact('answers.json'),
            },
          },
        },
      },
    },
  ]);
  render(
    withQueries(
      <Measure
        declaration={DECLARATION}
        measured={undefined}
        onDeclared={vi.fn()}
        onStarted={vi.fn()}
        onOpenResult={vi.fn()}
      />,
    ),
  );
  expect(await screen.findByText(/the first 2 of 3 cases/)).toBeTruthy();
  expect(screen.getByText(/derived again from questions when the pair is admitted/)).toBeTruthy();
  expect(screen.getByText(/responses\.py, generation\.json/)).toBeTruthy();
});
