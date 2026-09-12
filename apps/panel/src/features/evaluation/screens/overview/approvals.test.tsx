import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it } from 'vitest';
import { Approvals } from './approvals';
import { serve, withQueries } from '@/test/server';

const RECORD = {
  approval_id: 'a'.repeat(64),
  variant_id: 'aa11',
  context_id: 'bb22',
  approved_by: 'operator',
  approved_at: 1789200000,
};

function only(approvals: unknown[], extra: Parameters<typeof serve>[0] = []) {
  serve([
    { method: 'GET', path: '/auth/config', answer: { status: 200, body: { enabled: false } } },
    {
      method: 'GET',
      path: '/evaluation-approvals',
      answer: { status: 200, body: { approvals } },
    },
    { method: 'GET', path: '/bundle', answer: { status: 200, body: [] } },
    ...extra,
  ]);
}

it('says who admitted a pair and when, and keeps a withdrawn one on the list', async () => {
  only([
    { record: RECORD, withdrawn: null },
    {
      record: { ...RECORD, approval_id: 'b'.repeat(64), variant_id: 'cc33' },
      withdrawn: { withdrawn_by: 'operator', withdrawn_at: 1789203600 },
    },
  ]);
  render(withQueries(<Approvals />));
  expect(await screen.findByText(/Admitted by operator on/)).toBeTruthy();
  // An approval that vanished from the list would read as one nobody made.
  expect(screen.getByText(/Withdrawn by operator on .* final for this pair/)).toBeTruthy();
  expect(screen.getByText('withdrawn')).toBeTruthy();
});

it('asks before withdrawing, because withdrawal hides every result of the pair', async () => {
  only([{ record: RECORD, withdrawn: null }]);
  render(withQueries(<Approvals />));
  await userEvent.click(await screen.findByRole('button', { name: 'Withdraw…' }));
  expect(screen.getByText(/Hide every result of this pair\? This cannot be undone\./)).toBeTruthy();
  // And backing out is a button of its own rather than a reload.
  await userEvent.click(screen.getByRole('button', { name: 'Keep' }));
  expect(screen.queryByText(/cannot be undone/)).toBeNull();
});

it('refuses a bundle with no declaration in it, before anything is uploaded', async () => {
  only([]);
  render(withQueries(<Approvals />));
  const input = await screen.findByLabelText('Bundle files');
  await userEvent.upload(input, new File(['x = 1'], 'scorer.py', { type: 'text/x-python' }));
  await userEvent.click(screen.getByRole('button', { name: 'Stage and admit' }));
  expect(await screen.findByText(/needs a manifest.json/)).toBeTruthy();
});

it('reads a store nobody configured as a deployment fact, not an empty list', async () => {
  serve([
    { method: 'GET', path: '/auth/config', answer: { status: 200, body: { enabled: false } } },
    {
      method: 'GET',
      path: '/evaluation-approvals',
      answer: { status: 501, body: { code: 'registry_disabled', message: 'no object store' } },
    },
  ]);
  render(withQueries(<Approvals />));
  expect(await screen.findByText(/keeps no durable evidence/)).toBeTruthy();
});
