import * as React from 'react';
import { Link } from '@tanstack/react-router';
import {
  useInfiniteQuery,
  type InfiniteData,
  type UseInfiniteQueryResult,
} from '@tanstack/react-query';
import {
  flexRender,
  getCoreRowModel,
  useReactTable,
  type ColumnDef,
} from '@tanstack/react-table';
import { Beaker, GitBranch, Rows3, Search } from 'lucide-react';

import { getDatasetRows, listEvaluations } from '@/api/generated/sdk.gen';
import type {
  DatasetRow,
  DatasetRowsPage,
  DatasetSummary,
  EvaluationSummary,
} from '@/api/generated/types.gen';
import { Badge, Button, Card, EmptyState, Spinner } from '@/components/ui/primitives';
import { StatusBadge } from '@/components/status-badge';
import { cn, formatTime } from '@/lib/utils';

export type DatasetView = 'rows' | 'evaluations' | 'lineage';

const ROW_PAGE = 50;
const EVALUATION_PAGE = 25;

export function DatasetExplorer({
  dataset,
  versionId,
  view,
  search,
  onVersionChange,
  onViewChange,
  onSearchChange,
}: {
  dataset: DatasetSummary;
  versionId: string;
  view: DatasetView;
  search?: string;
  onVersionChange: (version: string) => void;
  onViewChange: (view: DatasetView) => void;
  onSearchChange: (search: string | undefined) => void;
}) {
  const version =
    dataset.versions.find((candidate) => candidate.version === versionId) ?? dataset.latest;
  const reference = `${dataset.name}@${version.version}`;
  const [draft, setDraft] = React.useState(search ?? '');

  React.useEffect(() => setDraft(search ?? ''), [search]);
  React.useEffect(() => {
    const timeout = window.setTimeout(() => {
      const value = draft.trim() || undefined;
      if (value !== search) onSearchChange(value);
    }, 250);
    return () => window.clearTimeout(timeout);
  }, [draft, onSearchChange, search]);

  const rows = useInfiniteQuery({
    queryKey: ['dataset-rows', dataset.name, version.version, search],
    initialPageParam: 0,
    enabled: view === 'rows' || view === 'lineage',
    queryFn: async ({ pageParam }) => {
      const response = await getDatasetRows({
        query: {
          name: dataset.name,
          version: version.version,
          offset: pageParam,
          limit: ROW_PAGE,
          search,
        },
      });
      if (!response.data) throw apiError(response.error, 'Could not load dataset rows.');
      return response.data;
    },
    getNextPageParam: (last) => last.next_offset ?? undefined,
  });

  const relationEnabled = view === 'evaluations' || view === 'lineage';
  const versionedEvaluations = useDatasetEvaluations(reference, relationEnabled);
  const legacyEvaluations = useDatasetEvaluations(dataset.name, relationEnabled);
  const evaluations = React.useMemo(() => {
    const byId = new Map<string, EvaluationSummary>();
    for (const report of [
      ...flattenEvaluations(versionedEvaluations.data?.pages),
      ...flattenEvaluations(legacyEvaluations.data?.pages),
    ]) {
      byId.set(report.evaluation_id, report);
    }
    return [...byId.values()].sort((left, right) => right.started_at.localeCompare(left.started_at));
  }, [legacyEvaluations.data, versionedEvaluations.data]);
  const evaluationTotal =
    (versionedEvaluations.data?.pages[0]?.total_known ?? 0) +
    (legacyEvaluations.data?.pages[0]?.total_known ?? 0);

  const loadMoreEvaluations = React.useCallback(() => {
    if (versionedEvaluations.hasNextPage && !versionedEvaluations.isFetchingNextPage) {
      void versionedEvaluations.fetchNextPage();
    }
    if (legacyEvaluations.hasNextPage && !legacyEvaluations.isFetchingNextPage) {
      void legacyEvaluations.fetchNextPage();
    }
  }, [legacyEvaluations, versionedEvaluations]);

  return (
    <div className="flex min-w-0 flex-col gap-4">
      <Card className="overflow-hidden">
        <div className="flex flex-wrap items-start justify-between gap-3 border-b border-border p-4">
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-2">
              <h2 className="truncate text-base font-semibold">{dataset.name}</h2>
              <Badge>{version.row_count} rows</Badge>
              <Badge>{version.columns.length} columns</Badge>
            </div>
            {dataset.description ? (
              <p className="mt-1 max-w-3xl text-xs text-muted-foreground">{dataset.description}</p>
            ) : null}
          </div>
          <label className="flex items-center gap-2 text-xs text-muted-foreground">
            Version
            <select
              value={version.version}
              onChange={(event) => onVersionChange(event.target.value)}
              className="h-8 max-w-56 rounded-md border border-border bg-card px-2 text-card-foreground outline-none focus-visible:ring-2 focus-visible:ring-primary"
            >
              {dataset.versions.map((candidate) => (
                <option key={candidate.version} value={candidate.version}>
                  {candidate.version.slice(0, 12)} · {new Date(candidate.created_at).toLocaleString()}
                </option>
              ))}
            </select>
          </label>
        </div>
        <div className="flex flex-wrap items-center justify-between gap-2 px-3 py-2">
          <div className="flex gap-1">
            <ViewButton active={view === 'rows'} onClick={() => onViewChange('rows')} icon={Rows3}>
              Rows
            </ViewButton>
            <ViewButton active={view === 'evaluations'} onClick={() => onViewChange('evaluations')} icon={Beaker}>
              Evaluations {evaluationTotal > 0 ? `(${evaluationTotal})` : ''}
            </ViewButton>
            <ViewButton active={view === 'lineage'} onClick={() => onViewChange('lineage')} icon={GitBranch}>
              Lineage
            </ViewButton>
          </div>
          <code className="id max-w-full truncate text-muted-foreground" title={reference}>
            {dataset.name}@{version.version.slice(0, 12)}
          </code>
        </div>
      </Card>

      {view === 'rows' ? (
        <RowsViewer
          query={rows}
          columns={version.columns}
          draft={draft}
          onDraftChange={setDraft}
        />
      ) : null}
      {view === 'evaluations' ? (
        <EvaluationsViewer
          evaluations={evaluations}
          total={evaluationTotal}
          reference={reference}
          loading={versionedEvaluations.isLoading || legacyEvaluations.isLoading}
          fetchingMore={
            versionedEvaluations.isFetchingNextPage || legacyEvaluations.isFetchingNextPage
          }
          hasMore={versionedEvaluations.hasNextPage || legacyEvaluations.hasNextPage}
          onLoadMore={loadMoreEvaluations}
        />
      ) : null}
      {view === 'lineage' ? (
        <Lineage
          dataset={dataset}
          reference={reference}
          evaluations={evaluations}
          pipeline={rows.data?.pages[0]?.pipeline}
          source={rows.data?.pages[0]?.source}
          windowSeconds={rows.data?.pages[0]?.window_seconds}
          versionId={version.version}
        />
      ) : null}
    </div>
  );
}

function RowsViewer({
  query,
  columns,
  draft,
  onDraftChange,
}: {
  query: UseInfiniteQueryResult<InfiniteData<DatasetRowsPage, unknown>, Error>;
  columns: string[];
  draft: string;
  onDraftChange: (value: string) => void;
}) {
  const data = React.useMemo(
    () => query.data?.pages.flatMap((page) => page.rows) ?? [],
    [query.data],
  );
  const page = query.data?.pages[0];
  const [selected, setSelected] = React.useState<DatasetRow>();
  React.useEffect(() => setSelected(undefined), [page?.version.version, draft]);

  const definitions = React.useMemo<ColumnDef<DatasetRow>[]>(
    () => [
      {
        id: '__row',
        header: '#',
        cell: ({ row }) => (
          <span className="id text-muted-foreground">{row.original.row_index}</span>
        ),
      },
      ...columns.map(
        (column): ColumnDef<DatasetRow> => ({
          id: column,
          accessorFn: (row) => row.row[column],
          header: () => (
            <div className="flex flex-col">
              <span>{column}</span>
              <span className="font-normal text-muted-foreground">
                {inferColumnType(data, column)}
              </span>
            </div>
          ),
          cell: ({ getValue }) => <DatasetCell value={getValue()} />,
        }),
      ),
    ],
    [columns, data],
  );
  const table = useReactTable({ data, columns: definitions, getCoreRowModel: getCoreRowModel() });

  return (
    <Card className="overflow-hidden">
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border p-3">
        <label className="relative min-w-64 flex-1 md:max-w-md">
          <Search className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
          <input
            value={draft}
            onChange={(event) => onDraftChange(event.target.value)}
            placeholder="Search every value in this version"
            className="h-9 w-full rounded-md border border-border bg-background pl-8 pr-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-primary"
          />
        </label>
        <span className="text-xs text-muted-foreground">
          {page
            ? `${page.matching_rows} matching · ${data.length} loaded of ${page.total_rows}`
            : 'loading schema and first rows…'}
        </span>
      </div>

      {query.isError ? (
        <EmptyState title="Could not load this version" hint={query.error.message} />
      ) : query.isLoading ? (
        <p className="flex items-center justify-center gap-2 p-10 text-sm text-muted-foreground">
          <Spinner /> Loading rows
        </p>
      ) : data.length === 0 ? (
        <EmptyState
          title={draft ? 'No rows match this search' : 'This version is empty'}
          hint={draft ? 'Search is case-insensitive and includes nested values.' : undefined}
        />
      ) : (
        <>
          <div
            className="max-h-[32rem] overflow-auto"
            onScroll={(event) => {
              const target = event.currentTarget;
              if (
                target.scrollHeight - target.scrollTop - target.clientHeight < 160 &&
                query.hasNextPage &&
                !query.isFetchingNextPage
              ) {
                void query.fetchNextPage();
              }
            }}
          >
            <table className="w-max min-w-full border-separate border-spacing-0 text-left text-sm">
              <thead className="sticky top-0 z-10 bg-card">
                {table.getHeaderGroups().map((group) => (
                  <tr key={group.id}>
                    {group.headers.map((header) => (
                      <th
                        key={header.id}
                        className={cn(
                          'min-w-48 border-b border-r border-border px-3 py-2 text-xs font-semibold last:border-r-0',
                          header.id === '__row' && 'min-w-14 w-14',
                        )}
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
                    onClick={() => setSelected(row.original)}
                    className={cn(
                      'cursor-pointer transition-colors hover:bg-accent/30',
                      selected?.row_index === row.original.row_index && 'bg-accent/50',
                    )}
                  >
                    {row.getVisibleCells().map((cell) => (
                      <td
                        key={cell.id}
                        className="max-w-[32rem] border-b border-r border-border/50 px-3 py-2 align-top last:border-r-0"
                      >
                        {flexRender(cell.column.columnDef.cell, cell.getContext())}
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
            {query.isFetchingNextPage ? (
              <p className="flex items-center justify-center gap-2 py-3 text-xs text-muted-foreground">
                <Spinner /> Loading the next {ROW_PAGE} rows
              </p>
            ) : null}
          </div>
          {selected ? (
            <div className="border-t border-border bg-muted/20 p-3">
              <p className="mb-2 text-xs font-semibold">Row {selected.row_index}</p>
              <pre className="id max-h-64 overflow-auto whitespace-pre-wrap text-muted-foreground">
                {JSON.stringify(selected.row, null, 2)}
              </pre>
            </div>
          ) : null}
        </>
      )}
    </Card>
  );
}

function EvaluationsViewer({
  evaluations,
  total,
  reference,
  loading,
  fetchingMore,
  hasMore,
  onLoadMore,
}: {
  evaluations: EvaluationSummary[];
  total: number;
  reference: string;
  loading: boolean;
  fetchingMore: boolean;
  hasMore: boolean;
  onLoadMore: () => void;
}) {
  if (loading) {
    return <EmptyState title="Loading linked evaluations…" />;
  }
  if (evaluations.length === 0) {
    return (
      <Card className="p-4">
        <EmptyState
          title="No evaluations use this version yet"
          hint={`Record evaluation.dataset as ${reference} to create an exact, reproducible link.`}
        />
      </Card>
    );
  }

  return (
    <Card className="overflow-hidden">
      <div className="flex items-center justify-between gap-3 border-b border-border p-3">
        <div>
          <h3 className="text-sm font-semibold">Evaluation tests</h3>
          <p className="text-xs text-muted-foreground">
            {total} report{total === 1 ? '' : 's'} linked by exact version or legacy collection name.
          </p>
        </div>
        <Link
          to="/evaluation"
          search={{ dataset: reference, window: 0 }}
          className="text-xs text-primary hover:underline"
        >
          Open full Evaluation view
        </Link>
      </div>
      <div
        className="max-h-[34rem] overflow-auto"
        onScroll={(event) => {
          const target = event.currentTarget;
          if (target.scrollHeight - target.scrollTop - target.clientHeight < 120 && hasMore) {
            onLoadMore();
          }
        }}
      >
        <table className="w-full text-left text-sm">
          <thead className="sticky top-0 bg-card text-xs text-muted-foreground">
            <tr>
              <th className="px-3 py-2">Suite</th>
              <th className="px-3 py-2">Variant</th>
              <th className="px-3 py-2">Status</th>
              <th className="px-3 py-2">Pass rate</th>
              <th className="px-3 py-2">Cases</th>
              <th className="px-3 py-2">Started</th>
            </tr>
          </thead>
          <tbody>
            {evaluations.map((evaluation) => (
              <tr key={evaluation.evaluation_id} className="border-t border-border/50 hover:bg-accent/30">
                <td className="px-3 py-2">
                  <Link
                    to="/evaluation"
                    search={{
                      dataset: evaluation.dataset ?? undefined,
                      suite: evaluation.suite,
                      report: evaluation.evaluation_id,
                      window: 0,
                    }}
                    className="font-medium text-primary hover:underline"
                  >
                    {evaluation.suite}
                  </Link>
                </td>
                <td className="px-3 py-2 text-muted-foreground">{evaluation.variant ?? '—'}</td>
                <td className="px-3 py-2"><StatusBadge status={evaluation.status} /></td>
                <td className="px-3 py-2 tabular-nums">{formatRate(evaluation.pass_rate)}</td>
                <td className="px-3 py-2 tabular-nums">{evaluation.cases_total}</td>
                <td className="px-3 py-2 text-xs text-muted-foreground">{formatTime(evaluation.started_at)}</td>
              </tr>
            ))}
          </tbody>
        </table>
        {fetchingMore ? (
          <p className="flex items-center justify-center gap-2 py-3 text-xs text-muted-foreground">
            <Spinner /> Loading more reports
          </p>
        ) : null}
      </div>
    </Card>
  );
}

function Lineage({
  dataset,
  reference,
  evaluations,
  pipeline,
  source,
  windowSeconds,
  versionId,
}: {
  dataset: DatasetSummary;
  reference: string;
  evaluations: EvaluationSummary[];
  pipeline?: string;
  source?: string;
  windowSeconds?: number | null;
  versionId: string;
}) {
  const version = dataset.versions.find((item) => item.version === versionId) ?? dataset.latest;
  const variants = [...new Set(evaluations.flatMap((item) => (item.variant ? [item.variant] : [])))];
  return (
    <div className="grid gap-4 xl:grid-cols-[minmax(0,1fr)_20rem]">
      <Card className="overflow-hidden">
        <div className="border-b border-border p-3">
          <h3 className="text-sm font-semibold">Flow PHP provenance</h3>
          <p className="text-xs text-muted-foreground">The exact transformation stored with this immutable output.</p>
        </div>
        {pipeline ? (
          <pre className="id max-h-[34rem] overflow-auto whitespace-pre-wrap p-4 text-muted-foreground">{pipeline}</pre>
        ) : (
          <p className="p-4 text-sm text-muted-foreground">Open Rows once to load the artifact metadata.</p>
        )}
      </Card>
      <div className="flex flex-col gap-4">
        <Card className="p-4 text-xs">
          <p className="font-semibold">Dataset reference</p>
          <code className="id mt-2 block break-all text-primary">{reference}</code>
          <dl className="mt-4 grid grid-cols-[5rem_1fr] gap-x-2 gap-y-2 text-muted-foreground">
            <dt>Recipe</dt>
            <dd>
              {version.recipe ? (
                <Link
                  to="/data-curation"
                  search={{ q: pipeline, name: version.recipe, dataset: dataset.name }}
                  className="text-primary hover:underline"
                >
                  {version.recipe}
                </Link>
              ) : 'ad hoc'}
            </dd>
            <dt>Source</dt><dd className="break-all">{source ?? 'loading…'}</dd>
            <dt>Period</dt><dd>{windowSeconds ? `${windowSeconds}s` : 'all retained'}</dd>
          </dl>
        </Card>
        <Card className="p-4 text-xs">
          <p className="font-semibold">Experiment variants</p>
          {variants.length ? (
            <div className="mt-2 flex flex-wrap gap-1">
              {variants.map((variant) => (
                <Link
                  key={variant}
                  to="/experiments"
                  search={{ dataset: reference, variant }}
                >
                  <Badge tone="warning">{variant}</Badge>
                </Link>
              ))}
            </div>
          ) : (
            <p className="mt-2 text-muted-foreground">No evaluation has named a variant yet.</p>
          )}
        </Card>
      </div>
    </div>
  );
}

function ViewButton({
  active,
  onClick,
  icon: Icon,
  children,
}: {
  active: boolean;
  onClick: () => void;
  icon: React.ComponentType<{ className?: string }>;
  children: React.ReactNode;
}) {
  return (
    <Button size="sm" variant={active ? 'default' : 'ghost'} onClick={onClick}>
      <Icon className="h-3.5 w-3.5" /> {children}
    </Button>
  );
}

function useDatasetEvaluations(dataset: string, enabled: boolean) {
  return useInfiniteQuery({
    queryKey: ['dataset-evaluations', dataset],
    initialPageParam: undefined as string | undefined,
    enabled,
    queryFn: async ({ pageParam }) => {
      const response = await listEvaluations({
        query: { dataset, after: pageParam, limit: EVALUATION_PAGE },
      });
      if (!response.data) throw new Error('Could not load linked evaluations.');
      return response.data;
    },
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
}

function flattenEvaluations(
  pages: Array<{ evaluations: EvaluationSummary[] }> | undefined,
): EvaluationSummary[] {
  return pages?.flatMap((page) => page.evaluations) ?? [];
}

function DatasetCell({ value }: { value: unknown }) {
  if (value === null || value === undefined) return <span className="text-muted-foreground">—</span>;
  if (typeof value === 'object') {
    const text = JSON.stringify(value);
    return <span className="id block max-w-[30rem] truncate" title={text}>{text}</span>;
  }
  const text = String(value);
  return <span className={cn('block max-w-[30rem] truncate', typeof value === 'number' && 'tabular-nums')} title={text}>{text}</span>;
}

function inferColumnType(rows: DatasetRow[], column: string): string {
  const value = rows.find((row) => row.row[column] !== null && row.row[column] !== undefined)?.row[column];
  if (Array.isArray(value)) return 'array';
  if (value === null || value === undefined) return 'unknown';
  return typeof value;
}

function formatRate(value: number | null | undefined): string {
  return value === null || value === undefined ? '—' : `${(value * 100).toFixed(1)}%`;
}

function apiError(error: unknown, fallback: string): Error {
  if (error && typeof error === 'object' && 'message' in error && typeof error.message === 'string') {
    return new Error(error.message);
  }
  return new Error(fallback);
}
