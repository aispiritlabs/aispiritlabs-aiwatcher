import { render, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { DurableEvaluation, EvidenceState } from '@/api/generated/types.gen';
import { Evidence, EvidenceRow, EvidenceUnavailable, Retention } from './evidence';
import { ApiFailure } from '@/shared/lib/result';
import { serve, withQueries } from '@/test/server';

afterEach(() => vi.unstubAllGlobals());

const RECEIPT = {
  evaluation_id: 'kept-1',
  version: 'ff00',
  variant_id: 'aa11',
  context_id: 'bb22',
  // 2026-09-12, and thirty days after it.
  committed_at: 1789200000,
  expires_at: 1791792000,
};

function evidence(state: EvidenceState, extra: Partial<DurableEvaluation> = {}): DurableEvaluation {
  return {
    receipt: RECEIPT,
    state,
    metrics: { accuracy: 1 },
    counts: { selected: 3, scored: 3, failed: 0, unscored: 0 },
    manifest: null,
    status: 'succeeded',
    ...extra,
  };
}

function only(routes: Parameters<typeof serve>[0] = []) {
  serve([
    { method: 'GET', path: '/auth/config', answer: { status: 200, body: { enabled: false } } },
    ...routes,
  ]);
}

const STATES: [EvidenceState, RegExp][] = [
  ['complete', /Every selected case was scored/],
  ['partial', /does not cover every selected case/],
  ['missing_artifact', /no longer in the store/],
  ['corrupt_artifact', /does not match the digest/],
  ['expired', /deleted when its deadline passed/],
  ['deleted_source', /deleted or revoked at its owner/],
  ['forbidden', /Three different things say this/],
];

it.each(STATES)(
  'says what %s means rather than drawing it as a failed read',
  async (state, says) => {
    only([
      {
        method: 'GET',
        path: '/cases',
        answer: { status: 200, body: { version: 'ff00', cases: [], state } },
      },
    ]);
    render(withQueries(<Evidence evidence={evidence(state)} />));
    if (state === 'complete') {
      // The one state with nothing to act on. Its sentence is the badge.
      expect(await screen.findByText('Kept')).toBeTruthy();
      expect(screen.queryByText(says)).toBeNull();
      return;
    }
    expect(await screen.findByText(says)).toBeTruthy();
  },
);

it('never prints "partial" twice for two different questions', async () => {
  only();
  render(
    withQueries(
      <Evidence
        evidence={evidence('partial', {
          status: 'partial',
          counts: { selected: 3, scored: 1, failed: 1, unscored: 1 },
        })}
      />,
    ),
  );
  // `EvidenceState::Partial` is about what is readable; `ResultStatus::Partial`
  // about what was measured. Two badges reading "partial" is the thing to avoid.
  expect((await screen.findAllByText('Kept with gaps')).length).toBeGreaterThan(0);
  expect(screen.getByText('Partly measured')).toBeTruthy();
  expect(screen.queryAllByText(/^partial$/i)).toHaveLength(0);
});

it('puts the retention deadline beside the result, as a date', async () => {
  only();
  render(withQueries(<Evidence evidence={evidence('complete')} />));
  const deadline = await screen.findByText('Kept until');
  const stat = deadline.closest('div');
  expect(stat).toBeTruthy();
  expect(within(stat as HTMLElement).getByText(/2026/)).toBeTruthy();
});

it('names the three causes of forbidden and does not guess between them', async () => {
  // Authentication off, so there is nobody to refuse: the role cause is ruled
  // out as a fact rather than left as one of three maybes.
  only();
  render(withQueries(<Evidence evidence={evidence('forbidden')} />));
  expect(await screen.findByText(/you hold admin/)).toBeTruthy();
  expect(screen.getByText(/was withdrawn/)).toBeTruthy();
  expect(screen.getByText(/no longer establish access/)).toBeTruthy();
});

it('states the role requirement the way Conversations does, rather than as a failure', async () => {
  serve([
    { method: 'GET', path: '/auth/config', answer: { status: 200, body: { enabled: true } } },
    {
      method: 'GET',
      path: '/auth/me',
      answer: { status: 200, body: { subject: 'v', roles: ['viewer'] } },
    },
  ]);
  render(withQueries(<Evidence evidence={evidence('forbidden')} />));
  expect(await screen.findByText(/needs the admin role/)).toBeTruthy();
  expect(screen.queryByText(/you hold admin/)).toBeNull();
});

it('reads a store nobody configured as a 501 naming the variable, not an empty catalogue', () => {
  render(
    <EvidenceUnavailable
      failure={
        new ApiFailure(
          501,
          { code: 'registry_disabled', message: 'durable evaluations require an object store.' },
          'unused',
        )
      }
    />,
  );
  expect(screen.getByText(/AIWATCHER_EVALUATION_SOURCE_DIR/)).toBeTruthy();
  expect(screen.getByText(/keeps no durable evidence/)).toBeTruthy();
});

it('says on the row which of the two kinds it is', () => {
  render(<EvidenceRow evidence={evidence('complete')} selected={false} onSelect={() => {}} />);
  // The folded half of this list goes when the event log's retention takes it,
  // and this half does not. Finding that out by clicking is the thing to avoid.
  expect(screen.getByText('kept')).toBeTruthy();
  expect(screen.getByText('3/3 scored')).toBeTruthy();
});

it('marks a kept result whose bytes the last pass could not find, without a second state', () => {
  render(<EvidenceRow evidence={evidence('complete')} gaps selected={false} onSelect={() => {}} />);
  // The header verifies, so the state badge is right to read Kept. What the
  // catalogue cannot see is what the collection pass found behind it.
  expect(screen.getByText('Kept')).toBeTruthy();
  expect(screen.getByText('bytes missing')).toBeTruthy();
});

it('explains in the detail why a kept result is marked as missing bytes', async () => {
  only();
  render(
    withQueries(
      <Evidence
        evidence={evidence('complete')}
        gaps={{
          ran_at: 1789200000,
          retired: 0,
          collected: 0,
          failures: 0,
          collected_at: 1789200000,
          damaged: ['kept-1'],
          damaged_count: 1,
        }}
      />,
    ),
  );
  expect(await screen.findByText(/Objects this result points at are missing/)).toBeTruthy();
  expect(screen.getByText(/collection pass of/)).toBeTruthy();
});

it('says how many kept results are missing bytes, beside how the pass itself went', () => {
  render(
    <Retention
      report={{
        ran_at: 1789200000,
        retired: 0,
        collected: 0,
        failures: 0,
        collected_at: 1789200000,
        damaged: ['kept-1', 'kept-2'],
        damaged_count: 2,
      }}
    />,
  );
  expect(screen.getByText(/2 kept results are missing bytes/)).toBeTruthy();
});
