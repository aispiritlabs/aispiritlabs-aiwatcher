import * as React from 'react';
import { Link, createFileRoute } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { flexRender, getCoreRowModel, useReactTable, type ColumnDef } from '@tanstack/react-table';
import { z } from 'zod';

import { listRuns } from '@/api/generated/sdk.gen';
import type { RunStatus, RunSummary } from '@/api/generated/types.gen';
import { Button, Card, EmptyState, IdChip } from '@/components/ui/primitives';
import { StatusBadge } from '@/components/status-badge';
import {
  DEFAULT_WINDOW_SECONDS,
  TimeRange,
  windowParam,
  windowSearchSchema,
} from '@/components/time-range';
import { formatAge, formatCount, formatDuration, formatTime, shortId } from '@/lib/utils';

/**
 * Filters live in the URL, not in component state. A run that looks wrong is
 * something people paste into a chat, and a link that does not carry the filter
 * lands the reader somewhere else.
 */
const searchSchema = z.object({
  ...windowSearchSchema,
  status: z.enum(['running', 'succeeded', 'failed']).optional(),
  conversation_id: z.string().optional(),
  agent_id: z.string().optional(),
});

export const Route = createFileRoute('/observability/runs')({
  validateSearch: searchSchema,
  component: RunsPage,
});

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
];

function RunsPage() {
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const windowSeconds = search.window ?? DEFAULT_WINDOW_SECONDS;

  const query = useQuery({
    queryKey: ['runs', search, windowSeconds],
    queryFn: async () => {
      const response = await listRuns({
        query: {
          status: search.status,
          conversation_id: search.conversation_id,
          agent_id: search.agent_id,
          window_seconds: windowParam(windowSeconds),
        },
      });
      if (response.error) throw new Error('failed to list runs');
      return response.data;
    },
  });

  const runs = React.useMemo(() => query.data?.runs ?? [], [query.data]);
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
            {query.data ? `${query.data.total_known} matching` : 'loading…'}
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
              variant={search.status === status ? 'default' : 'outline'}
              onClick={() =>
                void navigate({
                  search: (previous) => ({
                    ...previous,
                    status: previous.status === status ? undefined : status,
                  }),
                })
              }
            >
              {status}
            </Button>
          ))}
        </div>
      </div>

      {query.isError ? (
        <EmptyState
          title="Could not reach the API"
          hint="Is the aiwatcher server running? The panel proxies /api to it in development."
        />
      ) : runs.length === 0 && !query.isLoading ? (
        <EmptyState
          title="No runs in this window"
          hint={
            windowSeconds
              ? 'Nothing was active in the selected period. Widen it, or pick “all”.'
              : 'Publish a run.started event and it will appear here.'
          }
        />
      ) : (
        <Card className="overflow-hidden">
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
    </div>
  );
}
