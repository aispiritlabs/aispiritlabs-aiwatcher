import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import {
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
  RouterProvider,
} from '@tanstack/react-router';
import { afterEach, expect, it, vi } from 'vitest';

import type { MetricsSummary } from '@/api/generated/types.gen';
import { serve, withQueries } from '@/test/server';
import { MetricsPage } from './page';
import { searchSchema } from './search';

afterEach(() => vi.unstubAllGlobals());

function metrics(succeeded = 0, failed = 0, running = 0): MetricsSummary {
  const empty = { count: 0, p50: 0, p95: 0, p99: 0 };
  const runs = succeeded + failed + running;
  return {
    window: {
      from: '2026-09-14T10:00:00Z',
      to: '2026-09-14T11:00:00Z',
      runs_considered: runs,
      runs_retained: runs,
      retention_limit: 5000,
    },
    totals: {
      runs,
      succeeded,
      failed,
      running,
      llm_calls: 0,
      tool_calls: 0,
      step_calls: 0,
      input_tokens: 0,
      output_tokens: 0,
      cached_tokens: 0,
      cache_hit_ratio: 0, costed_calls: 0,
    },
    latency: { run: empty, llm: empty, tool: empty, step: empty, time_to_first_token: empty },
    timeline: [
      {
        at: '2026-09-14T10:00:00Z',
        runs,
        succeeded,
        failed,
        running,
        llm_calls: 0,
        tool_calls: 0,
        input_tokens: 0,
        output_tokens: 0,
        cached_tokens: 0,
      },
    ],
    by_agent: [],
    by_model: [],
    by_tool: [],
    by_step: [],
  };
}

function mount(body: unknown, status = 200, search = '', second?: unknown) {
  vi.stubGlobal('scrollTo', () => {});
  serve([
    {
      method: 'GET',
      path: '/metrics',
      // The comparison asks the same route twice, and what makes it a
      // comparison is that the second answer is a different period.
      answer: (call) =>
        call > 1 && second !== undefined ? { status: 200, body: second } : { status, body },
    },
  ]);
  const requests = vi.fn(fetch);
  vi.stubGlobal('fetch', requests);
  const root = createRootRoute();
  const route = createRoute({
    getParentRoute: () => root,
    path: '/observability/metrics',
    validateSearch: searchSchema,
    component: MetricsPage,
  });
  const router = createRouter({
    routeTree: root.addChildren([route]),
    history: createMemoryHistory({ initialEntries: [`/observability/metrics${search}`] }),
  });
  render(withQueries(<RouterProvider router={router} />));
  return { requests, router };
}

function tile(label: string) {
  return within(screen.getByText(label).parentElement!);
}

it.each([0, 4])('shows neutral no data with %i running and no completed runs', async (running) => {
  mount(metrics(0, 0, running));
  await screen.findByRole('heading', { name: 'Metrics' });
  const value = tile('Success rate').getByText('No data');
  expect(value.style.color).toBe('');
  expect(tile('Success rate').getByText('0 completed · 0 failed')).toBeTruthy();
  expect(tile('LLM p95').getByText('No data')).toBeTruthy();
  expect(tile('Tokens').getByText('No data')).toBeTruthy();
  expect(screen.queryByText('0ms')).toBeNull();
  if (running === 0) expect(screen.getByText('No runs in this window.')).toBeTruthy();
  else
    expect(
      screen.getByRole('img', { name: 'Stacked bars: succeeded, running, failed' }),
    ).toBeTruthy();
});

it.each([
  [3, 1, 8, '75%'],
  [2, 0, 8, '100%'],
  [0, 2, 8, '0%'],
] as const)(
  'counts success among completed runs (%i succeeded, %i failed, %i running)',
  async (succeeded, failed, running, expected) => {
    mount(metrics(succeeded, failed, running));
    await screen.findByRole('heading', { name: 'Metrics' });
    expect(tile('Success rate').getByText(expected)).toBeTruthy();
    const chart = screen.getByRole('img', { name: 'Stacked bars: succeeded, running, failed' });
    fireEvent.mouseMove(chart.querySelector('rect[fill="transparent"]')!);
    const tooltip = within(chart.parentElement!);
    expect(tooltip.getByText('running').parentElement?.textContent).toContain(String(running));
    expect(tooltip.getByText('succeeded').parentElement?.textContent).toContain(String(succeeded));
    expect(tooltip.getByText('failed').parentElement?.textContent).toContain(String(failed));
  },
);

it('keeps an honest combined series when an older API omits status counters', async () => {
  const body = metrics(2, 1, 5);
  const { succeeded: _succeeded, running: _running, ...bucket } = body.timeline[0]!;
  mount({ ...body, timeline: [bucket] });
  await screen.findByText('This API does not separate running and succeeded runs in the timeline.');
  expect(
    screen.getByRole('img', { name: 'Stacked bars: running or succeeded, failed' }),
  ).toBeTruthy();
  expect(tile('Success rate').getByText('67%')).toBeTruthy();
});

it('does not count cached input twice in the token stack', async () => {
  const body = metrics(1);
  const usage = { llm_calls: 1, input_tokens: 800, output_tokens: 200, cached_tokens: 400 };
  Object.assign(body.totals, usage, { cache_hit_ratio: 0.5 });
  Object.assign(body.timeline[0]!, usage);
  mount(body);
  const chart = await screen.findByRole('img', {
    name: 'Stacked bars: input (uncached), output, cached',
  });
  fireEvent.mouseMove(chart.querySelector('rect[fill="transparent"]')!);
  const total = within(chart.parentElement!).getByText('total').parentElement!;
  expect(total.textContent).toBe('total1.0k');
  expect(tile('Cache hit').getByText('50%')).toBeTruthy();
});

it('preserves agent and model filters when the window changes and explains their scope', async () => {
  // The axes are the shared vocabulary's — `agent`, not the route's
  // `agent_id` — and the translation to the route's parameters happens in
  // `shared/lib/object-filter.ts`. What this still proves is UX-02's property:
  // changing the period keeps the filter, and the page says what the model
  // filter does to the numbers beside it — which is now that it selects the
  // runs and narrows the call counters within them.
  const { requests, router } = mount(metrics(), 200, '?agent=researcher&model=opus&window=3600');
  await screen.findByText(/LLM calls, tokens, cost and LLM latency are that model’s/);
  // The filter bar renders through the loading state on purpose — taking a
  // chip off is the way out of "nothing matched" — so wait for the page.
  await screen.findByRole('heading', { name: 'Metrics' });
  await userEvent.click(screen.getByRole('button', { name: '15m' }));
  await waitFor(() => expect(requests).toHaveBeenCalledTimes(2));
  const url = new URL((requests.mock.calls[1]![0] as Request).url);
  expect(url.searchParams.get('window_seconds')).toBe('900');
  expect(url.searchParams.get('agent_id')).toBe('researcher');
  expect(url.searchParams.get('model')).toBe('opus');
  expect(router.state.location.search).toMatchObject({
    agent: ['researcher'],
    model: ['opus'],
    window: 900,
  });
});

it('carries the axes it used to name as unapplied', async () => {
  // The regression this holds: a workflow filter arriving from Explore was on
  // the screen and not on the request, and the page said so. The route takes
  // both now, so saying so would be the lie the warning existed to prevent.
  const { requests } = mount(metrics(), 200, '?workflow=import&tool=search&prompt=extract');
  await screen.findByRole('heading', { name: 'Metrics' });
  expect(screen.queryByText(/Not applied here/)).toBeNull();
  const url = new URL((requests.mock.calls[0]![0] as Request).url);
  expect(url.searchParams.get('workflow')).toBe('import');
  expect(url.searchParams.get('tool')).toBe('search');
  expect(url.searchParams.get('prompt')).toBe('extract');
});

it('still refuses to narrow to the first of two values on one axis', async () => {
  // The rule survives the route growing parameters: this read takes one agent,
  // so two chips are reported rather than half-answered. Sending the first
  // would put a narrower answer on the screen than the chips claim.
  const { requests } = mount(metrics(), 200, '?agent=researcher&agent=planner');
  await screen.findByText(/Not applied here/);
  expect(screen.getByText(/narrows to one agent; 2 are chosen/)).toBeTruthy();
  const url = new URL((requests.mock.calls[0]![0] as Request).url);
  expect(url.searchParams.get('agent_id')).toBeNull();
});

it('shows an API failure instead of empty successful metrics', async () => {
  mount({ message: 'unavailable' }, 503);
  await screen.findByText('Could not reach the API');
  expect(screen.queryByText('Success rate')).toBeNull();
});

it('asks for the period before the one the server answered, not the one this clock says', async () => {
  // The whole design of the second read. A browser running a few minutes fast
  // would otherwise ask for a period overlapping the one beside it, and the
  // overlap would be invisible in the answer. One second earlier than the
  // window's own start, because both ends of a window are inclusive.
  const earlier = metrics(1);
  earlier.window = { ...earlier.window, from: '2026-09-14T09:00:00Z', to: '2026-09-14T10:00:00Z' };
  const { requests } = mount(metrics(4), 200, '?window=3600&compare=previous', earlier);
  await screen.findByText(/Compared with/);
  const second = new URL((requests.mock.calls[1]![0] as Request).url);
  expect(second.searchParams.get('as_of')).toBe(
    String(Date.parse('2026-09-14T10:00:00Z') / 1000 - 1),
  );
  expect(second.searchParams.get('window_seconds')).toBe('3600');
});

it('carries the same filter into both periods', async () => {
  const { requests } = mount(metrics(4), 200, '?window=3600&compare=previous&model=opus', metrics(1));
  await screen.findByText(/Compared with/);
  for (const call of [0, 1]) {
    const url = new URL((requests.mock.calls[call]![0] as Request).url);
    expect(url.searchParams.get('model')).toBe('opus');
  }
});

it('puts each figure beside the same figure in the period before', async () => {
  const { requests } = mount(metrics(3, 1), 200, '?window=3600&compare=previous', metrics(2, 2));
  await screen.findByText(/Compared with/);
  await waitFor(() => expect(requests).toHaveBeenCalledTimes(2));
  // Four runs then and four now, so the count is unchanged while the rate
  // moved — which is the pair a single "runs are up" headline would hide.
  expect(tile('Runs').getByText(/was 4/)).toBeTruthy();
  expect(tile('Runs').getByText(/unchanged/)).toBeTruthy();
  // Points, not per cent: 50% to 75% is twenty-five points.
  expect(tile('Success rate').getByText(/was 50% · \+25 pts/)).toBeTruthy();
});

it('says nothing was reported before rather than reading absence as nought', async () => {
  // The cost rule, in the comparison: a period whose calls reported no price
  // has an unknown cost, and "−100%" against it would be an invention.
  const now = metrics(2);
  now.totals.cost_usd = 4;
  now.totals.costed_calls = 2;
  mount(now, 200, '?window=3600&compare=previous', metrics(2));
  await screen.findByText(/Compared with/);
  expect(tile('Cost').getByText('nothing reported before')).toBeTruthy();
});

it('offers no previous period for a window of everything, and says why', async () => {
  const { requests } = mount(metrics(1), 200, '?window=0&compare=previous');
  await screen.findByRole('heading', { name: 'Metrics' });
  const toggle = screen.getByRole('button', { name: 'vs previous' });
  expect(toggle).toHaveProperty('disabled', true);
  expect(toggle.getAttribute('title')).toMatch(/no period before it/);
  // Asked once: there is no window to step back by, so there is no second
  // question to ask.
  await waitFor(() => expect(requests).toHaveBeenCalledTimes(1));
});

it('reports a failed second read rather than drawing one period as two', async () => {
  serve([
    {
      method: 'GET',
      path: '/metrics',
      answer: (call) =>
        call > 1
          ? { status: 503, body: { code: 'unavailable', message: 'gone' } }
          : { status: 200, body: metrics(2) },
    },
  ]);
  vi.stubGlobal('scrollTo', () => {});
  vi.stubGlobal('fetch', vi.fn(fetch));
  const root = createRootRoute();
  const route = createRoute({
    getParentRoute: () => root,
    path: '/observability/metrics',
    validateSearch: searchSchema,
    component: MetricsPage,
  });
  const router = createRouter({
    routeTree: root.addChildren([route]),
    history: createMemoryHistory({
      initialEntries: ['/observability/metrics?window=3600&compare=previous'],
    }),
  });
  render(withQueries(<RouterProvider router={router} />));
  await screen.findByText('Could not read the previous period.');
  expect(screen.queryByText(/^was /)).toBeNull();
});
