import { useInfiniteQuery } from '@tanstack/react-query';
import { Link, getRouteApi } from '@tanstack/react-router';
import { flexRender, getCoreRowModel, useReactTable, type ColumnDef } from '@tanstack/react-table';
import * as React from 'react';

import { listRuns } from '@/api/generated/sdk.gen';
import type { RunStatus, RunSummary } from '@/api/generated/types.gen';
import { ObjectFilterBar } from '@/features/observability/components/object-filter-bar';
import { StatusBadge } from '@/shared/components/status-badge';
import { DEFAULT_WINDOW_SECONDS, TimeRange, windowParam } from '@/shared/components/time-range';
import {
  filterFromSearch,
  filterToSearch,
  isEmpty,
  queryFor,
  toggle,
  type ObjectFilter,
} from '@/shared/lib/object-filter';
import { Button, Card, EmptyState, IdChip } from '@/shared/components/ui/primitives';
import {
  formatAge,
  formatCount,
  formatDuration,
  formatTime,
  formatUsd,
  shortId,
} from '@/shared/lib/utils';

const routeApi = getRouteApi('/observability/runs');

const columns: ColumnDef<RunSummary>[] = [
  {
    accessorKey: 'status',
    header: 'Status',
    // The run's newest event, so a run whose producer was killed reads as
    // stalled rather than as one still working. See `StatusBadge`.
    cell: ({ row }) => (
      <StatusBadge
        status={row.original.status as RunStatus}
        lastEventAt={row.original.last_event_at}
      />
    ),
  },
  {
    accessorKey: 'run_id',
    header: 'Run',
    cell: ({ row }) => (
      <Link
        to="/runs/$runId"
        params={{ runId: row.original.run_id }}
        className="font-medium text-primary hover:underline"
      >
        {row.original.run_id}
      </Link>
    ),
  },
  {
    accessorKey: 'agents',
    header: 'Agents',
    cell: ({ row }) =>
      row.original.agents.length > 0 ? (
        <span className="text-sm">{row.original.agents.join(', ')}</span>
      ) : (
        <span className="text-muted-foreground">—</span>
      ),
  },
  {
    accessorKey: 'trace_id',
    header: 'Trace',
    cell: ({ row }) => (
      <IdChip value={shortId(row.original.trace_id)} full={row.original.trace_id} label="trace" />
    ),
  },
  {
    accessorKey: 'started_at',
    header: 'Started',
    cell: ({ row }) => (
      <span className="text-xs tabular-nums text-muted-foreground">
        {formatTime(row.original.started_at)}
      </span>
    ),
  },
  {
    accessorKey: 'last_event_at',
    header: 'Last event',
    cell: ({ row }) => (
      <span className="text-xs tabular-nums text-muted-foreground">
        {formatAge(row.original.last_event_at)} ago
      </span>
    ),
  },
  {
    accessorKey: 'duration_ms',
    header: 'Duration',
    cell: ({ row }) => (
      <span className="tabular-nums">{formatDuration(row.original.duration_ms)}</span>
    ),
  },
  {
    id: 'calls',
    header: 'Calls',
    cell: ({ row }) => (
      <span className="tabular-nums text-muted-foreground">
        {row.original.llm_calls} llm · {row.original.tool_calls} tool
      </span>
    ),
  },
  {
    id: 'tokens',
    header: 'Tokens',
    cell: ({ row }) => (
      <span
        className="tabular-nums"
        title={`${row.original.input_tokens} in · ${row.original.output_tokens} out · ${row.original.cached_tokens} cached`}
      >
        {formatCount(row.original.input_tokens + row.original.output_tokens)}
        {row.original.cached_tokens > 0 ? (
          <span className="ml-1 text-xs text-muted-foreground">
            ({formatCount(row.original.cached_tokens)} cached)
          </span>
        ) : null}
      </span>
    ),
  },
  {
    id: 'cost',
    header: 'Cost',
    // What the providers billed, where they said so. A dash rather than $0:
    // a run whose calls reported nothing has an unknown cost, not a free one.
    cell: ({ row }) => {
      const { cost_usd: cost, costed_calls: costed = 0, llm_calls: calls } = row.original;
      if (cost === undefined || cost === null) {
        return <span className="text-muted-foreground">—</span>;
      }
      return (
        <span
          className="tabular-nums"
          title={
            costed < calls
              ? `${costed} of ${calls} calls reported a cost`
              : 'as the providers billed it'
          }
        >
          {formatUsd(cost)}
          {costed < calls ? (
            <span className="ml-1 text-xs text-muted-foreground">partial</span>
          ) : null}
        </span>
      );
    },
  },
];

export function RunsPage() {
  const search = routeApi.useSearch();
  const navigate = routeApi.useNavigate();
  const windowSeconds = search.window ?? DEFAULT_WINDOW_SECONDS;
  const filter = React.useMemo(() => filterFromSearch(search), [search]);
  // The runs route takes every axis, so `unapplied` is empty unless somebody
  // chose two values on one of them. It is still read from the same function
  // the other views use: a page that knew it could apply everything would be a
  // page that stopped saying so the day a second value became expressible.
  const translated = React.useMemo(() => queryFor('runs', filter), [filter]);
  const filterQuery = translated.query;

  const setFilter = React.useCallback(
    (next: ObjectFilter) =>
      void navigate({ search: (previous) => ({ ...previous, ...filterToSearch(next) }) }),
    [navigate],
  );

  const query = useInfiniteQuery({
    queryKey: ['runs', filterQuery, windowSeconds],
    initialPageParam: undefined as string | undefined,
    queryFn: async ({ pageParam }) => {
      const response = await listRuns({
        query: {
          ...filterQuery,
          before: pageParam,
          window_seconds: windowParam(windowSeconds),
        },
      });
      if (!response.data) throw new Error('failed to list runs');
      return response.data;
    },
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });

  const runs = React.useMemo(
    () => Array.from(new Map(
      (query.data?.pages.flatMap((page) => page.runs) ?? []).map((run) => [run.run_id, run]),
    ).values()),
    [query.data],
  );
  const table = useReactTable({
    data: runs,
    columns,
    getCoreRowModel: getCoreRowModel(),
  });

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold">Runs</h1>
          <p className="text-sm text-muted-foreground">
            {query.data ? `${runs.length} loaded · ${query.data.pages[0]?.total_known ?? runs.length} matching` : 'loading…'}
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <TimeRange
            value={windowSeconds}
            onChange={(seconds) =>
              void navigate({ search: (previous) => ({ ...previous, window: seconds }) })
            }
          />
          {(['running', 'succeeded', 'failed'] as const).map((status) => (
            <Button
              key={status}
              size="sm"
              aria-pressed={(filter.status ?? []).includes(status)}
              variant={(filter.status ?? []).includes(status) ? 'default' : 'outline'}
              onClick={() => setFilter(toggle(filter, 'status', status))}
            >
              {status}
            </Button>
          ))}
        </div>
      </div>

      <ObjectFilterBar
        filter={filter}
        onChange={setFilter}
        windowSeconds={windowSeconds}
        unapplied={translated.unapplied}
        notes={translated.notes}
        reading="this list is those runs."
      />

      {query.isError ? (
        <EmptyState
          title="Could not reach the API"
          hint="Is the aiwatcher server running? The panel proxies /api to it in development."
        />
      ) : runs.length === 0 && !query.isLoading ? (
        <EmptyState
          title={isEmpty(filter) ? 'No runs in this window' : 'No runs match this filter'}
          hint={
            !isEmpty(filter)
              ? 'Nothing in the period has every chosen attribute. Take one off, or widen the period.'
              : windowSeconds
                ? 'Nothing was active in the selected period. Widen it, or pick “all”.'
                : 'Publish a run.started event and it will appear here.'
          }
        />
      ) : (
        <Card className="overflow-x-auto">
          <table className="w-full text-left text-sm">
            <thead>
              {table.getHeaderGroups().map((group) => (
                <tr key={group.id} className="border-b border-border">
                  {group.headers.map((header) => (
                    <th
                      key={header.id}
                      className="px-3 py-2 text-xs font-medium uppercase tracking-wide text-muted-foreground"
                    >
                      {flexRender(header.column.columnDef.header, header.getContext())}
                    </th>
                  ))}
                </tr>
              ))}
            </thead>
            <tbody>
              {table.getRowModel().rows.map((row) => (
                <tr
                  key={row.id}
                  className="border-b border-border/40 last:border-b-0 hover:bg-accent/40"
                >
                  {row.getVisibleCells().map((cell) => (
                    <td key={cell.id} className="px-3 py-2">
                      {flexRender(cell.column.columnDef.cell, cell.getContext())}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </Card>
      )}
      {query.hasNextPage && (
        <Button variant="outline" disabled={query.isFetchingNextPage} onClick={() => void query.fetchNextPage()}>
          {query.isFetchingNextPage ? 'Loading…' : 'Load more runs'}
        </Button>
      )}
    </div>
  );
}
