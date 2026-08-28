import * as React from 'react';
import { createFileRoute } from '@tanstack/react-router';
import { useMutation, useQuery } from '@tanstack/react-query';
import { ChevronDown, ChevronRight, Play } from 'lucide-react';
import { z } from 'zod';

import {
  FlowQueryError,
  FlowUnavailableError,
  STARTER_QUERY,
  checkQuery,
  fetchDatasets,
  isFlowAvailable,
  runQuery,
  type FlowCheck,
  type FlowDataset,
  type FlowResult,
} from '@/lib/flow';
import { Badge, Button, Card, EmptyState, Spinner } from '@/components/ui/primitives';
import { VirtualList } from '@/components/virtual-list';
import { cn, formatCount } from '@/lib/utils';

/**
 * Queries over the same runs the explorer shows, written as a Flow pipeline.
 *
 * The explorer answers the questions someone thought of when it was built —
 * group by agent, by workflow, by span. This answers the ones nobody thought
 * of, at the cost of writing them out.
 *
 * ## Why the whole page degrades rather than breaks
 *
 * The Flow service is optional and lives outside the Rust binary, so "not
 * running" is a normal state, not a failure. It gets a first-class screen
 * naming the command to start it, and the other three observability views are
 * unaffected either way.
 *
 * ## Why the query is in the URL
 *
 * Same reason every filter in this panel is: a query worth running twice is
 * worth sending to someone, and a link that carries the shape but not the query
 * lands the reader somewhere else.
 */

const searchSchema = z.object({ q: z.string().optional() });

export const Route = createFileRoute('/observability/query')({
  validateSearch: searchSchema,
  component: QueryPage,
});

function QueryPage() {
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const [draft, setDraft] = React.useState(search.q ?? STARTER_QUERY);

  const available = useQuery({
    queryKey: ['flow', 'available'],
    queryFn: isFlowAvailable,
    // Cheap, and it is how the page recovers once the service is started.
    refetchInterval: 10_000,
  });

  const datasets = useQuery({
    queryKey: ['flow', 'datasets'],
    queryFn: fetchDatasets,
    enabled: available.data === true,
  });

  const query = useMutation({
    mutationFn: runQuery,
    onSuccess: (_, pipeline) => {
      void navigate({ search: { q: pipeline }, replace: true });
    },
  });

  // Checking is cheap — it parses and validates columns without calling
  // aiwatcher at all — so it runs as you type rather than only on Run. The
  // debounce is what keeps it from firing per keystroke.
  const [settled, setSettled] = React.useState(draft);
  React.useEffect(() => {
    const timer = setTimeout(() => setSettled(draft), 400);
    return () => clearTimeout(timer);
  }, [draft]);

  const check = useQuery({
    queryKey: ['flow', 'check', settled],
    queryFn: () => checkQuery(settled),
    enabled: available.data === true && settled.trim().length > 0,
    // A diagnostic for text that is already stale helps nobody.
    staleTime: Infinity,
    retry: false,
  });

  const submit = React.useCallback(() => query.mutate(draft), [query, draft]);

  // Cmd/Ctrl+Enter runs it. A Run button alone makes iterating on a query feel
  // like filling in a form.
  const onKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if ((event.metaKey || event.ctrlKey) && event.key === 'Enter') {
      event.preventDefault();
      submit();
    }
  };

  if (available.isLoading) {
    return <p className="text-sm text-muted-foreground">Looking for the query service…</p>;
  }

  if (available.data === false) {
    return <ServiceMissing />;
  }

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold">Query</h1>
          <p className="max-w-3xl text-sm text-muted-foreground">
            A Flow PHP pipeline over the same runs the explorer shows. Reads the API rather than a
            copy of it, so results are as current as the runs list — and bounded by the same
            retention.
          </p>
        </div>
        <Button onClick={submit} disabled={query.isPending} className="gap-2">
          {query.isPending ? <Spinner /> : <Play className="h-3.5 w-3.5" />}
          Run
          <span className="text-[10px] opacity-60">⌘↵</span>
        </Button>
      </div>

      <div className="grid gap-4 lg:grid-cols-[1fr_minmax(16rem,22rem)]">
        <div className="flex flex-col gap-4">
          <Card className="overflow-hidden">
            <textarea
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
              onKeyDown={onKeyDown}
              spellCheck={false}
              rows={12}
              className="id w-full resize-y bg-transparent p-3 outline-none"
            />
          </Card>

          <Diagnostics check={check.data} pending={check.isFetching} stale={settled !== draft} />

          <Result state={query} />
        </div>

        <Card className="overflow-hidden">
          <Schemas datasets={datasets.data?.datasets ?? []} maxRows={datasets.data?.max_rows} />
        </Card>
      </div>
    </div>
  );
}

/**
 * What is wrong with the query in the editor, before anyone presses Run.
 *
 * Kept visually quiet when the query is fine: a green tick on every keystroke
 * is noise, and the absence of complaints is already the signal.
 */
function Diagnostics({
  check,
  pending,
  stale,
}: {
  check: FlowCheck | undefined;
  pending: boolean;
  stale: boolean;
}) {
  if (!check || stale) return null;

  if (check.ok) {
    return (
      <p className="px-1 text-xs text-muted-foreground">
        {pending ? 'checking…' : `checks out · ${check.checked_by.join(' + ')}`}
      </p>
    );
  }

  return (
    <div className="flex flex-col gap-1.5">
      {check.diagnostics.map((diagnostic, index) => (
        <div
          key={`${diagnostic.offset}-${index}`}
          className="rounded-md border border-warning/40 bg-warning/5 px-3 py-2"
        >
          <p className="text-sm text-foreground">{diagnostic.message}</p>
          <p className="mt-1 text-[11px] text-muted-foreground">
            at character {diagnostic.offset}
            {diagnostic.help ? ` · ${diagnostic.help}` : ''}
          </p>
        </div>
      ))}
    </div>
  );
}

function ServiceMissing() {
  return (
    <div className="flex flex-col gap-4">
      <div>
        <h1 className="text-lg font-semibold">Query</h1>
        <p className="max-w-3xl text-sm text-muted-foreground">
          Flow PHP pipelines over the runs aiwatcher has recorded.
        </p>
      </div>
      <EmptyState
        title="The query service is not running"
        hint="It is a separate PHP service, optional and outside the Rust binary — everything else on this page works without it."
      />
      {/*
        Both, because this screen cannot tell which one you are looking at: the
        panel is the same build on a laptop and in a cluster, and the answer is
        a different one in each. Showing only the `just` commands sent anyone
        reading this from a deployment looking for a checkout that is not there.
      */}
      <Card className="p-4">
        <p className="mb-2 text-xs text-muted-foreground">Locally, from a checkout:</p>
        <pre className="id overflow-x-auto rounded bg-muted p-3 text-muted-foreground">
          just flow-install{'\n'}just flow-serve
        </pre>
      </Card>
      <Card className="p-4">
        <p className="mb-2 text-xs text-muted-foreground">
          In a cluster, it is off in the chart until you ask for it:
        </p>
        <pre className="id overflow-x-auto rounded bg-muted p-3 text-muted-foreground">
          helm upgrade … --set flow.enabled=true
        </pre>
        <p className="mt-2 text-xs text-muted-foreground">
          Needs the <span className="font-mono">aiwatcher-flow</span> image, which{' '}
          <span className="font-mono">deploy/scripts/build-images.sh</span> builds beside the other
          two.
        </p>
      </Card>
    </div>
  );
}

function Result({
  state,
}: {
  state: {
    isPending: boolean;
    error: Error | null;
    data: FlowResult | undefined;
  };
}) {
  if (state.error instanceof FlowUnavailableError) {
    return (
      <EmptyState
        title="The query service stopped responding"
        hint="Start it with `just flow-serve`."
      />
    );
  }

  if (state.error instanceof FlowQueryError) {
    return (
      <Card className="border-danger/40 p-4">
        <p className="text-xs font-medium text-danger">The query was refused</p>
        <p className="mt-1 text-sm text-foreground">{state.error.message}</p>
        {state.error.column > 0 ? (
          <p className="mt-2 text-xs text-muted-foreground">at character {state.error.column}</p>
        ) : null}
      </Card>
    );
  }

  if (state.error) {
    return <Card className="border-danger/40 p-4 text-sm text-danger">{state.error.message}</Card>;
  }

  if (!state.data) {
    return (
      <EmptyState
        title="Nothing run yet"
        hint="Edit the pipeline and press Run. The starter query groups runs by agent."
      />
    );
  }

  return <ResultTable result={state.data} />;
}

function ResultTable({ result }: { result: FlowResult }) {
  if (result.rows.length === 0) {
    return <EmptyState title="No rows" hint="The pipeline ran and matched nothing." />;
  }

  return (
    <Card className="overflow-hidden">
      {/*
       * Where the numbers came from, said once. A table on its own reads as
       * live and complete, and this one is neither by default — it is the
       * retention window, as of the moment Run was pressed.
       */}
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-border p-2 text-xs text-muted-foreground">
        <span>
          {formatCount(result.row_count)} row{result.row_count === 1 ? '' : 's'}
        </span>
        <span>·</span>
        <span>
          {result.dataset} — {result.grain}
        </span>
        <span>·</span>
        <span>{result.took_ms} ms</span>
        <span>·</span>
        <span className="truncate">from {result.source}, within its retention window</span>
        {result.truncated ? (
          <Badge tone="warning" className="px-1.5 py-0 text-[10px]">
            truncated at the row cap
          </Badge>
        ) : null}
      </div>

      <div className="overflow-x-auto">
        <div className="min-w-full">
          <div className="flex border-b border-border bg-muted/40 text-xs font-medium">
            {result.columns.map((column) => (
              <div key={column} className="min-w-40 flex-1 px-3 py-1.5">
                {column}
              </div>
            ))}
          </div>
          <VirtualList
            items={result.rows}
            className="max-h-[28rem]"
            estimateSize={30}
            keyOf={(_, index) => String(index)}
            renderRow={(row) => (
              <div className="flex border-b border-border/20 text-sm hover:bg-accent/30">
                {result.columns.map((column) => (
                  <div
                    key={column}
                    className={cn(
                      'min-w-40 flex-1 px-3 py-1.5 tabular-nums',
                      result.truncate_cells && 'truncate',
                    )}
                  >
                    <Cell value={row[column]} />
                  </div>
                ))}
              </div>
            )}
          />
        </div>
      </div>
    </Card>
  );
}

function Cell({ value }: { value: unknown }) {
  if (value === null || value === undefined) {
    return <span className="text-muted-foreground">—</span>;
  }
  if (typeof value === 'object') {
    return <span className="id">{JSON.stringify(value)}</span>;
  }
  return <>{String(value)}</>;
}

/** The columns each dataset has. Writing a query against an undocumented shape is guesswork. */
function Schemas({ datasets, maxRows }: { datasets: FlowDataset[]; maxRows?: number }) {
  return (
    <div className="flex max-h-[38rem] flex-col">
      <div className="border-b border-border p-2 text-xs font-medium">Datasets</div>
      <div className="overflow-y-auto">
        {datasets.map((dataset) => (
          <DatasetSchema key={dataset.name} dataset={dataset} />
        ))}
        {maxRows ? (
          <p className="border-t border-border/60 p-3 text-[11px] leading-relaxed text-muted-foreground">
            At most {formatCount(maxRows)} rows come back per query; more than that is reported
            rather than silently cut. Use <code>-&gt;same(…)</code> rather than{' '}
            <code>-&gt;equals(…)</code> — every column here can be null, and the loose comparison
            mishandles nulls.
          </p>
        ) : null}
      </div>
    </div>
  );
}

function DatasetSchema({ dataset }: { dataset: FlowDataset }) {
  const [open, setOpen] = React.useState(dataset.name === 'runs');

  return (
    <div className="border-b border-border/40 last:border-b-0">
      <button
        type="button"
        onClick={() => setOpen((value) => !value)}
        className="flex w-full items-center gap-1.5 px-2 py-1.5 text-left text-sm hover:bg-accent/40"
      >
        {open ? (
          <ChevronDown className="h-3 w-3 shrink-0 text-muted-foreground" />
        ) : (
          <ChevronRight className="h-3 w-3 shrink-0 text-muted-foreground" />
        )}
        <span className="font-medium">{dataset.name}</span>
        {dataset.aliases.map((alias) => (
          <Badge key={alias} className="px-1.5 py-0 text-[10px]">
            {alias}
          </Badge>
        ))}
      </button>

      {open ? (
        <div className="px-3 pb-2">
          <p className="mb-1 text-[11px] leading-relaxed text-muted-foreground">
            {dataset.grain}. {dataset.description}
          </p>
          <div className="flex flex-col">
            {dataset.columns.map((column) => (
              <div key={column.name} className="flex items-baseline justify-between gap-2 py-0.5">
                <span className="id text-foreground">{column.name}</span>
                <span className="text-[10px] text-muted-foreground">{column.type}</span>
              </div>
            ))}
          </div>
        </div>
      ) : null}
    </div>
  );
}
