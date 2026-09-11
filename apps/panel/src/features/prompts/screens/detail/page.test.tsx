import * as React from 'react';
import { render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type {
  OptimizationRecord,
  OptimizationSummary,
  PromptDetail,
  PromptVersion,
} from '@/api/generated/types.gen';

import { type Route, refusal, serve, withQueries } from '@/test/server';

import { PromptPage } from '@/features/prompts/screens/detail/page';

const NAME = 'triage';
const BASELINE = 'a'.repeat(64);
const CANDIDATE = 'b'.repeat(64);
const OPTIMIZATION = 'opt-7f3c2a9e1d4b';

// The page reads its name and its selection from the URL. Holding them here is
// the whole router as far as this page is concerned.
vi.mock('@tanstack/react-router', () => ({
  Link: ({ children }: { children: React.ReactNode }) => <a href="#link">{children}</a>,
  getRouteApi: () => ({
    useParams: () => ({ name: NAME }),
    useSearch: () => ({ version: CANDIDATE }),
    useNavigate: () => () => Promise.resolve(),
  }),
}));

afterEach(() => vi.unstubAllGlobals());

const PROMPT = `/api/v1/prompts/${NAME}`;
const VERSION = `${PROMPT}/versions/${CANDIDATE}`;
const RECORD = `${PROMPT}/optimizations/${OPTIMIZATION}`;
const LABEL = `${PROMPT}/labels/production`;

const AUTHORED: PromptVersion = {
  origin: 'authored',
  name: NAME,
  version_id: BASELINE,
  text: 'Classify {{ ticket }}.',
  variables: ['ticket'],
  created_at: '2026-09-10T08:00:00Z',
};

const OPTIMIZED: PromptVersion = {
  origin: 'optimized',
  algorithm: 'deepeval/SIMBA',
  optimization_id: OPTIMIZATION,
  name: NAME,
  version_id: CANDIDATE,
  parent: BASELINE,
  text: 'Classify the ticket carefully.',
  variables: [],
  created_at: '2026-09-11T08:00:00Z',
};

function summary(
  outcome: OptimizationSummary['outcome'],
  extra: Partial<OptimizationSummary> = {},
) {
  return {
    optimization_id: OPTIMIZATION,
    algorithm: 'deepeval/SIMBA',
    baseline: BASELINE,
    candidate: CANDIDATE,
    primary_metric: 'accuracy',
    started_at: '2026-09-11T08:00:00Z',
    outcome,
    ...extra,
  } satisfies OptimizationSummary;
}

function record(outcome: OptimizationRecord['outcome'], extra: Partial<OptimizationRecord> = {}) {
  return { ...summary(outcome), prompt: NAME, ...extra } satisfies OptimizationRecord;
}

/** The detail with `production` on the baseline, so the candidate is not already it. */
function detail(selected: PromptVersion, optimizations: OptimizationSummary[]): PromptDetail {
  return {
    current: selected,
    head: {
      name: NAME,
      created_at: '2026-09-10T08:00:00Z',
      updated_at: '2026-09-11T08:00:00Z',
      labels: { production: BASELINE },
      versions: [
        { ...selected, size_bytes: selected.text.length },
        { ...AUTHORED, size_bytes: AUTHORED.text.length },
      ].filter(
        (version, index, all) =>
          all.findIndex((other) => other.version_id === version.version_id) === index,
      ),
      optimizations,
    },
  };
}

/** Authentication off, so the only thing standing between a click and the label is the verdict. */
function routes(
  selected: PromptVersion,
  optimizations: OptimizationSummary[],
  extra: Route[] = [],
) {
  return [
    { method: 'GET', path: '/auth/config', answer: { status: 200, body: { enabled: false } } },
    { method: 'GET', path: PROMPT, answer: { status: 200, body: detail(selected, optimizations) } },
    { method: 'GET', path: VERSION, answer: { status: 200, body: selected } },
    ...extra,
  ];
}

async function promoteButton(): Promise<HTMLButtonElement> {
  return (await screen.findByRole('button', { name: /Make production/ })) as HTMLButtonElement;
}

/** The sentence a disabled button points at — a title alone is not read out. */
function describedBy(button: HTMLButtonElement): string {
  const id = button.getAttribute('aria-describedby');
  return (id && document.getElementById(id)?.textContent) || '';
}

describe('the Make production button', () => {
  it('is withheld from a candidate the index records as rejected, saying why', async () => {
    const server = serve(
      routes(OPTIMIZED, [summary('rejected', { reason: 'no_held_out_improvement' })]),
    );
    render(withQueries(<PromptPage />));

    const button = await promoteButton();
    await waitFor(() => expect(button.disabled).toBe(true));
    expect(describedBy(button)).toMatch(
      /Cannot be made production: the held-out score did not improve/,
    );
    // Read from the head: the capped index already had it, so nothing else was asked.
    expect(server.countOf('GET', RECORD)).toBe(0);
    expect(server.countOf('PUT', LABEL)).toBe(0);
  });

  it('reads an optimisation that fell out of the capped index, and follows its verdict', async () => {
    const server = serve(
      routes(
        OPTIMIZED,
        [],
        [
          {
            method: 'GET',
            path: RECORD,
            answer: {
              status: 200,
              body: record('rejected', { reason: 'variables_lost', variables_lost: ['ticket'] }),
            },
          },
        ],
      ),
    );
    render(withQueries(<PromptPage />));

    const button = await promoteButton();
    await waitFor(() => expect(button.disabled).toBe(true));
    await waitFor(() =>
      expect(describedBy(button)).toMatch(
        /stopped interpolating a variable the baseline used \(ticket\)/,
      ),
    );
    expect(server.countOf('GET', RECORD)).toBe(1);
  });

  it('is withheld when no verdict was ever recorded, as the registry refuses it', async () => {
    serve(
      routes(
        OPTIMIZED,
        [],
        [
          {
            method: 'GET',
            path: RECORD,
            answer: { status: 404, body: refusal('not_found', 'no such optimisation') },
          },
        ],
      ),
    );
    render(withQueries(<PromptPage />));

    const button = await promoteButton();
    await waitFor(() => expect(describedBy(button)).toMatch(/no verdict was recorded/));
    expect(button.disabled).toBe(true);
  });

  it('stays for a candidate the verdict admitted', async () => {
    serve(routes(OPTIMIZED, [summary('admitted')]));
    render(withQueries(<PromptPage />));

    const button = await promoteButton();
    await waitFor(() => expect(button.disabled).toBe(false));
    expect(button.getAttribute('aria-describedby')).toBeNull();
  });

  it('stays for a version somebody wrote, which has no verdict to answer to', async () => {
    const server = serve(routes({ ...AUTHORED, version_id: CANDIDATE, parent: BASELINE }, []));
    render(withQueries(<PromptPage />));

    const button = await promoteButton();
    await waitFor(() => expect(button.disabled).toBe(false));
    expect(server.calls.some((call) => call.url.includes('/optimizations/'))).toBe(false);
  });

  it('leaves the decision to the registry when the verdict could not be read', async () => {
    // A 503 is not "rejected": withholding the button would be drawing a failed
    // read as an answer. The server still refuses if it has to.
    serve(
      routes(
        OPTIMIZED,
        [],
        [
          {
            method: 'GET',
            path: RECORD,
            answer: { status: 503, body: refusal('registry_unavailable', 'the store is down') },
          },
        ],
      ),
    );
    render(withQueries(<PromptPage />));

    await screen.findByText(/Could not read the verdict on this candidate \(the store is down\)/);
    expect((await promoteButton()).disabled).toBe(false);
  });
});
