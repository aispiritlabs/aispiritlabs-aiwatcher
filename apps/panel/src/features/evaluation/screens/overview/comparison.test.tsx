import { render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';

import type {
  CaseDiffPage,
  DurableEvaluation,
  EvidenceCaseDelta,
  EvidenceComparison,
  EvidenceMetricDelta,
} from '@/api/generated/types.gen';
import { serve } from '@/test/server';
import { withQueries } from '@/test/server';

import { Comparison } from './comparison';

afterEach(() => vi.unstubAllGlobals());

function result(id: string, context = 'bb22'): DurableEvaluation {
  return {
    receipt: {
      evaluation_id: id,
      version: `v-${id}`,
      variant_id: `var-${id}`,
      context_id: context,
      committed_at: 1789200000,
      expires_at: 1791792000,
    },
    state: 'complete',
    metrics: { accuracy: 0.75 },
    counts: { selected: 4, scored: 4, failed: 0, unscored: 0 },
    manifest: null,
    status: 'succeeded',
  };
}

function comparison(
  metrics: EvidenceMetricDelta[],
  extra: Partial<EvidenceComparison> = {},
): EvidenceComparison {
  return {
    current: result('after'),
    baseline: result('before'),
    comparability: 'comparable',
    reasons: [],
    same_variant: false,
    metrics,
    ...extra,
  };
}

function only(candidates: DurableEvaluation[], answer?: EvidenceComparison) {
  serve([
    { method: 'GET', path: '/auth/config', answer: { status: 200, body: { enabled: false } } },
    {
      method: 'GET',
      path: '/evaluation-results',
      answer: { status: 200, body: { evaluations: candidates, next_cursor: null } },
    },
    ...(answer
      ? [{ method: 'GET' as const, path: '/comparison', answer: { status: 200, body: answer } }]
      : []),
  ]);
}

it('offers what the server returned rather than deciding again which results compare', async () => {
  // The row's context does not match the open result's, and it is still
  // offered: which results may be compared is the server's rule, and a browser
  // re-deciding it would be a second answer free to disagree with the first.
  only([result('before'), result('elsewhere', 'different-context')]);
  render(
    withQueries(<Comparison evidence={result('after')} baseline={undefined} onSelect={() => {}} />),
  );
  await screen.findByRole('option', { name: /elsewhere/ });
  const chooser = screen.getByLabelText('Baseline') as HTMLSelectElement;
  const offered = [...chooser.options].map((option) => option.value).filter(Boolean);
  expect(offered).toEqual(['before', 'elsewhere']);
});

it('never offers the open result as its own baseline', async () => {
  only([result('after'), result('before')]);
  render(
    withQueries(<Comparison evidence={result('after')} baseline={undefined} onSelect={() => {}} />),
  );
  await screen.findByRole('option', { name: /before/ });
  const chooser = screen.getByLabelText('Baseline') as HTMLSelectElement;
  expect([...chooser.options].map((option) => option.value)).not.toContain('after');
});

it('withholds a delta visibly and says why, rather than leaving it absent', async () => {
  only(
    [result('before')],
    comparison(
      [{ name: 'accuracy', current: 0.75, baseline: 0.5, direction: 'higher', unit: 'ratio' }],
      { comparability: 'incompatible', reasons: ['Different split'] },
    ),
  );
  render(
    withQueries(<Comparison evidence={result('after')} baseline="before" onSelect={() => {}} />),
  );
  expect(await screen.findByText('Different split')).toBeTruthy();
  expect(screen.getByRole('group', { name: 'Comparability' })).toBeTruthy();
  expect(screen.getByText(/not measured on the same thing/)).toBeTruthy();
  // Both numbers are facts; only the difference is a claim nobody measured.
  expect(screen.getByText('0.7500')).toBeTruthy();
  expect(screen.getByText('0.5000')).toBeTruthy();
  expect(screen.getByText('—')).toBeTruthy();
});

it('colours a delta only where the pinned context declares which way is better', async () => {
  only(
    [result('before')],
    comparison([
      { name: 'accuracy', current: 0.75, baseline: 0.5, delta: 0.25, direction: 'higher' },
      { name: 'tokens', current: 120, baseline: 100, delta: 20, direction: 'none' },
    ]),
  );
  render(
    withQueries(<Comparison evidence={result('after')} baseline="before" onSelect={() => {}} />),
  );
  const improved = await screen.findByText('+0.2500');
  expect(improved.className).toContain('text-primary');
  // Nobody said whether more tokens is better, so the number stays plain.
  const unknown = screen.getByText('+20.0000');
  expect(unknown.className).not.toContain('text-primary');
  expect(unknown.className).not.toContain('text-danger');
});

it('says a comparison of one variant with itself measures repetition, not a change', async () => {
  only([result('before')], comparison([], { same_variant: true }));
  render(
    withQueries(<Comparison evidence={result('after')} baseline="before" onSelect={() => {}} />),
  );
  expect(await screen.findByText(/repetition/)).toBeTruthy();
});

it('does not describe a delta as repetition where there is no delta to misread', async () => {
  only(
    [result('before')],
    comparison([], { same_variant: true, comparability: 'incompatible', reasons: ['Different split'] }),
  );
  render(
    withQueries(<Comparison evidence={result('after')} baseline="before" onSelect={() => {}} />),
  );
  expect(await screen.findByText('Different split')).toBeTruthy();
  expect(screen.queryByText(/repetition/)).toBeNull();
});

it('says a context with nothing else in it has nothing to compare, and offers no control', async () => {
  only([result('after')]);
  render(
    withQueries(<Comparison evidence={result('after')} baseline={undefined} onSelect={() => {}} />),
  );
  expect(await screen.findByText(/Nothing else has been published/)).toBeTruthy();
});

it('still asks about a baseline the URL named that this context does not offer', async () => {
  // The server's answer to that pair is "incompatible", and which field moved
  // — which is the question somebody following a pasted link is asking. A
  // control that reset itself to the candidates would answer a different one.
  only(
    [],
    comparison([], { comparability: 'incompatible', reasons: ['Different split'] }),
  );
  render(
    withQueries(<Comparison evidence={result('after')} baseline="elsewhere" onSelect={() => {}} />),
  );
  expect(await screen.findByText('Different split')).toBeTruthy();
  const chooser = screen.getByLabelText('Baseline') as HTMLSelectElement;
  expect(chooser.value).toBe('elsewhere');
});

// ── Which cases moved ────────────────────────────────────────────────────────

function moved(cases: EvidenceCaseDelta[], next: string | null = null): CaseDiffPage {
  return { comparability: 'comparable', reasons: [], cases, next_cursor: next, state: 'complete' };
}

function withCases(page: CaseDiffPage) {
  serve([
    { method: 'GET', path: '/auth/config', answer: { status: 200, body: { enabled: false } } },
    {
      method: 'GET',
      path: '/evaluation-results',
      answer: { status: 200, body: { evaluations: [result('before')], next_cursor: null } },
    },
    {
      method: 'GET',
      path: '/comparison/cases',
      answer: { status: 200, body: page },
    },
    {
      method: 'GET',
      path: '/comparison',
      answer: {
        status: 200,
        body: comparison([
          { name: 'accuracy', current: 0.66, baseline: 1.0, delta: -0.34, direction: 'higher' },
        ]),
      },
    },
  ]);
}

it('does not read both results until somebody asks which cases moved', async () => {
  const server = serve([
    { method: 'GET', path: '/auth/config', answer: { status: 200, body: { enabled: false } } },
    {
      method: 'GET',
      path: '/evaluation-results',
      answer: { status: 200, body: { evaluations: [result('before')], next_cursor: null } },
    },
    {
      method: 'GET',
      path: '/comparison',
      answer: { status: 200, body: comparison([]) },
    },
  ]);
  render(
    withQueries(
      <Comparison
        evidence={result('after')}
        baseline="before"
        onSelect={() => {}}
        cases={undefined}
        onCases={() => {}}
      />,
    ),
  );
  expect(await screen.findByText(/Reads both results in full/)).toBeTruthy();
  // The comparison above it is two headers; this one is two whole results, and
  // opening a result must not pay for it.
  expect(server.countOf('GET', '/comparison/cases')).toBe(0);
});

it('shows a case that stopped being measurable with its error and no invented delta', async () => {
  withCases(
    moved([
      {
        case_id: 'two-plus-two',
        change: 'regressed',
        current: { error: 'the model timed out' },
        baseline: {},
        metrics: [{ name: 'accuracy', baseline: 1, direction: 'higher', unit: 'ratio' }],
      },
    ]),
  );
  render(
    withQueries(
      <Comparison
        evidence={result('after')}
        baseline="before"
        onSelect={() => {}}
        cases="worse"
        onCases={() => {}}
      />,
    ),
  );
  expect(await screen.findByText('two-plus-two')).toBeTruthy();
  expect(screen.getByText('regressed')).toBeTruthy();
  expect(screen.getByText('the model timed out')).toBeTruthy();
  // One side has a number and the other has none, so there is no difference to
  // show — and a zero there would say the case scored the same.
  const row = screen.getByText('two-plus-two').closest('tr');
  expect(row?.textContent).toContain('1.0000');
  expect(row?.textContent).toContain('—');
  expect(row?.textContent).not.toMatch(/0\.0000/);
});

it('renders what the server narrowed to rather than narrowing it again here', async () => {
  // Under "lost something" the server answered with a case it calls unchanged.
  // The browser shows it: which cases moved is decided where the pinned
  // context's declared directions are, and a second rule here would be free to
  // disagree with the one that produced the page.
  withCases(
    moved([
      {
        case_id: 'capital-pl',
        change: 'unchanged',
        current: {},
        baseline: {},
        metrics: [],
      },
    ]),
  );
  render(
    withQueries(
      <Comparison
        evidence={result('after')}
        baseline="before"
        onSelect={() => {}}
        cases="worse"
        onCases={() => {}}
      />,
    ),
  );
  expect(await screen.findByText('capital-pl')).toBeTruthy();
  expect(screen.getByText('unchanged')).toBeTruthy();
});

it('says another page means another case rather than another match', async () => {
  withCases(moved([], 'v-after:100:v-before:100'));
  render(
    withQueries(
      <Comparison
        evidence={result('after')}
        baseline="before"
        onSelect={() => {}}
        cases="worse"
        onCases={() => {}}
      />,
    ),
  );
  expect(await screen.findByText(/there are more to read/)).toBeTruthy();
  expect(screen.getByRole('button', { name: /read further into both results/ })).toBeTruthy();
});
