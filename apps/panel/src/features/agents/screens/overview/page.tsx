import { useInfiniteQuery } from '@tanstack/react-query';
import { Link, getRouteApi } from '@tanstack/react-router';
import { Search } from 'lucide-react';
import * as React from 'react';

import { listDimension } from '@/api/generated/sdk.gen';
import type { DimensionSummary } from '@/api/generated/types.gen';
import { DEFAULT_WINDOW_SECONDS, TimeRange, windowParam } from '@/shared/components/time-range';
import { Badge, Card, EmptyState, Spinner } from '@/shared/components/ui/primitives';
import { VirtualList } from '@/shared/components/virtual-list';
import { formatAge, formatCount, formatUsd, isStalled, shortId } from '@/shared/lib/utils';

const routeApi = getRouteApi('/agents/');
const PAGE = 100;

/**
 * Every agent the period holds, as the `agent` dimension's own rows.
 *
 * One read, `GET /api/v1/dimensions/agent`, and every number on a row is that
 * fold's. Nothing is summed here: the row already carries runs, outcomes,
 * calls, tokens and cost, and a second arithmetic in the browser would be a
 * second answer to what an agent did.
 */
export function AgentsPage() {
  const search = routeApi.useSearch();
  const navigate = routeApi.useNavigate();
  const windowSeconds = search.window ?? DEFAULT_WINDOW_SECONDS;
  const find = search.find ?? '';
  const [draft, setDraft] = React.useState(find);

  // The URL is the state; the input is a draft of it, committed on a debounce.
  React.useEffect(() => setDraft(find), [find]);
  React.useEffect(() => {
    if (draft === find) return;
    const timer = setTimeout(
      () =>
        void navigate({
          search: (previous) => ({ ...previous, find: draft || undefined }),
        }),
      250,
    );
    return () => clearTimeout(timer);
  }, [draft, find, navigate]);

  const agents = useInfiniteQuery({
    queryKey: ['dimensions', 'agent', find, windowSeconds],
    initialPageParam: undefined as string | undefined,
    queryFn: async ({ pageParam }) => {
      const response = await listDimension({
        path: { kind: 'agent' },
        query: {
          search: find || undefined,
          window_seconds: windowParam(windowSeconds),
          after: pageParam,
          limit: PAGE,
        },
      });
      if (response.error) throw new Error('failed to load agents');
      return response.data;
    },
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });

  const rows = React.useMemo(
    () => (agents.data?.pages ?? []).flatMap((page) => page.rows),
    [agents.data],
  );
  const total = agents.data?.pages[0]?.total;
  const ungrouped = agents.data?.pages[0]?.ungrouped_runs ?? 0;

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold">Agents</h1>
          <p className="text-sm text-muted-foreground">
            {agents.isPending
              ? 'loading…'
              : `${formatCount(rows.length)} shown${total === undefined ? '' : ` · ${formatCount(total)} in the period`}`}
          </p>
          <p className="text-xs text-muted-foreground">
            An agent is what the runs named, not a thing this instance holds a record of, so this
            list is bounded by retention like the runs it is folded from.
          </p>
        </div>
        <TimeRange
          value={windowSeconds}
          onChange={(seconds) =>
            void navigate({ search: (previous) => ({ ...previous, window: seconds }) })
          }
        />
      </div>

      <div className="flex items-center gap-1.5 rounded-md border border-border px-2 py-1">
        <Search className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        <input
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          placeholder="Find an agent…"
          aria-label="Find an agent"
          className="w-full bg-transparent text-sm outline-none placeholder:text-muted-foreground"
        />
      </div>

      {agents.isError ? (
        <EmptyState
          title="Could not reach the API"
          hint="Is the aiwatcher server running? The panel proxies /api to it in development."
        />
      ) : rows.length === 0 && !agents.isPending ? (
        <EmptyState
          title={find ? `No agent matching “${find}”` : 'No agent in this window'}
          hint={
            find
              ? 'The search is over the agent’s name, on the server. Widen the period, or clear it.'
              : 'A run that names an agent puts it here. Widen the period, or pick “all”.'
          }
        />
      ) : (
        <Card className="overflow-hidden">
          <VirtualList
            items={rows}
            estimateSize={64}
            className="max-h-[36rem]"
            keyOf={(row) => row.key}
            onReachEnd={() => {
              if (agents.hasNextPage && !agents.isFetchingNextPage) void agents.fetchNextPage();
            }}
            isFetchingMore={agents.isFetchingNextPage}
            renderRow={(row) => <AgentRow row={row} window={search.window} />}
          />
        </Card>
      )}

      {/* The fold's own footer, and it is worth showing: a runs list that holds
          more than this tree does would otherwise look like a bug. */}
      {ungrouped > 0 ? (
        <p className="text-xs text-muted-foreground">
          {formatCount(ungrouped)} run{ungrouped === 1 ? '' : 's'} in the period named no agent, so
          {ungrouped === 1 ? ' it has' : ' they have'} no row here.
        </p>
      ) : null}

      {agents.isFetching && !agents.isFetchingNextPage ? (
        <p className="flex items-center gap-2 text-xs text-muted-foreground">
          <Spinner /> Reading the period…
        </p>
      ) : null}
    </div>
  );
}

function AgentRow({ row, window }: { row: DimensionSummary; window: number | undefined }) {
  // Working, not merely "some run has no end event": a killed producer keeps
  // the running count above zero for ever, and a permanent green dot is worse
  // than none. The same fifteen minutes the explorer and the assembler use.
  const working = row.running > 0 && !isStalled(row.running_last_event_at);

  return (
    <Link
      to="/agents/$agentId"
      params={{ agentId: row.key }}
      search={{ window }}
      className="flex items-center gap-3 border-b border-border/40 px-3 py-2 text-sm last:border-b-0 hover:bg-accent/40"
    >
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="flex items-center gap-2">
          <span className="truncate font-medium" title={row.key}>
            {shortId(row.key, 40)}
          </span>
          {working ? (
            <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-success" title="Working now" />
          ) : null}
        </span>
        <span className="text-xs text-muted-foreground">
          last event {formatAge(row.last_activity_at)} ago
        </span>
      </span>

      <span className="hidden shrink-0 items-center gap-1 sm:flex">
        {row.failed > 0 ? <Badge tone="danger">{row.failed} failed</Badge> : null}
        {row.running > 0 ? <Badge tone="running">{row.running} running</Badge> : null}
        <Badge tone="neutral">{formatCount(row.runs)} runs</Badge>
      </span>

      <span className="hidden w-28 shrink-0 text-right text-xs tabular-nums text-muted-foreground lg:block">
        {formatCount(row.llm_calls)} llm · {formatCount(row.tool_calls)} tool
      </span>
      <span className="w-20 shrink-0 text-right tabular-nums">
        {row.cost_usd === undefined || row.cost_usd === null ? (
          <span className="text-muted-foreground" title="No call reported a cost">
            —
          </span>
        ) : (
          formatUsd(row.cost_usd)
        )}
      </span>
    </Link>
  );
}
