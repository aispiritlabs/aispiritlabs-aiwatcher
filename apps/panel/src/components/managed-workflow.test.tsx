import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { refusal, serve, withQueries } from '@/test/server';
import { ManagedExecutionControls, WorkflowLauncher } from './managed-workflow';

afterEach(() => vi.unstubAllGlobals());

const definitions = [
  {
    definition: { name: 'house', version: '1', steps: [] },
    revision: 'a'.repeat(64),
  },
];

describe('starting a registered workflow', () => {
  it('pins the chosen revision, sends parameters and an idempotency key', async () => {
    serve([
      { method: 'GET', path: '/workflow-definitions', answer: { status: 200, body: definitions } },
      {
        method: 'POST',
        path: '/executions',
        answer: {
          status: 202,
          body: {
            execution: { definition_name: 'house', execution_id: 'run-1' },
            created: true,
          },
        },
      },
    ]);
    const fetchMock = vi.fn(fetch);
    vi.stubGlobal('fetch', fetchMock);
    const onStarted = vi.fn();
    render(withQueries(<WorkflowLauncher onStarted={onStarted} />));
    await screen.findByRole('option', { name: 'house · 1' });
    await userEvent.selectOptions(screen.getByRole('combobox'), 'house');
    await userEvent.clear(screen.getByLabelText('Workflow parameters'));
    await userEvent.type(screen.getByLabelText('Workflow parameters'), '{{"house":42}');
    await userEvent.click(screen.getByRole('button', { name: 'Start run' }));
    await waitFor(() => expect(onStarted).toHaveBeenCalledWith('house', 'run-1'));
    const request = fetchMock.mock.calls
      .map(([input]) => input as Request)
      .find((input) => input.method === 'POST');
    expect(request?.headers.get('Idempotency-Key')).toBeTruthy();
    expect(await request?.clone().json()).toEqual({
      target: { kind: 'workflow', name: 'house', revision: 'a'.repeat(64) },
      parameters: { house: 42 },
    });
  });

  it('keeps invalid parameters in the form without sending a launch', async () => {
    const server = serve([
      { method: 'GET', path: '/workflow-definitions', answer: { status: 200, body: definitions } },
    ]);
    render(withQueries(<WorkflowLauncher onStarted={vi.fn()} />));
    await screen.findByRole('option', { name: 'house · 1' });
    await userEvent.selectOptions(screen.getByRole('combobox'), 'house');
    await userEvent.clear(screen.getByLabelText('Workflow parameters'));
    await userEvent.type(screen.getByLabelText('Workflow parameters'), '[[]');
    await userEvent.click(screen.getByRole('button', { name: 'Start run' }));
    await screen.findByRole('alert');
    expect(server.countOf('POST', '/executions')).toBe(0);
  });
});

describe('managed commands', () => {
  it('shows server refusals instead of hiding controls as an external workflow', async () => {
    serve([
      {
        method: 'GET',
        path: '/executions/run-1',
        answer: {
          status: 403,
          body: refusal('forbidden', 'This run needs an editor'),
        },
      },
    ]);
    render(withQueries(<ManagedExecutionControls executionId="run-1" />));
    expect((await screen.findByRole('alert')).textContent).toContain('This run needs an editor');
  });

  it('retries the selected failed step only when the server permits it', async () => {
    const server = serve([
      {
        method: 'GET',
        path: '/executions/run-1',
        answer: {
          status: 200,
          body: {
            execution: { state: { state_type: 'failed' } },
            allowed: [],
          },
        },
      },
      {
        method: 'GET',
        path: '/steps/persist/context',
        answer: { status: 200, body: { allowed: ['retry'] } },
      },
      { method: 'POST', path: '/steps/persist/commands/retry', answer: { status: 200, body: {} } },
    ]);
    render(withQueries(<ManagedExecutionControls executionId="run-1" node="persist" />));
    await userEvent.click(await screen.findByRole('button', { name: 'Retry persist' }));
    await waitFor(() => expect(server.countOf('POST', '/steps/persist/commands/retry')).toBe(1));
    expect(screen.queryByRole('button', { name: 'pause' })).toBeNull();
  });
});
