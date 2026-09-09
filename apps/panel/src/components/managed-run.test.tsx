import * as React from 'react';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { RunView } from '@/api/generated/types.gen';

import { refusal, serve, withQueries } from '@/test/server';

import { ManagedRunCard, useManagedRun } from './managed-run';

// The card links to the waterfall, and a `Link` outside a router throws. What
// it links to is a routing concern; what it says about a refusal is this file's.
vi.mock('@tanstack/react-router', () => ({
  Link: ({ children }: { children: React.ReactNode }) => <a href="#waterfall">{children}</a>,
}));

afterEach(() => vi.unstubAllGlobals());

const EXECUTION = '/api/v1/executions/e-1';

const RUNNING: RunView = {
  allowed: ['pause', 'cancel'],
  execution: {
    created_at: '2026-09-08T09:00:00Z',
    definition_name: 'import',
    execution_id: 'e-1',
    last_message_version: 3,
    mode: 'compiled',
    owner: 'Local',
    plan_id: 'plan-1',
    requested_by: 'somebody',
    state: { state_type: 'running' },
    steps: [
      {
        step_id: 'detect',
        runtime: 'marimo',
        current_attempt: 1,
        state: { state_type: 'running' },
      },
    ],
  },
};

/** The pipeline route's own wiring, so a test proves what that page does. */
function Harness() {
  const managed = useManagedRun('e-1');
  return (
    <ManagedRunCard
      run={managed.data ?? undefined}
      executionId="e-1"
      pending={false}
      missing={managed.data === null}
      failure={managed.error}
      onForget={() => {}}
    />
  );
}

describe('reading a managed run', () => {
  it('offers to forget a run the server says is not there', async () => {
    serve([
      {
        method: 'GET',
        path: EXECUTION,
        answer: { status: 404, body: refusal('not_found', 'execution e-1 not found') },
      },
    ]);

    render(withQueries(<Harness />));

    await screen.findByText(/No run under this id/);
    screen.getByRole('button', { name: /Forget it/ });
  });

  it('does not call an unreadable run forgotten', async () => {
    serve([
      {
        method: 'GET',
        path: EXECUTION,
        answer: {
          status: 503,
          body: refusal('log_unavailable', 'the event log is unavailable: connection refused'),
        },
      },
    ]);

    render(withQueries(<Harness />));

    await screen.findByText(/the event log is unavailable/);
    // The two things a 503 must not become: an absence, and an invitation to
    // drop the only link to a run that is very probably still going.
    expect(screen.queryByText(/No run under this id/)).toBeNull();
    expect(screen.queryByRole('button', { name: /Forget it/ })).toBeNull();
  });
});

describe('commanding a managed run', () => {
  it('shows a pause the caller is not allowed to make, and does not re-read the run', async () => {
    const server = serve([
      { method: 'GET', path: EXECUTION, answer: { status: 200, body: RUNNING } },
      {
        method: 'POST',
        path: '/commands/pause',
        answer: {
          status: 403,
          body: refusal('forbidden', 'this needs the admin role; you have viewer'),
        },
      },
    ]);

    render(withQueries(<Harness />));
    await screen.findByRole('button', { name: /Pause/ });
    expect(server.countOf('GET', EXECUTION)).toBe(1);

    await userEvent.click(screen.getByRole('button', { name: /Pause/ }));

    await screen.findByText(/this needs the admin role; you have viewer/);
    // The success path invalidates the run, which would re-read it. A refusal
    // that ran `onSuccess` is exactly what this counts.
    expect(server.countOf('GET', EXECUTION)).toBe(1);
    // And the run is still drawn, rather than replaced by an error page.
    expect(screen.getAllByText('running').length).toBeGreaterThan(0);
  });

  it('shows a cancel the run has already moved past', async () => {
    serve([
      { method: 'GET', path: EXECUTION, answer: { status: 200, body: RUNNING } },
      {
        method: 'POST',
        path: '/commands/cancel',
        answer: {
          status: 409,
          body: refusal('conflict', 'a completed execution cannot be cancelled'),
        },
      },
    ]);

    render(withQueries(<Harness />));
    await userEvent.click(await screen.findByRole('button', { name: /Cancel/ }));

    await screen.findByText(/a completed execution cannot be cancelled/);
  });

  it('re-reads the run when the server accepted the command', async () => {
    const paused: RunView = {
      allowed: ['resume', 'cancel'],
      execution: { ...RUNNING.execution, state: { state_type: 'paused' } },
    };
    const server = serve([
      {
        method: 'GET',
        path: EXECUTION,
        answer: (call) => ({ status: 200, body: call === 1 ? RUNNING : paused }),
      },
      { method: 'POST', path: '/commands/pause', answer: { status: 200, body: paused } },
    ]);

    render(withQueries(<Harness />));
    await userEvent.click(await screen.findByRole('button', { name: /Pause/ }));

    await screen.findByRole('button', { name: /Resume/ });
    await waitFor(() => expect(server.countOf('GET', EXECUTION)).toBe(2));
  });
});

describe('reopening one step of a run', () => {
  const CONTEXT = '/steps/detect/context';
  const PINNED = 'ab'.repeat(32);

  function marimoContext() {
    return {
      context_id: 'e-1/detect/1',
      definition_kind: 'curation_pipeline',
      definition_name: 'import',
      definition_revision: 'rev-1',
      plan_id: 'plan-1',
      step_id: 'detect',
      allowed: [],
      input_artifacts: [],
      runtime: {
        runtime: 'marimo',
        notebook: 'pii_scan',
        code_revision: PINNED,
        params: {},
        block: 'detect',
      },
    };
  }

  it('shows the source that ran, read by the digest the plan pinned', async () => {
    // The whole of a historical reload: the notebook has been edited since, and
    // this must show what the run executed rather than what is there now.
    const server = serve([
      { method: 'GET', path: EXECUTION, answer: { status: 200, body: RUNNING } },
      { method: 'GET', path: CONTEXT, answer: { status: 200, body: marimoContext() } },
      {
        method: 'GET',
        path: `/ml-pipeline/notebooks/pii_scan/revisions/${PINNED}`,
        answer: {
          status: 200,
          body: {
            name: 'pii_scan',
            title: 'What ran',
            revision: PINNED,
            size: 42,
            modified_at: '2026-09-08T09:00:00Z',
            source: 'threshold = 0.8  # the code this run executed',
            app_url: '/ml-pipeline/app/pii_scan/',
          },
        },
      },
    ]);

    render(withQueries(<Harness />));
    await userEvent.click(await screen.findByRole('button', { name: /detect/ }));
    await userEvent.click(
      await screen.findByRole('button', { name: /Show the code this step ran/ }),
    );

    await screen.findByText(/the code this run executed/);
    // By the pinned digest, never by the notebook's name alone — that route is
    // the head, and the head is what changed.
    expect(server.countOf('GET', `/ml-pipeline/notebooks/pii_scan/revisions/${PINNED}`)).toBe(1);
    expect(server.countOf('GET', '/ml-pipeline/notebooks/pii_scan')).toBe(0);
  });

  it('says the runtime is not answering rather than showing nothing', async () => {
    serve([
      { method: 'GET', path: EXECUTION, answer: { status: 200, body: RUNNING } },
      { method: 'GET', path: CONTEXT, answer: { status: 200, body: marimoContext() } },
      {
        method: 'GET',
        path: `/ml-pipeline/notebooks/pii_scan/revisions/${PINNED}`,
        answer: { status: 503, body: { error: { message: 'not running' } } },
      },
    ]);

    render(withQueries(<Harness />));
    await userEvent.click(await screen.findByRole('button', { name: /detect/ }));
    await userEvent.click(
      await screen.findByRole('button', { name: /Show the code this step ran/ }),
    );

    // And says the run executed it anyway, because it did.
    await screen.findByText(/It is still what the run executed/);
  });
});

describe('answering a step that is waiting on a person', () => {
  const CONTEXT = '/steps/sign-off/context';
  const ANSWER = '/steps/sign-off/input';

  const WAITING: RunView = {
    allowed: ['pause', 'cancel'],
    execution: {
      ...RUNNING.execution,
      steps: [
        {
          step_id: 'sign-off',
          runtime: 'human_input',
          current_attempt: 1,
          state: { state_type: 'awaiting_input' },
        },
      ],
    },
  };

  /** The context of a gate the run is stopped on, as the server answers it. */
  function gate(choices: string[], deadline?: string) {
    return {
      context_id: 'e-1/sign-off/1',
      definition_kind: 'curation_pipeline',
      definition_name: 'import',
      definition_revision: 'rev-1',
      plan_id: 'plan-1',
      step_id: 'sign-off',
      allowed: ['answer'],
      input_artifacts: [],
      runtime: {
        runtime: 'human_input',
        prompt: 'Publish these 4,120 rows?',
        role: 'editor',
        choices,
        block: 'sign-off',
      },
      state: {
        step_id: 'sign-off',
        current_attempt: 1,
        state: { state_type: 'awaiting_input' },
        awaiting: { prompt: 'Publish these 4,120 rows?', role: 'editor', choices, deadline },
      },
    };
  }

  /** Authentication on, and this is who is asking. */
  function signedInAs(role: string) {
    return [
      {
        method: 'GET',
        path: '/auth/config',
        answer: { status: 200, body: { enabled: true } },
      },
      {
        method: 'GET',
        path: '/auth/me',
        answer: {
          status: 200,
          body: { credential: 'session', subject: 'somebody', roles: [role] },
        },
      },
    ];
  }

  it('offers exactly the answers the question declared, and sends the attempt that asked', async () => {
    // The negative control for the choices: offer anything the spec does not
    // carry and the answer comes back a 409 nobody could have predicted from
    // the screen. So the buttons *are* the spec's list, and nothing else.
    const server = serve([
      ...signedInAs('editor'),
      { method: 'GET', path: EXECUTION, answer: { status: 200, body: WAITING } },
      {
        method: 'GET',
        path: CONTEXT,
        answer: { status: 200, body: gate(['approve', 'reject']) },
      },
      { method: 'POST', path: ANSWER, answer: { status: 200, body: WAITING } },
    ]);

    render(withQueries(<Harness />));
    await userEvent.click(await screen.findByRole('button', { name: /sign-off/ }));

    await screen.findByText('Publish these 4,120 rows?');
    // Exactly these, and in this order — not "these among others". An answer
    // the step never declared is refused by name, so a button offering one is
    // a 409 somebody had to press to find out about.
    const answers = within(screen.getByRole('group', { name: 'Answers' })).getAllByRole('button');
    expect(answers.map((button) => button.textContent)).toEqual(['approve', 'reject']);
    // A question with a list is answered from the list. A free-text box beside
    // it would be a way to type something the server will refuse.
    expect(screen.queryByPlaceholderText('Your answer')).toBeNull();

    await userEvent.click(screen.getByRole('button', { name: 'approve' }));

    await waitFor(() => expect(server.countOf('POST', ANSWER)).toBe(1));
  });

  it('says when a question stops being answerable, and only when it does', async () => {
    // A deadline is shown rather than counted down: the moment is the fact, and
    // a clock in the browser that a reload resets would be a second answer to
    // "when", free to disagree with the one the server is holding.
    serve([
      ...signedInAs('editor'),
      { method: 'GET', path: EXECUTION, answer: { status: 200, body: WAITING } },
      {
        method: 'GET',
        path: CONTEXT,
        answer: { status: 200, body: gate(['approve'], '2026-09-10T09:00:00Z') },
      },
    ]);

    render(withQueries(<Harness />));
    await userEvent.click(await screen.findByRole('button', { name: /sign-off/ }));

    await screen.findByText(/Answerable until/);
  });

  it('says nothing about a deadline on a gate that waits as long as it takes', async () => {
    serve([
      ...signedInAs('editor'),
      { method: 'GET', path: EXECUTION, answer: { status: 200, body: WAITING } },
      { method: 'GET', path: CONTEXT, answer: { status: 200, body: gate(['approve']) } },
    ]);

    render(withQueries(<Harness />));
    await userEvent.click(await screen.findByRole('button', { name: /sign-off/ }));

    await screen.findByRole('button', { name: 'approve' });
    expect(screen.queryByText(/Answerable until/)).toBeNull();
  });

  it('types the answer to a question that offered none', async () => {
    serve([
      ...signedInAs('editor'),
      { method: 'GET', path: EXECUTION, answer: { status: 200, body: WAITING } },
      { method: 'GET', path: CONTEXT, answer: { status: 200, body: gate([]) } },
      { method: 'POST', path: ANSWER, answer: { status: 200, body: WAITING } },
    ]);

    render(withQueries(<Harness />));
    await userEvent.click(await screen.findByRole('button', { name: /sign-off/ }));

    await screen.findByPlaceholderText('Your answer');
    expect(screen.queryByRole('button', { name: 'approve' })).toBeNull();
  });

  it('tells a viewer whose decision this is rather than offering a button that would be refused', async () => {
    serve([
      ...signedInAs('viewer'),
      { method: 'GET', path: EXECUTION, answer: { status: 200, body: WAITING } },
      {
        method: 'GET',
        path: CONTEXT,
        answer: { status: 200, body: gate(['approve', 'reject']) },
      },
    ]);

    render(withQueries(<Harness />));
    await userEvent.click(await screen.findByRole('button', { name: /sign-off/ }));

    // The question is still shown: a run stopped on a decision somebody else
    // has to make is exactly what a viewer needs to be able to see.
    await screen.findByText('Publish these 4,120 rows?');
    await screen.findByText(/This needs the editor role/);
    expect(screen.queryByRole('button', { name: 'approve' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'reject' })).toBeNull();
  });
});

describe('opening the editor on a step', () => {
  const CONTEXT = '/steps/detect/context';
  const EDITOR = '/steps/detect/editor';
  const PINNED = 'ab'.repeat(32);

  function marimoContext() {
    return {
      context_id: 'e-1/detect/1',
      definition_kind: 'curation_pipeline',
      definition_name: 'import',
      definition_revision: 'rev-1',
      plan_id: 'plan-1',
      step_id: 'detect',
      allowed: [],
      input_artifacts: [],
      runtime: {
        runtime: 'marimo',
        notebook: 'pii_scan',
        code_revision: PINNED,
        params: {},
        block: 'detect',
      },
    };
  }

  it("asks the server to stage this attempt's rows, and links to the app", async () => {
    // A button rather than something that happens on open: staging replaces
    // what everybody looking at that notebook's live app is shown.
    const server = serve([
      { method: 'GET', path: EXECUTION, answer: { status: 200, body: RUNNING } },
      { method: 'GET', path: CONTEXT, answer: { status: 200, body: marimoContext() } },
      {
        method: 'POST',
        path: EDITOR,
        answer: {
          status: 200,
          body: {
            app_url: '/ml-pipeline/app/pii_scan/',
            context_id: 'e-1/detect/1',
            notebook: 'pii_scan',
            code_revision: PINNED,
            rows: 12,
          },
        },
      },
    ]);

    render(withQueries(<Harness />));
    await userEvent.click(await screen.findByRole('button', { name: /detect/ }));
    expect(server.countOf('POST', EDITOR)).toBe(0);

    await userEvent.click(
      await screen.findByRole('button', { name: /Open the editor on this step/ }),
    );

    const link = await screen.findByRole('link', { name: /Open the notebook on 12 rows/ });
    expect(link.getAttribute('href')).toBe('/ml-pipeline/app/pii_scan/');
    expect(server.countOf('POST', EDITOR)).toBe(1);
  });

  it('reports a refused open rather than a dead button', async () => {
    serve([
      { method: 'GET', path: EXECUTION, answer: { status: 200, body: RUNNING } },
      { method: 'GET', path: CONTEXT, answer: { status: 200, body: marimoContext() } },
      {
        method: 'POST',
        path: EDITOR,
        answer: {
          status: 501,
          body: refusal(
            'editor_disabled',
            'this instance opens no notebook editor (AIWATCHER_ML_PIPELINE_URL, AIWATCHER_PROMPT_STORE)',
          ),
        },
      },
    ]);

    render(withQueries(<Harness />));
    await userEvent.click(await screen.findByRole('button', { name: /detect/ }));
    await userEvent.click(
      await screen.findByRole('button', { name: /Open the editor on this step/ }),
    );

    await screen.findByText(/AIWATCHER_ML_PIPELINE_URL/);
    expect(screen.queryByRole('link', { name: /Open the notebook/ })).toBeNull();
  });
});
