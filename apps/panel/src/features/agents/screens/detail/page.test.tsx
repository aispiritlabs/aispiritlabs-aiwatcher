import {
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
  RouterProvider,
} from '@tanstack/react-router';
import { render, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';

import { AgentPage } from './page';
import { searchSchema } from './search';
import { serve, withQueries } from '@/test/server';

afterEach(() => vi.unstubAllGlobals());

const NOTHING = { count: 0, p50: 0, p95: 0, p99: 0 };

function metrics(overrides: Record<string, unknown> = {}) {
  return {
    totals: {
      runs: 4,
      succeeded: 3,
      failed: 1,
      running: 0,
      llm_calls: 9,
      tool_calls: 2,
      step_calls: 0,
      input_tokens: 100,
      output_tokens: 20,
      cached_tokens: 0,
      cache_hit_ratio: 0,
      costed_calls: 0,
    },
    latency: { runs: NOTHING, llm: NOTHING, tool: NOTHING, step: NOTHING, ttft: NOTHING },
    window: {
      from: '2026-09-18T10:00:00Z',
      to: '2026-09-18T11:00:00Z',
      runs_considered: 4,
      runs_retained: 4,
      retention_limit: 5000,
    },
    timeline: [],
    by_agent: [
      {
        agent_id: 'researcher',
        runs: 4,
        failures: 1,
        llm_calls: 6,
        tool_calls: 2,
        input_tokens: 80,
        output_tokens: 16,
        cached_tokens: 0,
        cost_usd: 0.42,
        llm_latency: { count: 6, p50: 800, p95: 1500, p99: 1800 },
      },
      {
        agent_id: 'writer',
        runs: 2,
        failures: 0,
        llm_calls: 3,
        tool_calls: 0,
        input_tokens: 20,
        output_tokens: 4,
        cached_tokens: 0,
        cost_usd: null,
        llm_latency: NOTHING,
      },
    ],
    by_model: [],
    by_tool: [],
    by_step: [],
    ...overrides,
  };
}

/** One dimension row, with the fields this page reads. */
function row(key: string, runs: number) {
  return {
    key,
    runs,
    running: 0,
    failures: 0,
    llm_calls: 0,
    tool_calls: 0,
    input_tokens: 0,
    output_tokens: 0,
    cached_tokens: 0,
    first_activity_at: '2026-09-18T10:00:00Z',
    last_activity_at: '2026-09-18T11:00:00Z',
  };
}

const REGISTERED = {
  status: 200,
  body: { prompts: [{ name: 'extract-rooms' }], total: 1, next_cursor: null },
};

function mount(
  search = '',
  body: unknown = metrics(),
  prompts: { status: number; body?: unknown } = REGISTERED,
  second?: unknown,
) {
  vi.stubGlobal('scrollTo', () => {});
  serve([
    {
      method: 'GET',
      path: '/metrics',
      answer: (call) =>
        call > 1 && second !== undefined
          ? { status: 200, body: second }
          : { status: 200, body },
    },
    {
      method: 'GET',
      path: '/runs',
      answer: { status: 200, body: { runs: [], total_known: 0, next_cursor: null } },
    },
    {
      method: 'GET',
      path: '/dimensions/workflow',
      answer: { status: 200, body: { kind: 'workflow', rows: [], total: 0, ungrouped_runs: 0 } },
    },
    {
      method: 'GET',
      path: '/dimensions/runtime',
      answer: { status: 200, body: { kind: 'runtime', rows: [], total: 0, ungrouped_runs: 0 } },
    },
    {
      method: 'GET',
      path: '/dimensions/prompt',
      answer: {
        status: 200,
        body: {
          kind: 'prompt',
          rows: [row('extract-rooms', 4), row('retired-draft', 1)],
          total: 2,
          ungrouped_runs: 3,
        },
      },
    },
    { method: 'GET', path: '/prompts', answer: prompts },
  ]);
  const requests = vi.fn(fetch);
  vi.stubGlobal('fetch', requests);
  const root = createRootRoute();
  const route = createRoute({
    getParentRoute: () => root,
    path: '/agents/$agentId',
    validateSearch: searchSchema,
    component: AgentPage,
  });
  const router = createRouter({
    routeTree: root.addChildren([route]),
    history: createMemoryHistory({ initialEntries: [`/agents/researcher${search}`] }),
  });
  render(withQueries(<RouterProvider router={router} />));
  return { requests, router };
}

function urlFor(requests: ReturnType<typeof vi.fn>, path: string) {
  return requests.mock.calls
    .map(([request]) => new URL((request as Request).url))
    .find((url) => url.pathname.endsWith(path));
}

it('reads the agent’s own numbers out of by_agent, not the totals beside them', async () => {
  // The distinction this page is built on: `totals` covers every span of the
  // runs this agent took part in, and a run where it hands work to another
  // agent counts that agent's calls there. `by_agent` is keyed by the agent on
  // each span, so it is this agent's own.
  const { requests } = mount();
  await screen.findByText('What this agent did');
  // 6, not the 9 in `totals`: the other three are the writer's, in the same
  // runs.
  expect(within(screen.getByText('LLM calls').parentElement!).getByText('6')).toBeTruthy();
  expect(within(screen.getByText('Cost').parentElement!).getByText('$0.42')).toBeTruthy();
  expect(urlFor(requests, '/metrics')?.searchParams.get('agent_id')).toBe('researcher');
});

it('carries a filter it arrived with into the runs it can apply it to', async () => {
  const { requests } = mount('?workflow=house-import');
  await screen.findByText('What this agent did');
  expect(urlFor(requests, '/runs')?.searchParams.get('workflow')).toBe('house-import');
  expect(urlFor(requests, '/runs')?.searchParams.get('agent_id')).toBe('researcher');
  // Both reads take it now, so the strip and the list are about one population
  // — which is what this page claims by putting them on one screen.
  expect(urlFor(requests, '/metrics')?.searchParams.get('workflow')).toBe('house-import');
  expect(urlFor(requests, '/metrics')?.searchParams.get('agent_id')).toBe('researcher');
  expect(screen.queryByText(/Not applied here/)).toBeNull();
});

it('still says when an axis the page is about is contradicted', async () => {
  // The path is the agent, so a link naming a different one is reported rather
  // than quietly overridden — the one thing on this page a filter cannot do.
  const { requests } = mount('?agent=planner');
  await screen.findByText('What this agent did');
  expect(screen.getByText(/this page is about researcher/)).toBeTruthy();
  expect(urlFor(requests, '/runs')?.searchParams.get('agent_id')).toBe('researcher');
});

it('says the period holds nothing rather than pretending the agent is unknown', async () => {
  // Nothing here registers an agent, so a name with no runs in the window is
  // an empty period and never a 404.
  mount('', metrics({ by_agent: [] }));
  await screen.findByText('Nothing under this agent in the period');
});

it('lists the prompts its runs named, and links only the ones the registry holds', async () => {
  // The name is retained telemetry and the registry is authored, so the two
  // can disagree in either direction: `retired-draft` ran and is no longer
  // registered. A link that 404s is worse than a fact somebody looks up.
  const { requests } = mount();
  await screen.findByText('Prompts it runs on');
  expect(await screen.findByRole('link', { name: 'extract-rooms' })).toBeTruthy();
  expect(screen.getByText('retired-draft').tagName).toBe('SPAN');
  expect(screen.getByText('3 of its runs named no prompt.')).toBeTruthy();
  // The dimension carries the page's whole filter, so this card is about the
  // same runs as the strip above it.
  expect(urlFor(requests, '/dimensions/prompt')?.searchParams.get('agent_id')).toBe('researcher');
});

it('renders a prompt as a fact when no registry answers at all', async () => {
  // 501 is the store saying it is not configured. An unreadable registry
  // resolves nothing, which is the same answer as a name it does not list.
  mount('', metrics(), { status: 501, body: { code: 'not_configured', message: 'no store' } });
  await screen.findByText('Prompts it runs on');
  expect(await screen.findByText('extract-rooms')).toBeTruthy();
  expect(screen.queryByRole('link', { name: 'extract-rooms' })).toBeNull();
});

it('sets its own figures beside the period before, and asks for it by the server’s boundary', async () => {
  const earlier = metrics();
  earlier.by_agent = [{ ...earlier.by_agent[0]!, runs: 2, llm_calls: 3, cost_usd: 0.21 }];
  earlier.window = { ...earlier.window, from: '2026-09-18T09:00:00Z', to: '2026-09-18T10:00:00Z' };
  const { requests } = mount('?window=3600&compare=previous', metrics(), REGISTERED, earlier);
  await screen.findByText(/Compared with/);
  const second = requests.mock.calls
    .map(([request]) => new URL((request as Request).url))
    .filter((url) => url.pathname.endsWith('/metrics'))[1]!;
  // The boundary is the one the first answer reported, one second earlier, so
  // the two halves are adjacent whatever this browser's clock says.
  expect(second.searchParams.get('as_of')).toBe(
    String(Date.parse('2026-09-18T10:00:00Z') / 1000 - 1),
  );
  // Four runs now against two then, and six calls against three: the figures
  // are the agent's own row in each period, never the totals beside them.
  expect(screen.getByText('was 2 · +100%')).toBeTruthy();
  expect(within(screen.getByText('Cost').parentElement!).getByText(/was \$0.21/)).toBeTruthy();
});

it('says once that an agent did not run in the period before', async () => {
  // Six "nothing reported before" lines would be six ways of saying one fact,
  // and the fact is about the agent rather than about any of the figures.
  const earlier = metrics({ by_agent: [] });
  mount('?window=3600&compare=previous', metrics(), REGISTERED, earlier);
  await screen.findByText(/This agent has no runs in it/);
  expect(screen.queryByText(/^was /)).toBeNull();
});
