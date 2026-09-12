import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';

import type { Assessment, RubricHead, RubricVersion } from '@/api/generated/types.gen';
import { serve, withQueries } from '@/test/server';

import { CaseJudgement } from './judgement';

afterEach(() => vi.unstubAllGlobals());

const HEAD: RubricHead = {
  name: 'helpfulness',
  version: 'sha-1',
  question: 'did the answer help?',
  updated_at: 1789200000,
};

const FORM: RubricVersion = {
  version: 'sha-1',
  rubric: {
    name: 'helpfulness',
    question: 'did the answer help?',
    guidance: '',
    scale: { kind: 'ordinal', levels: ['bad', 'fine', 'good'] },
    direction: 'higher',
  },
  published_by: 'ada',
  published_at: 1789200000,
};

function said(source: 'human' | 'judge', author: string, value: string): Assessment {
  return {
    standing_id: `standing-${author}`,
    target_id: 'target-1',
    target: {
      kind: 'case',
      evaluation_id: 'after',
      case_id: 'two-plus-two',
      repetition_id: 'measurement-1',
    },
    rubric: 'helpfulness',
    rubric_version: 'sha-1',
    value: { type: 'level', value },
    source,
    author,
    rationale: '',
    revision: 1,
    recorded_at: 1789200000,
    recorded_by: author,
  };
}

function only(assessments: Assessment[], rubrics: RubricHead[] = [HEAD], auth?: unknown) {
  return serve([
    {
      method: 'GET',
      path: '/auth/config',
      answer: { status: 200, body: { enabled: auth !== undefined } },
    },
    ...(auth ? [{ method: 'GET' as const, path: '/auth/me', answer: { status: 200, body: auth } }] : []),
    {
      method: 'GET',
      path: '/evaluation-assessments',
      answer: { status: 200, body: { target_id: 'target-1', assessments } },
    },
    { method: 'GET', path: '/evaluation-rubrics', answer: { status: 200, body: { rubrics } } },
    { method: 'GET', path: '/evaluation-rubrics/helpfulness', answer: { status: 200, body: FORM } },
    {
      method: 'POST',
      path: '/evaluation-assessments',
      answer: { status: 200, body: said('human', 'ada', 'fine') },
    },
  ]);
}

function open() {
  render(
    withQueries(
      <CaseJudgement evaluationId="after" caseId="two-plus-two" repetitionId="measurement-1" />,
    ),
  );
}

it('shows what each author said and rules on none of it', async () => {
  // Whether "good" is good news is the rubric's `direction` to declare, and a
  // browser turning two answers into one verdict would be deciding exactly
  // what the declaration exists to state — so both stand, side by side.
  only([said('human', 'ada', 'bad'), said('judge', 'gpt-4o', 'good')]);
  open();

  expect(await screen.findByText('bad')).toBeTruthy();
  expect(screen.getByText('good')).toBeTruthy();
  expect(screen.getByText('human')).toBeTruthy();
  expect(screen.getByText('judge')).toBeTruthy();
});

it('offers exactly the answers the scale admits, and records the version it showed', async () => {
  const server = only([]);
  open();

  // Three levels, because the rubric declares three. Not a free text field,
  // and not a number: the scale is what an answer has to fit.
  const good = await screen.findByRole('button', { name: 'good' });
  expect(screen.getByRole('button', { name: 'bad' })).toBeTruthy();
  expect(screen.getByRole('button', { name: 'fine' })).toBeTruthy();

  fireEvent.click(good);
  fireEvent.click(screen.getByRole('button', { name: 'Record' }));
  await screen.findByRole('button', { name: 'Record' });

  const written = server.calls.find(
    (call) => call.method === 'POST' && call.url.endsWith('/evaluation-assessments'),
  );
  expect(written?.body).toMatchObject({
    rubric: 'helpfulness',
    rubric_version: 'sha-1',
    value: { type: 'level', value: 'good' },
    target: { kind: 'case', evaluation_id: 'after', repetition_id: 'measurement-1' },
  });
  // And no author: a person's judgement is the session's, and one that named
  // somebody else would be refused anyway.
  expect((written?.body as { author?: string }).author).toBeUndefined();
});

it('says a judgement needs a form rather than offering one nobody declared', async () => {
  only([], []);
  open();

  expect(await screen.findByText(/No rubric declared/)).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Record' })).toBeNull();
});

it('tells a reader which role records a judgement instead of a button that fails', async () => {
  only([], [HEAD], { subject: 'v', roles: ['viewer'] });
  open();

  // Waited for rather than read at once: until the session is read back the
  // answer is "nobody has said no yet", which is not a refusal to render.
  const record = await screen.findByTitle(/needs the editor role/);
  expect((record as HTMLButtonElement).disabled).toBe(true);
});
