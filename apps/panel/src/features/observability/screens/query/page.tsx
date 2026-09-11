import { useMutation, useQuery } from '@tanstack/react-query';
import { Link, getRouteApi } from '@tanstack/react-router';
import { ChevronDown, ChevronRight, MousePointerClick, Pencil, Play, Radio } from 'lucide-react';
import * as React from 'react';
import { z } from 'zod';
import { searchSchema } from './search';

import { AttributePicker } from '@/features/observability/components/attribute-picker';
import { useObservabilityRevision } from '@/features/observability/lib/observability-revision';
import {
  ATTRIBUTES,
  EMPTY_DRAFT,
  METRICS,
  compile,
  countMetric,
  isBlank,
  sortableColumns,
  type AttributeId,
  type Grain,
  type QueryDraft,
} from '@/features/observability/lib/query-builder';
import {
  selectionFromSearch,
  selectionToSearch,
} from '@/features/observability/lib/selection-params';
import {
  DEFAULT_WINDOW_SECONDS,
  TIME_WINDOWS,
  TimeRange,
  windowParam,
} from '@/shared/components/time-range';
import { Badge, Button, Card, EmptyState, Spinner } from '@/shared/components/ui/primitives';
import { VirtualList } from '@/shared/components/virtual-list';
import { contentFor } from '@/shared/lib/engine-content';
import {
  ENGINE_LABEL,
  QueryEngineUnavailableError,
  QueryError,
  checkQuery,
  fetchDatasets,
  isQueryEngineAvailable,
  linkedEngine,
  runQuery,
  useQueryEngine,
  writtenElsewhere,
  type QueryCheck,
  type QueryDataset,
  type QueryEngineName,
  type QueryResult,
} from '@/shared/lib/query';
import { cn, formatCount } from '@/shared/lib/utils';

const routeApi = getRouteApi('/observability/query');

type Search = z.infer<typeof searchSchema>;

/** The draft the URL is. Reading it here is what makes a builder link work. */
function draftFromSearch(search: Search): QueryDraft {
  const [by, direction] = (search.sort ?? '').split(':');
  return {
    grain: (search.grain ?? EMPTY_DRAFT.grain) as Grain,
    filters: selectionFromSearch(search),
    groupBy: (search.group ?? []) as AttributeId[],
    metrics: search.metric ?? [],
    sort: by && (direction === 'asc' || direction === 'desc') ? { by, direction } : undefined,
    limit: search.limit,
  };
}

export function QueryPage() {
  const search = routeApi.useSearch();
  const navigate = routeApi.useNavigate();
  const windowSeconds = search.window ?? DEFAULT_WINDOW_SECONDS;
  const liveRevision = useObservabilityRevision();

  // The language the deployed engine reads (AW-3). Flow's until healthz answers,
  // because that is what absence has always meant — but nothing is foreign until
  // it has, which is why `deployed` is kept apart.
  const deployed = useQueryEngine().data?.engine;
  const engine = deployed ?? 'flow';

  const mode = search.mode ?? (search.q ? 'write' : 'build');
  const draft = React.useMemo(() => draftFromSearch(search), [search]);
  const built = React.useMemo(() => compile(draft, engine), [draft, engine]);

  // In `build` the text is derived and the editor is a preview; in `write` the
  // editor holds the truth. One state either way, so Run never has to ask
  // which of two things it is running. `null` is the engine's starter, so a page
  // opened before healthz answers still lands on text the engine runs.
  const [written, setWritten] = React.useState<string | null>(search.q ?? null);
  const text = written ?? contentFor(engine)?.starterQuery ?? '';
  const pipeline = mode === 'build' ? built : text;

  // The engine the editor's text was written for. Text from a link is what the
  // link says, and Flow's when it says nothing; the starter and what Build hands
  // over are the deployed engine's. Another engine's is shown and not run, as a
  // recipe or a block is: the engine would answer it with a syntax error that
  // reads as the text being wrong.
  const writtenFor = written === null ? engine : linkedEngine(search.q, search.writtenFor, engine);
  const foreign = mode === 'write' ? writtenElsewhere(writtenFor, deployed) : null;

  const available = useQuery({
    queryKey: ['flow', 'available'],
    queryFn: isQueryEngineAvailable,
    // Cheap, and it is how the page recovers once the service is started.
    refetchInterval: 10_000,
  });

  const datasets = useQuery({
    queryKey: ['flow', 'datasets'],
    queryFn: fetchDatasets,
    enabled: available.data === true,
  });

  // The window scopes the datasets the query reads, not the query itself: the
  // service forwards it to the aiwatcher routes that take one and leaves the
  // per-run `events` route alone. The first run is explicit; after that the
  // executed pipeline follows backend events just like the other tabs.
  const lastPipeline = React.useRef<string | null>(null);
  const query = useMutation({
    mutationFn: (text: string) => runQuery(text, windowParam(windowSeconds)),
    onSuccess: (_, text) => {
      lastPipeline.current = text;
      // Only `write` mode writes the text back: in `build` the URL already
      // holds the draft that produced it, and storing both would be two
      // representations of one query, free to disagree on the next reload.
      if (mode === 'write') {
        void navigate({
          search: (previous) => ({ ...previous, q: text, writtenFor }),
          replace: true,
        });
      }
    },
  });

  const appliedRevision = React.useRef(liveRevision);
  React.useEffect(() => {
    if (appliedRevision.current === liveRevision) return;
    if (!lastPipeline.current) {
      appliedRevision.current = liveRevision;
      return;
    }
    // If a refresh is already running, leave the revision pending. This effect
    // runs again when the mutation settles and folds every intervening event
    // into one follow-up query instead of racing responses against each other.
    if (query.isPending) return;
    appliedRevision.current = liveRevision;
    query.mutate(lastPipeline.current);
  }, [liveRevision, query.isPending, query.mutate]);

  // Checking is cheap — it parses and validates columns without calling
  // aiwatcher at all — so it runs as the query changes rather than only on
  // Run. The debounce is what keeps it from firing per keystroke, and it
  // covers the builder too: dragging a limit is as chatty as typing.
  const [settled, setSettled] = React.useState(pipeline);
  React.useEffect(() => {
    const timer = setTimeout(() => setSettled(pipeline), 400);
    return () => clearTimeout(timer);
  }, [pipeline]);

  const check = useQuery({
    queryKey: ['flow', 'check', settled],
    queryFn: () => checkQuery(settled),
    enabled: available.data === true && settled.trim().length > 0 && !foreign,
    // A diagnostic for text that is already stale helps nobody.
    staleTime: Infinity,
    retry: false,
  });

  const submit = React.useCallback(() => {
    if (!foreign) query.mutate(pipeline);
  }, [query, pipeline, foreign]);

  // Cmd/Ctrl+Enter runs it. A Run button alone makes iterating on a query feel
  // like filling in a form.
  const onKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if ((event.metaKey || event.ctrlKey) && event.key === 'Enter') {
      event.preventDefault();
      submit();
    }
  };

  const patch = React.useCallback(
    (next: Partial<Search>) => void navigate({ search: (previous) => ({ ...previous, ...next }) }),
    [navigate],
  );

  const editDraft = React.useCallback(
    (next: QueryDraft) =>
      patch({
        ...selectionToSearch(next.filters),
        grain: next.grain,
        group: next.groupBy.length > 0 ? next.groupBy : undefined,
        metric: next.metrics.length > 0 ? next.metrics : undefined,
        sort: next.sort ? `${next.sort.by}:${next.sort.direction}` : undefined,
        limit: next.limit,
      }),
    [patch],
  );

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
            A {ENGINE_LABEL[engine]} query over the same runs the explorer shows. Reads the API
            rather than a copy of it, so after the first Run its results follow live events — and
            stay bounded by the same retention.
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-3">
          <TimeRange value={windowSeconds} onChange={(seconds) => patch({ window: seconds })} />
          <Button onClick={submit} disabled={query.isPending || Boolean(foreign)} className="gap-2">
            {query.isPending ? <Spinner /> : <Play className="h-3.5 w-3.5" />}
            Run
            <span className="text-[10px] opacity-60">⌘↵</span>
          </Button>
        </div>
      </div>

      <div className="grid gap-4 lg:grid-cols-[minmax(15rem,19rem)_1fr]">
        <div className="flex flex-col gap-4">
          <Card className="overflow-hidden">
            <div className="border-b border-border p-2 text-xs font-medium">Attributes</div>
            <GrainToggle
              grain={draft.grain}
              onChange={(grain) => editDraft({ ...draft, grain })}
              disabled={mode === 'write'}
            />
            <div className={cn('max-h-[24rem] overflow-y-auto', mode === 'write' && 'opacity-50')}>
              <AttributePicker
                attributes={ATTRIBUTES}
                value={draft.filters}
                onChange={(filters) => editDraft({ ...draft, filters })}
                windowSeconds={windowSeconds}
                unavailable={(attribute) =>
                  attribute.reach[draft.grain] ? undefined : attribute.unavailable
                }
              />
            </div>
          </Card>

          <Card className="overflow-hidden">
            <Schemas
              datasets={datasets.data?.datasets ?? []}
              maxRows={datasets.data?.max_rows}
              engine={engine}
            />
          </Card>
        </div>

        <div className="flex flex-col gap-4">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <ModeToggle
              mode={mode}
              onBuild={() => patch({ mode: 'build', q: undefined, writtenFor: undefined })}
              onWrite={() => {
                // The compiled text becomes the written one, so nothing is
                // lost crossing over — and `mode` is what makes it one-way.
                setWritten(built);
                patch({ mode: 'write', q: built, writtenFor: engine });
              }}
            />
            <WatchLive draft={draft} windowSeconds={windowSeconds} />
          </div>

          {mode === 'build' ? <Shape draft={draft} onChange={editDraft} /> : null}

          <Card className="overflow-hidden">
            {mode === 'build' ? (
              <>
                <div className="flex items-center justify-between border-b border-border px-3 py-1.5 text-xs text-muted-foreground">
                  <span>The pipeline this builds</span>
                  <span>read-only until you take it into the editor</span>
                </div>
                <pre className="id max-h-[22rem] overflow-auto p-3 text-muted-foreground">
                  {built}
                </pre>
              </>
            ) : (
              <>
                {foreign ? (
                  <p className="border-b border-border bg-warning/10 px-3 py-2 text-xs text-warning">
                    {foreign}
                  </p>
                ) : null}
                <textarea
                  value={text}
                  onChange={(event) => setWritten(event.target.value)}
                  onKeyDown={onKeyDown}
                  readOnly={Boolean(foreign)}
                  spellCheck={false}
                  rows={14}
                  className="id w-full resize-y bg-transparent p-3 outline-none"
                />
              </>
            )}
          </Card>

          <Diagnostics check={check.data} pending={check.isFetching} stale={settled !== pipeline} />

          <Result state={query} blank={mode === 'build' && isBlank(draft)} />
        </div>
      </div>
    </div>
  );
}

/**
 * Runs or spans.
 *
 * Named "grain" rather than "dataset" because that is the word the query
 * service's own catalog uses for it, and because what it decides is what one
 * row *is* — which is the thing that makes model and tool reachable and
 * runtime and session not.
 */
function GrainToggle({
  grain,
  onChange,
  disabled,
}: {
  grain: Grain;
  onChange: (grain: Grain) => void;
  disabled: boolean;
}) {
  return (
    <div className="flex items-center gap-1 border-b border-border px-2 py-1.5">
      <span className="mr-1 text-[11px] text-muted-foreground">One row per</span>
      {(['runs', 'spans'] as const).map((candidate) => (
        <button
          key={candidate}
          type="button"
          disabled={disabled}
          onClick={() => onChange(candidate)}
          className={cn(
            'rounded-full border px-2 py-0.5 text-xs transition-colors disabled:cursor-not-allowed disabled:opacity-50',
            grain === candidate
              ? 'border-primary bg-primary/10 text-foreground'
              : 'border-border text-muted-foreground hover:text-foreground',
          )}
        >
          {candidate === 'runs' ? 'run' : 'span'}
        </button>
      ))}
    </div>
  );
}

function ModeToggle({
  mode,
  onBuild,
  onWrite,
}: {
  mode: 'build' | 'write';
  onBuild: () => void;
  onWrite: () => void;
}) {
  return (
    <div className="flex items-center gap-1">
      <button
        type="button"
        onClick={onBuild}
        className={cn(
          'flex items-center gap-1.5 rounded-md px-2.5 py-1 text-sm transition-colors',
          mode === 'build'
            ? 'bg-accent text-foreground'
            : 'text-muted-foreground hover:text-foreground',
        )}
      >
        <MousePointerClick className="h-3.5 w-3.5" />
        Build
      </button>
      <button
        type="button"
        onClick={onWrite}
        title={
          mode === 'build'
            ? 'Takes the compiled pipeline into the editor. There is no way back — the builder cannot read a query it did not write.'
            : undefined
        }
        className={cn(
          'flex items-center gap-1.5 rounded-md px-2.5 py-1 text-sm transition-colors',
          mode === 'write'
            ? 'bg-accent text-foreground'
            : 'text-muted-foreground hover:text-foreground',
        )}
      >
        <Pencil className="h-3.5 w-3.5" />
        Write
      </button>
    </div>
  );
}

/**
 * The same filter, followed as it happens.
 *
 * A link rather than a mode on this page: the two answer different questions —
 * this one aggregates what the read model retains, that one tails the log —
 * and a toggle would imply the table refreshes into the feed.
 */
function WatchLive({ draft, windowSeconds }: { draft: QueryDraft; windowSeconds: number }) {
  return (
    <Link
      to="/observability/live"
      search={{ ...selectionToSearch(draft.filters), window: windowSeconds }}
      className="flex items-center gap-1.5 rounded-md px-2.5 py-1 text-sm text-muted-foreground transition-colors hover:text-foreground"
    >
      <Radio className="h-3.5 w-3.5" />
      Watch live
    </Link>
  );
}

/** What the rows are grouped into, and what is counted for each group. */
function Shape({ draft, onChange }: { draft: QueryDraft; onChange: (next: QueryDraft) => void }) {
  const groupable = ATTRIBUTES.filter((attribute) => attribute.reach[draft.grain]);
  const metrics = METRICS.filter((metric) => metric.grains.includes(draft.grain));
  const count = countMetric(draft.grain);
  const columns = sortableColumns(draft);
  const sort = draft.sort ?? { by: columns[0] ?? '', direction: 'desc' as const };

  const toggle = <T,>(list: T[], item: T) =>
    list.includes(item) ? list.filter((candidate) => candidate !== item) : [...list, item];

  return (
    <Card className="flex flex-col gap-3 p-3">
      <Row label="Group by">
        {groupable.map((attribute) => (
          <Chip
            key={attribute.id}
            label={attribute.label}
            on={draft.groupBy.includes(attribute.id)}
            onClick={() => onChange({ ...draft, groupBy: toggle(draft.groupBy, attribute.id) })}
          />
        ))}
        {draft.groupBy.length === 0 ? (
          <span className="text-[11px] text-muted-foreground">
            nothing grouped — the rows come back as they are
          </span>
        ) : null}
      </Row>

      {draft.groupBy.length > 0 ? (
        <Row label="Report">
          {/* The grain's count is on and cannot come off: `aggregate()` with
              nothing in it is a refusal, and a list of group keys with no
              number beside it is a second question. */}
          <Chip label={count.label} on locked />
          {metrics
            .filter((metric) => metric.id !== count.id)
            .map((metric) => (
              <Chip
                key={metric.id}
                label={metric.label}
                on={draft.metrics.includes(metric.id)}
                onClick={() => onChange({ ...draft, metrics: toggle(draft.metrics, metric.id) })}
              />
            ))}
        </Row>
      ) : null}

      <Row label="Sort by">
        <select
          value={sort.by}
          onChange={(event) =>
            onChange({ ...draft, sort: { by: event.target.value, direction: sort.direction } })
          }
          className="rounded-md border border-border bg-transparent px-2 py-0.5 text-xs"
        >
          {columns.map((column) => (
            <option key={column} value={column}>
              {column}
            </option>
          ))}
        </select>
        <Chip
          label={sort.direction === 'desc' ? 'descending' : 'ascending'}
          on
          onClick={() =>
            onChange({
              ...draft,
              sort: { by: sort.by, direction: sort.direction === 'desc' ? 'asc' : 'desc' },
            })
          }
        />
        <span className="ml-2 text-[11px] text-muted-foreground">at most</span>
        <input
          type="number"
          min={1}
          max={10_000}
          value={draft.limit ?? ''}
          placeholder="all"
          onChange={(event) =>
            onChange({
              ...draft,
              limit: event.target.value ? Number(event.target.value) : undefined,
            })
          }
          className="w-20 rounded-md border border-border bg-transparent px-2 py-0.5 text-xs"
        />
        <span className="text-[11px] text-muted-foreground">rows</span>
      </Row>
    </Card>
  );
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      <span className="w-[4.5rem] shrink-0 text-[11px] text-muted-foreground">{label}</span>
      {children}
    </div>
  );
}

function Chip({
  label,
  on,
  locked,
  onClick,
}: {
  label: string;
  on: boolean;
  locked?: boolean;
  onClick?: () => void;
}) {
  return (
    <button
      type="button"
      disabled={locked}
      onClick={onClick}
      className={cn(
        'rounded-full border px-2 py-0.5 text-xs transition-colors',
        on
          ? 'border-primary bg-primary/10 text-foreground'
          : 'border-border text-muted-foreground hover:text-foreground',
        locked && 'cursor-default opacity-70',
      )}
    >
      {label}
    </button>
  );
}

/** `900` → `15m`. The label the time control uses for the same number. */
function formatWindow(seconds: number): string {
  return TIME_WINDOWS.find((option) => option.seconds === seconds)?.label ?? `${seconds}s`;
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
  check: QueryCheck | undefined;
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
          Queries over the runs aiwatcher has recorded, in the language the deployed engine reads.
        </p>
      </div>
      <EmptyState
        title="The query service is not running"
        hint="It is a separate service — Flow PHP, DataFusion or DuckDB, whichever AIWATCHER_QUERY_ENGINE names — optional and outside the Rust binary; everything else on this page works without it."
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
          just query-install{'\n'}just query-serve
        </pre>
      </Card>
      <Card className="p-4">
        <p className="mb-2 text-xs text-muted-foreground">
          In a cluster, it is off in the chart until you ask for it:
        </p>
        <pre className="id overflow-x-auto rounded bg-muted p-3 text-muted-foreground">
          helm upgrade … --set query.enabled=true --set query.engine=flow|datafusion|duckdb
        </pre>
        <p className="mt-2 text-xs text-muted-foreground">
          Needs that engine&apos;s image — <span className="font-mono">aiwatcher-flow</span>,{' '}
          <span className="font-mono">aiwatcher-query-datafusion</span> or{' '}
          <span className="font-mono">aiwatcher-query-duckdb</span> — which a release publishes
          beside the server and the panel.
        </p>
      </Card>
    </div>
  );
}

function Result({
  state,
  blank,
}: {
  state: {
    isPending: boolean;
    error: Error | null;
    data: QueryResult | undefined;
  };
  /** Nothing has been narrowed yet, so the empty state can say what to click. */
  blank: boolean;
}) {
  if (state.error instanceof QueryEngineUnavailableError) {
    return (
      <EmptyState
        title="The query service stopped responding"
        hint="Start it with `just query-serve`."
      />
    );
  }

  if (state.error instanceof QueryError) {
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
        hint={
          blank
            ? 'Pick an attribute on the left, or something to group by, then press Run.'
            : 'Press Run. The pipeline below is what will be sent.'
        }
      />
    );
  }

  return <ResultTable result={state.data} />;
}

function ResultTable({ result }: { result: QueryResult }) {
  if (result.rows.length === 0) {
    return <EmptyState title="No rows" hint="The pipeline ran and matched nothing." />;
  }

  return (
    <Card className="overflow-hidden">
      {/*
       * Where the numbers came from, said once. The first result is explicit;
       * while this tab stays open it is re-run when the live stream advances.
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
        {/*
         * Said rather than implied: a scoped table and an empty system look
         * identical, and the window was applied when Run was pressed, not
         * when the control was last moved.
         */}
        <span className="truncate">
          from {result.source},{' '}
          {result.window_seconds
            ? `over the last ${formatWindow(result.window_seconds)}`
            : 'within its retention window'}
        </span>
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
function Schemas({
  datasets,
  maxRows,
  engine,
}: {
  datasets: QueryDataset[];
  maxRows?: number;
  /** The deployed engine: Flow's null trap is its own, and each Python engine spells the test its own way. */
  engine: QueryEngineName;
}) {
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
            rather than silently cut.{' '}
            {engine === 'flow' ? (
              <>
                Use <code>-&gt;same(…)</code> rather than <code>-&gt;equals(…)</code> — every column
                here can be null, and the loose comparison mishandles nulls.
              </>
            ) : (
              <>
                Every column here can be null, and <code>==</code> against a null is null, which a
                filter drops — ask <code>{engine === 'duckdb' ? '.isnull()' : '.is_null()'}</code>{' '}
                for those rows.
              </>
            )}
          </p>
        ) : null}
      </div>
    </div>
  );
}

function DatasetSchema({ dataset }: { dataset: QueryDataset }) {
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
