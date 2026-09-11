import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { refusal, serve, withQueries } from '@/test/server';
import { ManagedExecutionControls, WorkflowLauncher } from '@/features/workflows/components/managed-workflow';

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

  it('answers a gate the graph is parked on, with the answers that gate declared', async () => {
    // A workflow gate stops the graph and the answer is the only thing that
    // moves it on, so this view has to be able to give one — before this, a run
    // that parked here had every control except the one that mattered.
    const server = serve([
      {
        method: 'GET',
        path: '/executions/run-1',
        answer: {
          status: 200,
          body: {
            execution: { state: { state_type: 'awaiting_input' } },
            allowed: ['cancel'],
          },
        },
      },
      {
        method: 'GET',
        path: '/steps/sign-off/context',
        answer: {
          status: 200,
          body: {
            allowed: ['answer'],
            runtime: { runtime: 'human_input', prompt: 'Import these houses?', role: 'editor' },
            state: {
              current_attempt: 1,
              awaiting: {
                prompt: 'Import these houses?',
                role: 'editor',
                choices: ['approve', 'reject'],
              },
            },
          },
        },
      },
      { method: 'POST', path: '/steps/sign-off/input', answer: { status: 200, body: {} } },
    ]);

    render(withQueries(<ManagedExecutionControls executionId="run-1" node="sign-off" />));

    await screen.findByText('Import these houses?');
    const answers = within(screen.getByRole('group', { name: 'Answers' })).getAllByRole('button');
    expect(answers.map((button) => button.textContent)).toEqual(['approve', 'reject']);

    await userEvent.click(screen.getByRole('button', { name: 'approve' }));

    await waitFor(() => expect(server.countOf('POST', '/steps/sign-off/input')).toBe(1));
    // A box somebody drew. Answering it is the process working, which is very
    // different news from the next case.
    expect(screen.getByText('This gate is part of the definition.')).toBeTruthy();
  });

  it('says when the running code asked rather than the plan', async () => {
    // The two render identically otherwise, and they are not the same news. An
    // authored gate is a routine sign-off; a `python_task` that is waiting
    // stopped in the middle of its own work because it hit something nobody
    // planned — which is exactly when somebody should read the question twice
    // before pressing approve.
    serve([
      {
        method: 'GET',
        path: '/executions/run-1',
        answer: {
          status: 200,
          body: {
            execution: { state: { state_type: 'awaiting_input' } },
            allowed: ['cancel'],
          },
        },
      },
      {
        method: 'GET',
        path: '/steps/stage/context',
        answer: {
          status: 200,
          body: {
            allowed: ['answer'],
            runtime: { runtime: 'python_task', task_ref: 'agent@1', queue: 'default' },
            state: {
              current_attempt: 2,
              awaiting: {
                prompt: 'May I send this email to jan@example.com?',
                role: 'editor',
                choices: ['approve', 'reject'],
              },
            },
          },
        },
      },
    ]);

    render(withQueries(<ManagedExecutionControls executionId="run-1" node="stage" />));

    await screen.findByText('May I send this email to jan@example.com?');
    expect(
      screen.getByText('This step stopped in the middle of its own work to ask.'),
    ).toBeTruthy();
  });

  it('says nothing about who asked when the binding is not there', async () => {
    // A wrong sentence is worse than none. An older build that sends no
    // binding, or a context still being read, must not have "the code decided
    // to ask" put under a gate somebody authored.
    serve([
      {
        method: 'GET',
        path: '/executions/run-1',
        answer: {
          status: 200,
          body: {
            execution: { state: { state_type: 'awaiting_input' } },
            allowed: ['cancel'],
          },
        },
      },
      {
        method: 'GET',
        path: '/steps/sign-off/context',
        answer: {
          status: 200,
          body: {
            allowed: ['answer'],
            state: {
              current_attempt: 1,
              awaiting: { prompt: 'Publish?', role: 'editor', choices: ['approve'] },
            },
          },
        },
      },
    ]);

    render(withQueries(<ManagedExecutionControls executionId="run-1" node="sign-off" />));

    await screen.findByText('Publish?');
    expect(screen.queryByText(/stopped in the middle/)).toBeNull();
    expect(screen.queryByText(/part of the definition/)).toBeNull();
  });
});
