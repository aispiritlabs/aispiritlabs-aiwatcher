import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { Link, getRouteApi } from '@tanstack/react-router';
import * as React from 'react';

import { getMetrics, listDimension, listRuns } from '@/api/generated/sdk.gen';
import type { DimensionKind, DimensionSummary, RunSummary } from '@/api/generated/types.gen';
import { RankedBars, type RankedRow } from '@/shared/components/charts/ranked-bars';
import { FilterNotes } from '@/shared/components/filter-notes';
import { StatusBadge } from '@/shared/components/status-badge';
import { DEFAULT_WINDOW_SECONDS, TimeRange, windowParam } from '@/shared/components/time-range';
import {
  Badge,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  EmptyState,
  Spinner,
  Stat,
} from '@/shared/components/ui/primitives';
import { VirtualList } from '@/shared/components/virtual-list';
import {
  AgainstPeriod,
  CompareToggle,
  endOfPeriodBefore,
  periodLabel,
} from '@/shared/components/period-compare';
import { PromptNameLink } from '@/shared/components/prompt-bits';
import {
  filterFromSearch,
  queryFor,
  type TargetQuery,
  type Unapplied,
} from '@/shared/lib/object-filter';
import {
  formatAge,
  formatCount,
  formatDuration,
  formatUsd,
  isStalled,
  shortId,
} from '@/shared/lib/utils';

const routeApi = getRouteApi('/agents/$agentId');
const RUNS_PAGE = 50;

/**
 * One agent.
 *
 * Three reads, and the division between them is the whole design of this page:
 *
 * - **What this agent did** is `by_agent` in `GET /api/v1/metrics`, keyed by
 *   the agent id on each *span*. Its calls, its tokens, its cost, its latency.
 * - **What happened around it** is the same response's `by_model`, `by_tool`
 *   and `by_step`, which are folded over every span of the matching runs — so a
 *   run where this agent hands work to another counts that other agent's model
 *   here too. The cards say so; a heading reading "its models" over numbers
 *   that are not only its would be the quiet kind of wrong.
 * - **Its runs** are `GET /api/v1/runs`, a cursor page like every other list
 *   that grows with retention.
 *
 * Nothing is computed here. Every figure is one the server folded, which is
 * also why there is no "average cost per run" anywhere on the page: a division
 * this file invented would be a number no other view could agree with.
 */
export function AgentPage() {
  const { agentId } = routeApi.useParams();
  const search = routeApi.useSearch();
  const navigate = routeApi.useNavigate();
  const windowSeconds = search.window ?? DEFAULT_WINDOW_SECONDS;

  // The path is the agent, so it wins over anything the URL's own axis says —
  // and says that it did, rather than quietly overriding a link.
  const filter = React.useMemo(
    () => ({ ...filterFromSearch(search), agent: [agentId] }),
    [search, agentId],
  );
  const contradicted: Unapplied[] = React.useMemo(() => {
    const named = (search.agent ?? []).filter((value) => value !== agentId);
    return named.length === 0
      ? []
      : [{ axis: 'agent' as const, why: `this page is about ${agentId}` }];
  }, [search.agent, agentId]);

  const metricsFilter = React.useMemo(() => queryFor('metrics', filter), [filter]);
  const runsFilter = React.useMemo(() => queryFor('runs', filter), [filter]);
  const dimensionFilter = React.useMemo(() => queryFor('dimensions', filter), [filter]);

  const metrics = useQuery({
    queryKey: ['metrics', 'agent', agentId, windowSeconds, metricsFilter.query],
    queryFn: async () => {
      const response = await getMetrics({
        query: {
          ...metricsFilter.query,
          window_seconds: windowParam(windowSeconds),
          buckets: 48,
        },
      });
      if (response.error) throw new Error('failed to load metrics');
      return response.data;
    },
  });

  const runs = useInfiniteQuery({
    queryKey: ['runs', 'agent', agentId, windowSeconds, runsFilter.query],
    initialPageParam: undefined as string | undefined,
    queryFn: async ({ pageParam }) => {
      const response = await listRuns({
        query: {
          ...runsFilter.query,
          before: pageParam,
          window_seconds: windowParam(windowSeconds),
          limit: RUNS_PAGE,
        },
      });
      if (!response.data) throw new Error('failed to list runs');
      return response.data;
    },
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });

  // The period before this one, ending where the server said this one begins —
  // `period-compare.tsx` for why it is derived from the answer rather than
  // from this clock.
  const comparing = search.compare === 'previous' && windowSeconds > 0;
  const endedAt = metrics.data ? endOfPeriodBefore(metrics.data.window.from) : undefined;
  const before = useQuery({
    queryKey: ['metrics', 'agent', 'before', agentId, windowSeconds, metricsFilter.query, endedAt],
    enabled: comparing && endedAt !== undefined,
    queryFn: async () => {
      const response = await getMetrics({
        query: {
          ...metricsFilter.query,
          window_seconds: windowParam(windowSeconds),
          as_of: endedAt,
          buckets: 48,
        },
      });
      if (response.error) throw new Error('failed to load the previous period');
      return response.data;
    },
  });

  const rows = React.useMemo(
    () => (runs.data?.pages ?? []).flatMap((page) => page.runs),
    [runs.data],
  );
  const mine = metrics.data?.by_agent.find((row) => row.agent_id === agentId);
  // Its own row in the period before, which an agent that ran only now has
  // none of — and that is a reading rather than a gap.
  const was = comparing ? before.data?.by_agent.find((row) => row.agent_id === agentId) : undefined;
  const seen = metrics.data !== undefined && (mine !== undefined || rows.length > 0);

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <p className="text-xs uppercase tracking-wide text-muted-foreground">Agent</p>
          <h1 className="break-all text-lg font-semibold">{agentId}</h1>
          <p className="text-sm text-muted-foreground">
            <Link to="/agents" search={{ window: search.window }} className="hover:underline">
              All agents
            </Link>
            {' · '}
            <Link
              to="/observability/explore"
              search={{ by: 'agent', key: agentId, window: search.window }}
              className="hover:underline"
            >
              Open in Explore
            </Link>
            {' · '}
            <Link
              to="/observability/live"
              search={{ agent: [agentId], window: search.window }}
              className="hover:underline"
            >
              Watch live
            </Link>
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <TimeRange
            value={windowSeconds}
            onChange={(seconds) =>
              void navigate({ search: (previous) => ({ ...previous, window: seconds }) })
            }
          />
          <CompareToggle
            on={search.compare === 'previous'}
            windowSeconds={windowSeconds}
            onChange={(on) =>
              void navigate({
                search: (previous) => ({ ...previous, compare: on ? 'previous' : undefined }),
              })
            }
          />
        </div>
      </div>

      <FilterNotes
        notes={runsFilter.notes}
        unapplied={[...contradicted, ...metricsFilter.unapplied]}
      />

      {comparing ? (
        <p className="text-xs text-muted-foreground">
          {before.isError ? (
            <span className="text-danger">Could not read the previous period.</span>
          ) : before.data ? (
            <>
              Compared with {periodLabel(before.data.window.from, before.data.window.to)}, on the
              same filter.{' '}
              {was
                ? 'The strip below carries the change; everything under it is this period only.'
                : 'This agent has no runs in it, so there is nothing to compare each figure with.'}
            </>
          ) : (
            'Reading the previous period…'
          )}
        </p>
      ) : null}

      {metrics.isError ? (
        <EmptyState
          title="Could not reach the API"
          hint="Is the aiwatcher server running? The panel proxies /api to it in development."
        />
      ) : metrics.isPending ? (
        <p className="flex items-center gap-2 text-sm text-muted-foreground">
          <Spinner /> Reading the period…
        </p>
      ) : !seen ? (
        // Never a 404: nothing here holds a record of an agent, so the only
        // honest answer is that the period has nothing under this name.
        <EmptyState
          title="Nothing under this agent in the period"
          hint="An agent is a key the runs contribute, not a thing this instance registers — so it appears when it runs and leaves with retention. Widen the period, or pick “all”."
        />
      ) : (
        <>
          <Card>
            <CardHeader>
              <CardTitle>What this agent did</CardTitle>
            </CardHeader>
            <CardContent className="grid grid-cols-2 gap-4 sm:grid-cols-3 lg:grid-cols-6">
              <Stat
                label="Runs"
                value={formatCount(mine?.runs ?? 0)}
                hint={`${formatCount(mine?.failures ?? 0)} failed`}
                compare={<Then now={mine?.runs} was={was?.runs} format={formatCount} />}
              />
              <Stat
                label="LLM calls"
                value={formatCount(mine?.llm_calls ?? 0)}
                compare={<Then now={mine?.llm_calls} was={was?.llm_calls} format={formatCount} />}
              />
              <Stat
                label="Tool calls"
                value={formatCount(mine?.tool_calls ?? 0)}
                compare={<Then now={mine?.tool_calls} was={was?.tool_calls} format={formatCount} />}
              />
              <Stat
                label="Tokens"
                value={formatCount((mine?.input_tokens ?? 0) + (mine?.output_tokens ?? 0))}
                hint={`${formatCount(mine?.input_tokens ?? 0)} in · ${formatCount(mine?.output_tokens ?? 0)} out`}
                compare={
                  <Then
                    now={mine && mine.input_tokens + mine.output_tokens}
                    was={was && was.input_tokens + was.output_tokens}
                    format={formatCount}
                  />
                }
              />
              <Stat
                label="LLM p95"
                value={
                  mine && mine.llm_latency.count > 0
                    ? formatDuration(mine.llm_latency.p95)
                    : 'No data'
                }
                hint={
                  mine && mine.llm_latency.count > 0
                    ? `p50 ${formatDuration(mine.llm_latency.p50)} · ${formatCount(mine.llm_latency.count)} calls`
                    : undefined
                }
                compare={
                  <Then
                    now={mine && mine.llm_latency.count > 0 ? mine.llm_latency.p95 : null}
                    was={was && was.llm_latency.count > 0 ? was.llm_latency.p95 : null}
                    format={formatDuration}
                  />
                }
              />
              {/* A dash, never $0: an agent whose calls reported nothing has an
                  unknown cost, not a free one. */}
              <Stat
                label="Cost"
                value={
                  mine?.cost_usd === undefined || mine?.cost_usd === null
                    ? '—'
                    : formatUsd(mine.cost_usd)
                }
                hint={
                  mine?.cost_usd === undefined || mine?.cost_usd === null
                    ? 'not reported'
                    : 'as the providers billed it'
                }
                compare={<Then now={mine?.cost_usd} was={was?.cost_usd} format={formatUsd} />}
              />
            </CardContent>
          </Card>

          <div className="grid gap-4 lg:grid-cols-2">
            <Card>
              <CardHeader>
                <CardTitle>Models in its runs</CardTitle>
              </CardHeader>
              <CardContent className="p-0">
                <RankedBars
                  rows={(metrics.data?.by_model ?? []).map(
                    (model): RankedRow => ({
                      key: model.model,
                      label: model.model,
                      value: model.input_tokens + model.output_tokens,
                      detail: `${model.calls} calls · p95 ${formatDuration(model.latency.p95)}${model.cost_usd != null ? ` · ${formatUsd(model.cost_usd)}` : ''}`,
                      warn: model.failures > 0,
                    }),
                  )}
                  formatValue={formatCount}
                  emptyMessage="No LLM call in its runs."
                />
              </CardContent>
            </Card>

            <Card>
              <CardHeader>
                <CardTitle>Tools in its runs</CardTitle>
              </CardHeader>
              <CardContent className="p-0">
                <RankedBars
                  rows={(metrics.data?.by_tool ?? []).map(
                    (tool): RankedRow => ({
                      key: tool.tool_name,
                      label: tool.tool_name,
                      value: tool.calls,
                      detail: `p95 ${formatDuration(tool.latency.p95)}${tool.failures > 0 ? ` · ${tool.failures} failed` : ''}`,
                      warn: tool.failures > 0,
                    }),
                  )}
                  emptyMessage="No tool call in its runs."
                />
              </CardContent>
            </Card>
          </div>

          <p className="text-xs text-muted-foreground">
            Both cards are folded over every span of the runs this agent took part in, so a run
            where it hands work to another agent counts that agent’s calls here too. What is this
            agent’s own is the strip above.
          </p>

          <div className="grid gap-4 lg:grid-cols-2">
            <Where
              kind="workflow"
              title="Workflows it appears in"
              query={dimensionFilter.query}
              window={windowSeconds}
            />
            <Where
              kind="runtime"
              title="Runtimes that produced it"
              query={dimensionFilter.query}
              window={windowSeconds}
            />
          </div>

          <Prompts query={dimensionFilter.query} window={windowSeconds} />

          <Card className="overflow-hidden">
            <CardHeader>
              <CardTitle>
                Runs{' '}
                <span className="font-normal text-muted-foreground">
                  {runs.data
                    ? `· ${formatCount(rows.length)} loaded of ${formatCount(runs.data.pages[0]?.total_known ?? rows.length)}`
                    : ''}
                </span>
              </CardTitle>
            </CardHeader>
            {rows.length === 0 && !runs.isPending ? (
              <CardContent>
                <p className="text-sm text-muted-foreground">
                  No run in the period matches this filter.
                </p>
              </CardContent>
            ) : (
              <VirtualList
                items={rows}
                estimateSize={56}
                className="max-h-[32rem]"
                keyOf={(run) => run.run_id}
                onReachEnd={() => {
                  if (runs.hasNextPage && !runs.isFetchingNextPage) void runs.fetchNextPage();
                }}
                isFetchingMore={runs.isFetchingNextPage}
                renderRow={(run) => <RunRow run={run} />}
              />
            )}
          </Card>
        </>
      )}
    </div>
  );
}

/**
 * One figure against the period before, or nothing at all.
 *
 * The strip is rendered whether or not a second period was asked for, so the
 * wrapper is what keeps six `comparing ? … : undefined` out of the markup. An
 * agent with no row in the period before has no figure to sit beside any of
 * them, and that is said once above the strip rather than six times inside it.
 */
function Then({
  now,
  was,
  format,
}: {
  now: number | null | undefined;
  was: number | null | undefined;
  format: (value: number) => string;
}) {
  if (was === undefined) return null;
  return <AgainstPeriod now={now} before={was} format={format} />;
}

/**
 * One dimension's rows, over the runs this page is about.
 *
 * The whole filter rather than the agent alone: the dimension route takes
 * every axis the runs list does, so a reader who arrived under a workflow sees
 * the same population in this card as in the strip above and the list below.
 */
function Where({
  kind,
  title,
  query,
  window,
}: {
  kind: Extract<DimensionKind, 'workflow' | 'runtime'>;
  title: string;
  query: TargetQuery['dimensions'];
  window: number;
}) {
  const rows = useQuery({
    queryKey: ['dimensions', kind, { query, window }],
    queryFn: async () => {
      const response = await listDimension({
        path: { kind },
        query: { ...query, window_seconds: windowParam(window), limit: 20 },
      });
      if (response.error) throw new Error(`failed to load ${kind}s`);
      return response.data;
    },
  });

  return (
    <Card>
      <CardHeader>
        <CardTitle>{title}</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-1 text-sm">
        {rows.isPending ? (
          <p className="flex items-center gap-2 text-xs text-muted-foreground">
            <Spinner /> Reading…
          </p>
        ) : rows.isError ? (
          <p className="text-xs text-danger">Could not read the {kind} list.</p>
        ) : (rows.data?.rows.length ?? 0) === 0 ? (
          <p className="text-xs text-muted-foreground">
            No run of this agent in the period names a {kind}.
          </p>
        ) : (
          rows.data?.rows.map((row: DimensionSummary) => (
            <Link
              key={row.key}
              to="/observability/explore"
              search={{ by: kind, key: row.key, window }}
              className="flex items-baseline justify-between gap-3 rounded px-1 py-0.5 hover:bg-accent/40"
            >
              <span className="truncate" title={row.key}>
                {shortId(row.key, 32)}
              </span>
              <span className="shrink-0 text-xs tabular-nums text-muted-foreground">
                {formatCount(row.runs)} runs
              </span>
            </Link>
          ))
        )}
        {rows.data?.next_cursor ? (
          <p className="text-[11px] text-muted-foreground">
            {formatCount(rows.data.total)} in the period; the first twenty are shown.
          </p>
        ) : null}
      </CardContent>
    </Card>
  );
}

/**
 * The prompts this agent's runs ran on.
 *
 * The card that used to say this was not answerable. It was true when it was
 * written: a call carried `aiwatcher.prompt.name` and nothing grouped by it,
 * so the only way to the list was paging every run and counting versions in
 * the browser — which is the one thing the panel may not do. The span row
 * lifts the reference now and `prompt` is a dimension, so the answer is one
 * read of the same shape as the two cards above it.
 *
 * The name, never the version: the registry is keyed by name and a version is
 * what a prompt's own page lists, so grouping by version would put a row under
 * every edit of one prompt. A call that sent only a version id contributes no
 * key and its run is counted in the dimension's `ungrouped_runs`.
 */
function Prompts({ query, window }: { query: TargetQuery['dimensions']; window: number }) {
  const rows = useQuery({
    queryKey: ['dimensions', 'prompt', { query, window }],
    queryFn: async () => {
      const response = await listDimension({
        path: { kind: 'prompt' },
        query: { ...query, window_seconds: windowParam(window), limit: 20 },
      });
      if (response.error) throw new Error('failed to load prompts');
      return response.data;
    },
  });

  return (
    <Card>
      <CardHeader>
        <CardTitle>Prompts it runs on</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-1 text-sm">
        {rows.isPending ? (
          <p className="flex items-center gap-2 text-xs text-muted-foreground">
            <Spinner /> Reading…
          </p>
        ) : rows.isError ? (
          <p className="text-xs text-danger">Could not read the prompt list.</p>
        ) : (rows.data?.rows.length ?? 0) === 0 ? (
          <p className="text-xs leading-relaxed text-muted-foreground">
            No call in these runs named a registered prompt. A call carries the prompt as a
            reference and many producers send none — the text itself is never on the log (ADR_0011).
          </p>
        ) : (
          rows.data?.rows.map((row: DimensionSummary) => (
            <div
              key={row.key}
              className="flex items-baseline justify-between gap-3 rounded px-1 py-0.5"
            >
              {/* Two links, because they answer two questions: the name goes to
                  the text the registry holds, the count to the runs that ran
                  on it. Nested anchors would be neither. */}
              <span className="truncate" title={row.key}>
                <PromptNameLink name={row.key} />
              </span>
              <Link
                to="/observability/explore"
                search={{ by: 'prompt', key: row.key, window }}
                className="shrink-0 text-xs tabular-nums text-muted-foreground hover:underline"
              >
                {formatCount(row.runs)} runs
              </Link>
            </div>
          ))
        )}
        {rows.data && rows.data.ungrouped_runs > 0 ? (
          <p className="text-[11px] text-muted-foreground">
            {formatCount(rows.data.ungrouped_runs)} of its runs named no prompt.
          </p>
        ) : null}
      </CardContent>
    </Card>
  );
}

function RunRow({ run }: { run: RunSummary }) {
  return (
    <Link
      to="/runs/$runId"
      params={{ runId: run.run_id }}
      className="flex items-center gap-3 border-b border-border/40 px-3 py-2 text-sm last:border-b-0 hover:bg-accent/40"
    >
      <StatusBadge status={run.status} lastEventAt={run.last_event_at} />
      <span className="min-w-0 flex-1 truncate font-medium" title={run.run_id}>
        {run.run_id}
      </span>
      {run.workflow ? (
        <Badge tone="neutral" className="hidden shrink-0 sm:inline-flex">
          {shortId(run.workflow, 20)}
        </Badge>
      ) : null}
      <span className="hidden w-24 shrink-0 text-right text-xs tabular-nums text-muted-foreground lg:block">
        {run.llm_calls} llm · {run.tool_calls} tool
      </span>
      <span className="w-20 shrink-0 text-right tabular-nums">
        {formatDuration(run.duration_ms)}
      </span>
      <span className="hidden w-24 shrink-0 text-right text-xs tabular-nums text-muted-foreground sm:block">
        {isStalled(run.last_event_at) && run.status === 'running'
          ? `quiet ${formatAge(run.last_event_at)}`
          : `${formatAge(run.last_event_at)} ago`}
      </span>
    </Link>
  );
}
