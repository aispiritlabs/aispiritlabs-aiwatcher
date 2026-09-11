import * as React from 'react';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { ScheduleView } from '@/api/generated/types.gen';

import { refusal, serve, withQueries } from '@/test/server';

import { ScheduleCard } from '@/shared/components/schedule-card';

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children }: { children: React.ReactNode }) => <a href="#run">{children}</a>,
}));

afterEach(() => vi.unstubAllGlobals());

const SCHEDULE = '/api/v1/curation-pipelines/import/schedule';

const AT_SEVEN: ScheduleView = {
  next_run: '2026-09-09T05:00:00Z',
  schedule: {
    definition_kind: 'curation_pipeline',
    definition_name: 'import',
    updated_at: '2026-09-08T08:00:00Z',
    schedule: {
      cadence: { every: 'daily', hour: 7, minute: 30 },
      timezone: 'Europe/Warsaw',
      enabled: true,
      overlap: 'skip',
    },
  },
};

function button(name: RegExp): HTMLButtonElement {
  return screen.getByRole('button', { name }) as HTMLButtonElement;
}

/** The hour box, which is what a refused DELETE must not quietly reset. */
function hourBox(): HTMLInputElement {
  const boxes = screen.getAllByRole('spinbutton') as HTMLInputElement[];
  const hour = boxes[0];
  if (!hour) throw new Error('no hour field rendered');
  return hour;
}

describe('reading a schedule', () => {
  it('treats a pipeline with no schedule as the ordinary empty case', async () => {
    serve([
      {
        method: 'GET',
        path: SCHEDULE,
        answer: { status: 404, body: refusal('not_found', 'import has no schedule') },
      },
    ]);

    render(withQueries(<ScheduleCard name="import" saved />));

    await waitFor(() => expect(hourBox().value).toBe('9'));
    // No refusal, and the form works: this is what most pipelines look like.
    expect(screen.queryByRole('alert')).toBeNull();
    expect(button(/^Save$/).disabled).toBe(false);
    expect(screen.queryByRole('button', { name: /Forget/ })).toBeNull();
  });

  it('says so when the schedule could not be read, and will not save over it', async () => {
    serve([
      {
        method: 'GET',
        path: SCHEDULE,
        answer: {
          status: 501,
          body: refusal(
            'executions_disabled',
            'this instance has no workflow store configured (AIWATCHER_WORKFLOW_STORE)',
          ),
        },
      },
    ]);

    render(withQueries(<ScheduleCard name="import" saved />));

    await screen.findByText(/AIWATCHER_WORKFLOW_STORE/);
    // The form was never filled in, so Save would write this component's own
    // defaults over a schedule nobody has seen.
    expect(button(/^Save$/).disabled).toBe(true);
    expect(button(/Save and run now/).disabled).toBe(true);
  });
});

describe('forgetting a schedule', () => {
  it('clears the form when the server actually deleted it', async () => {
    const server = serve([
      {
        method: 'GET',
        path: SCHEDULE,
        answer: (call) =>
          call === 1
            ? { status: 200, body: AT_SEVEN }
            : { status: 404, body: refusal('not_found', 'import has no schedule') },
      },
      { method: 'DELETE', path: SCHEDULE, answer: { status: 204 } },
    ]);

    render(withQueries(<ScheduleCard name="import" saved />));
    await waitFor(() => expect(hourBox().value).toBe('7'));

    await userEvent.click(screen.getByRole('button', { name: /Forget/ }));

    await waitFor(() => expect(hourBox().value).toBe('9'));
    expect(server.countOf('DELETE', SCHEDULE)).toBe(1);
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('keeps the schedule on screen when the delete was refused', async () => {
    serve([
      { method: 'GET', path: SCHEDULE, answer: { status: 200, body: AT_SEVEN } },
      {
        method: 'DELETE',
        path: SCHEDULE,
        answer: {
          status: 403,
          body: refusal('forbidden', 'this needs the admin role; you have viewer'),
        },
      },
    ]);

    render(withQueries(<ScheduleCard name="import" saved />));
    await waitFor(() => expect(hourBox().value).toBe('7'));

    await userEvent.click(screen.getByRole('button', { name: /Forget/ }));

    await screen.findByText(/this needs the admin role; you have viewer/);
    // The schedule is still there, so the form still shows it. Resetting it
    // here is what told somebody a schedule was gone that was not.
    expect(hourBox().value).toBe('7');
    screen.getByRole('button', { name: /Forget/ });
  });
});

describe('saving a schedule', () => {
  it('reports the refusal with every problem the server named', async () => {
    serve([
      {
        method: 'GET',
        path: SCHEDULE,
        answer: { status: 404, body: refusal('not_found', 'import has no schedule') },
      },
      {
        method: 'PUT',
        path: SCHEDULE,
        answer: {
          status: 422,
          body: refusal('plan_refused', 'that pipeline does not compile', [
            'the view block names no dataset',
            'a notebook block follows nothing',
          ]),
        },
      },
    ]);

    render(withQueries(<ScheduleCard name="import" saved />));
    await waitFor(() => expect(hourBox().value).toBe('9'));

    await userEvent.click(screen.getByRole('button', { name: /^Save$/ }));

    await screen.findByText('the view block names no dataset');
    screen.getByText('a notebook block follows nothing');
  });
});
