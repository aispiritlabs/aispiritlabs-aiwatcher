import * as React from 'react';
import { createFileRoute, Link } from '@tanstack/react-router';
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { ChevronDown, ChevronRight, Search } from 'lucide-react';
import { z } from 'zod';

import { getRunEvents, listDimension, listRuns, listSpans } from '@/api/generated/sdk.gen';
import type {
  DimensionKind,
  DimensionSummary,
  RecordedEvent,
  RunStatus,
  RunSummary,
  SpanRow,
} from '@/api/generated/types.gen';
import { Badge, Button, Card, EmptyState, IdChip, Spinner } from '@/components/ui/primitives';
import { StatusBadge } from '@/components/status-badge';
import { VirtualList } from '@/components/virtual-list';
import { cn, formatCount, formatDuration, formatTime, pinchId, shortId } from '@/lib/utils';

/**
 * One place to move between every level of a run.
 *
 * ```text
 * <pivot> → run (trace) → agent (span) → llm / tool call (span) → events
 * ```
 *
 * The tree on the left is the whole hierarchy, expandable in place; the pane on
 * the right is the messages for whatever is selected. Selecting deeper never
 * loses the levels above it, which is the thing that made this hard before:
 * inspecting one LLM call meant going back to a list and re-filtering, and the
 * way back up was gone.
 *
 * Every selection lives in the URL, so a level is linkable and the back button
 * walks the hierarchy.
 *
 * ## Why there is one grouping control and not two
 *
 * There used to be a second one, in the message pane: having narrowed to a
 * span, you could then re-group its messages by span, agent or type. It was
 * grouping already-narrowed data by the thing it had just been narrowed to, and
 * it collapsed the one view that has to stay chronological. The pivot decides
 * the shape; below it the list is flat and in order.
 *
 * ## Why nothing here loads eagerly
 *
 * Every list is a cursor page from the server and a virtual window in the
 * browser. A run with forty thousand events used to arrive in one response and
 * mount forty thousand rows; now it arrives two hundred at a time and mounts
 * what fits on screen. The searches are server-side for the same reason —
 * filtering in the browser requires having downloaded everything first, which
 * is exactly what this avoids.
 */

/** What the tree's top level is. Everything below it stays the same. */
const PIVOTS = [
  'session',
  'agent',
  'runtime',
  'workflow',
  'trace',
  'model',
  'tool',
  'span',
] as const;
type Pivot = (typeof PIVOTS)[number];

/** How a pivot narrows the runs list when one of its rows is opened. */
const RUN_FILTER: Record<Exclude<Pivot, 'span'>, string> = {
  session: 'conversation_id',
  agent: 'agent_id',
  runtime: 'runtime',
  workflow: 'workflow',
  trace: 'trace_id',
  model: 'model',
  tool: 'tool',
};

const TREE_PAGE = 100;
const EVENT_PAGE = 200;

const searchSchema = z.object({
  /** The dimension the tree is rooted on. */
  by: z.enum(PIVOTS).optional(),
  /** The selected row of that dimension. */
  key: z.string().optional(),
  run: z.string().optional(),
  span: z.string().optional(),
  /** Filters the tree. Server-side. */
  find: z.string().optional(),
  /** Filters the messages. Server-side. */
  q: z.string().optional(),
});

export const Route = createFileRoute('/observability/explore')({
  validateSearch: searchSchema,
  component: ExplorePage,
});

type Selection = z.infer<typeof searchSchema>;

function ExplorePage() {
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const pivot = search.by ?? 'session';

  // Two callbacks on purpose. `merge` is for the tree and the search boxes,
  // which change one field; `replace` is for the breadcrumbs and the pivot,
  // which navigate to a whole level and must not depend on what was set before.
  const merge = React.useCallback(
    (next: Partial<Selection>) => {
      void navigate({ search: (previous) => ({ ...previous, ...next }) });
    },
    [navigate],
  );
  const replace = React.useCallback(
    (next: Selection) => {
      void navigate({ search: () => next });
    },
    [navigate],
  );

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold">Explore</h1>
          <p className="text-sm text-muted-foreground">
            Sessions, runtimes, workflows, runs, spans and the messages underneath them — without
            leaving the page.
          </p>
        </div>
        {/*
         * The pivot. The hierarchy below a root never changes — run → span →
         * messages — so switching what the top level *is* costs no relearning,
         * and "which runtime is slow" and "which session failed" are the same
         * three clicks.
         */}
        <div className="flex flex-wrap items-center gap-1">
          <span className="mr-1 text-xs text-muted-foreground">group by</span>
          {PIVOTS.map((option) => (
            <Button
              key={option}
              size="sm"
              variant={pivot === option ? 'default' : 'outline'}
              onClick={() => replace({ by: option })}
            >
              {option}
            </Button>
          ))}
        </div>
      </div>

      <Breadcrumbs selection={search} onSelect={replace} />

      <div className="grid gap-4 lg:grid-cols-[minmax(20rem,28rem)_1fr]">
        <Card className="overflow-hidden">
          <Tree selection={search} onSelect={merge} />
        </Card>
        <Card className="overflow-hidden">
          <Messages selection={search} onSelect={merge} />
        </Card>
      </div>
    </div>
  );
}

/**
 * The path back up.
 *
 * Each crumb navigates to an **explicit** selection state rather than clearing
 * one field over the current one. Partial clears are where this went wrong
 * first: spreading `{ span: undefined }` over the previous search dropped the
 * whole path, and the fix that always holds is to say what the target is
 * instead of what to remove.
 */
function Breadcrumbs({
  selection,
  onSelect,
}: {
  selection: Selection;
  onSelect: (next: Selection) => void;
}) {
  const pivot = selection.by ?? 'session';
  const crumbs: { label: string; value: string; target: Selection }[] = [];

  if (selection.key) {
    crumbs.push({
      label: pivot,
      value: pivot === 'trace' ? pinchId(selection.key) : selection.key,
      target: { by: selection.by, key: selection.key },
    });
  }
  if (selection.run) {
    crumbs.push({
      label: 'run',
      value: selection.run,
      target: { by: selection.by, key: selection.key, run: selection.run },
    });
  }
  if (selection.span) {
    crumbs.push({
      label: 'span',
      value: shortId(selection.span),
      target: {
        by: selection.by,
        key: selection.key,
        run: selection.run,
        span: selection.span,
      },
    });
  }

  if (crumbs.length === 0) {
    return (
      <p className="text-xs text-muted-foreground">
        Pick a {pivot} to start, or open a run directly from the runs list.
      </p>
    );
  }

  return (
    <div className="flex flex-wrap items-center gap-1.5 text-xs">
      <button
        type="button"
        onClick={() => onSelect({ by: selection.by })}
        className="text-muted-foreground hover:text-foreground"
      >
        all {pivot}s
      </button>
      {crumbs.map((crumb, index) => (
        <React.Fragment key={crumb.label}>
          <span className="text-muted-foreground">/</span>
          <button
            type="button"
            onClick={() => onSelect(crumb.target)}
            className={cn(
              'rounded px-1.5 py-0.5 transition-colors',
              index === crumbs.length - 1
                ? 'bg-accent text-accent-foreground'
                : 'text-muted-foreground hover:text-foreground',
            )}
          >
            <span className="text-muted-foreground">{crumb.label} </span>
            {crumb.value}
          </button>
        </React.Fragment>
      ))}
    </div>
  );
}

// ── Tree ─────────────────────────────────────────────────────────────────────

/**
 * The tree, flattened.
 *
 * A virtual window needs one array, not a nest of components, so the whole
 * expanded tree is materialised into rows and the depth is carried on each row.
 * Only one path is open at a time, which is what keeps that array small enough
 * to build on every render.
 */
type TreeRow =
  | { kind: 'dimension'; id: string; row: DimensionSummary }
  | { kind: 'run'; id: string; run: RunSummary }
  | { kind: 'span'; id: string; span: SpanRow; depth: number };

function Tree({
  selection,
  onSelect,
}: {
  selection: Selection;
  onSelect: (next: Partial<Selection>) => void;
}) {
  const pivot = selection.by ?? 'session';
  const [draft, setDraft] = React.useState(selection.find ?? '');
  const find = selection.find ?? '';

  // The URL is the state; the input is a draft of it. Committing on a debounce
  // rather than on every keystroke keeps the history from filling with
  // half-typed words and the server from answering seven queries for one.
  React.useEffect(() => setDraft(find), [find]);
  React.useEffect(() => {
    if (draft === find) return;
    const timer = setTimeout(() => onSelect({ find: draft || undefined }), 250);
    return () => clearTimeout(timer);
  }, [draft, find, onSelect]);

  const dimensions = useInfiniteQuery({
    queryKey: ['dimensions', pivot, find],
    initialPageParam: undefined as string | undefined,
    queryFn: async ({ pageParam }) => {
      const response = await listDimension({
        path: { kind: pivot as DimensionKind },
        query: {
          search: find || undefined,
          after: pageParam,
          limit: TREE_PAGE,
        },
      });
      if (response.error) throw new Error('failed to load dimensions');
      return response.data;
    },
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    enabled: pivot !== 'span',
    refetchInterval: 15_000,
  });

  // The `span` pivot has no dimension above it: the flat span list *is* the
  // top level. It is the one view that answers "which call was slow" without
  // knowing the run first.
  const allSpans = useInfiniteQuery({
    queryKey: ['spans', 'flat', find],
    initialPageParam: undefined as string | undefined,
    queryFn: async ({ pageParam }) => {
      const response = await listSpans({
        query: {
          search: find || undefined,
          after: pageParam,
          limit: TREE_PAGE,
        },
      });
      // This route declares no failure response, so `response.error` is `never`
      // and cannot narrow `data`. The transport can still fail; check the body.
      if (!response.data) throw new Error('failed to load spans');
      return response.data;
    },
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    enabled: pivot === 'span',
    refetchInterval: 15_000,
  });

  const runs = useQuery({
    queryKey: ['runs', pivot, selection.key],
    queryFn: async () => {
      const response = await listRuns({
        query: {
          [RUN_FILTER[pivot as Exclude<Pivot, 'span'>]]: selection.key,
          limit: 100,
        },
      });
      if (response.error) throw new Error('failed to list runs');
      return response.data;
    },
    enabled: pivot !== 'span' && Boolean(selection.key),
  });

  // Spans of the open run come from the flat endpoint too, narrowed by run, so
  // there is one span shape in the panel rather than the read model's nested
  // one plus a cast.
  const runSpans = useQuery({
    queryKey: ['spans', 'run', selection.run],
    queryFn: async () => {
      const response = await listSpans({
        query: { run_id: selection.run, limit: 500 },
      });
      if (!response.data) throw new Error('failed to load spans');
      return response.data;
    },
    enabled: Boolean(selection.run),
  });

  const rows = React.useMemo<TreeRow[]>(() => {
    if (pivot === 'span') {
      return (allSpans.data?.pages ?? [])
        .flatMap((page) => page.spans)
        .map((span) => ({
          kind: 'span' as const,
          id: spanKey(span),
          span,
          depth: 0,
        }));
    }

    const out: TreeRow[] = [];
    for (const page of dimensions.data?.pages ?? []) {
      for (const row of page.rows) {
        out.push({ kind: 'dimension', id: `dim:${row.key}`, row });
        if (row.key !== selection.key) continue;
        for (const run of runs.data?.runs ?? []) {
          out.push({ kind: 'run', id: `run:${run.run_id}`, run });
          if (run.run_id !== selection.run) continue;
          out.push(...nestSpans(runSpans.data?.spans ?? [], 2));
        }
      }
    }
    return out;
  }, [
    pivot,
    allSpans.data,
    dimensions.data,
    runs.data,
    runSpans.data,
    selection.key,
    selection.run,
  ]);

  const active = pivot === 'span' ? allSpans : dimensions;
  const ungrouped = pivot === 'span' ? 0 : (dimensions.data?.pages[0]?.ungrouped_runs ?? 0);

  return (
    <div className="flex max-h-[38rem] flex-col">
      <div className="flex items-center gap-2 border-b border-border p-2">
        <Search className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        <input
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          placeholder={`Filter ${pivot}s…`}
          className="w-full bg-transparent text-sm outline-none placeholder:text-muted-foreground"
        />
        {active.isFetching ? <Spinner className="shrink-0 text-muted-foreground" /> : null}
      </div>

      {active.isError ? (
        <p className="p-4 text-sm text-muted-foreground">Could not reach the API.</p>
      ) : rows.length === 0 && !active.isLoading ? (
        <p className="p-4 text-sm text-muted-foreground">
          {find ? `No ${pivot} matches “${find}”.` : `Nothing recorded for this pivot yet.`}
        </p>
      ) : (
        <VirtualList
          items={rows}
          className="max-h-[34rem]"
          keyOf={(row) => row.id}
          onReachEnd={() => {
            if (active.hasNextPage && !active.isFetchingNextPage) void active.fetchNextPage();
          }}
          isFetchingMore={active.isFetchingNextPage}
          renderRow={(row) => <TreeNode row={row} selection={selection} onSelect={onSelect} />}
          footer={
            ungrouped > 0 ? (
              <p className="border-t border-border/60 p-3 text-xs text-muted-foreground">
                {ungrouped} run{ungrouped === 1 ? '' : 's'} carry no {pivot} —{' '}
                <Link to="/observability/runs" className="text-primary hover:underline">
                  see the runs list
                </Link>
                .
              </p>
            ) : null
          }
        />
      )}
    </div>
  );
}

/** A span's identity across runs: the id alone is only unique inside a trace. */
function spanKey(span: SpanRow): string {
  return `${span.run_id}:${span.span_id}`;
}

/** The span hierarchy, nested the way the waterfall nests it. */
function nestSpans(spans: SpanRow[], baseDepth: number): TreeRow[] {
  const children = new Map<string, SpanRow[]>();
  const known = new Set(spans.map((span) => span.span_id));
  const roots: SpanRow[] = [];
  for (const span of spans) {
    const parent = span.parent_span_id;
    if (parent && known.has(parent)) {
      children.set(parent, [...(children.get(parent) ?? []), span]);
    } else {
      roots.push(span);
    }
  }
  const byStart = (a: SpanRow, b: SpanRow) => Date.parse(a.start) - Date.parse(b.start);

  const out: TreeRow[] = [];
  const walk = (span: SpanRow, depth: number) => {
    out.push({ kind: 'span', id: spanKey(span), span, depth });
    for (const child of (children.get(span.span_id) ?? []).sort(byStart)) {
      walk(child, depth + 1);
    }
  };
  for (const root of roots.sort(byStart)) walk(root, baseDepth);
  return out;
}

function TreeNode({
  row,
  selection,
  onSelect,
}: {
  row: TreeRow;
  selection: Selection;
  onSelect: (next: Partial<Selection>) => void;
}) {
  const pivot = selection.by ?? 'session';

  if (row.kind === 'dimension') {
    const open = selection.key === row.row.key;
    const label = pivot === 'trace' ? pinchId(row.row.key) : row.row.key || '(unnamed)';
    return (
      <Row
        depth={0}
        expandable
        open={open}
        active={open && !selection.run}
        onClick={() =>
          onSelect({
            key: open ? undefined : row.row.key,
            run: undefined,
            span: undefined,
          })
        }
      >
        <span className="flex-1 truncate font-medium">{label}</span>
        <span className="shrink-0 text-xs text-muted-foreground">
          {row.row.runs} run{row.row.runs === 1 ? '' : 's'} ·{' '}
          {formatCount(row.row.input_tokens + row.row.output_tokens)} tok
        </span>
        {row.row.failed > 0 ? (
          <Badge tone="danger" className="shrink-0 px-1.5 py-0 text-[10px]">
            {row.row.failed}
          </Badge>
        ) : null}
        {row.row.running > 0 ? <Spinner className="shrink-0 text-running" /> : null}
      </Row>
    );
  }

  if (row.kind === 'run') {
    const open = selection.run === row.run.run_id;
    return (
      <Row
        depth={1}
        expandable
        open={open}
        active={open && !selection.span}
        onClick={() => onSelect({ run: open ? undefined : row.run.run_id, span: undefined })}
      >
        <StatusBadge status={row.run.status as RunStatus} />
        <span className="flex-1 truncate">{row.run.run_id}</span>
        <span className="shrink-0 text-xs tabular-nums text-muted-foreground">
          {formatDuration(row.run.duration_ms)}
        </span>
      </Row>
    );
  }

  const span = row.span;
  const selected = selection.span === span.span_id && selection.run === span.run_id;
  return (
    <Row
      depth={row.depth}
      active={selected}
      onClick={() =>
        // From the flat pivot a span carries its own run, so one click lands on
        // both levels — otherwise selecting a span would leave the message pane
        // with nothing to read.
        onSelect(selected ? { span: undefined } : { run: span.run_id, span: span.span_id })
      }
    >
      <span aria-hidden className={cn('h-1.5 w-1.5 shrink-0 rounded-full', dotFor(span))} />
      <span className="flex-1 truncate">{span.name}</span>
      {span.step_type ? (
        <span className="shrink-0 rounded bg-muted px-1 py-0.5 text-[10px] text-muted-foreground">
          {span.step_type}
        </span>
      ) : null}
      <span className="shrink-0 text-xs tabular-nums text-muted-foreground">
        {formatDuration(span.duration_ms)}
      </span>
    </Row>
  );
}

function Row({
  depth,
  open,
  expandable,
  active,
  onClick,
  children,
}: {
  depth: number;
  open?: boolean;
  expandable?: boolean;
  active?: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      style={{ paddingLeft: 8 + depth * 14 }}
      className={cn(
        'flex w-full items-center gap-1.5 border-b border-border/30 py-1.5 pr-3 text-left text-sm transition-colors',
        active ? 'bg-accent text-accent-foreground' : 'hover:bg-accent/40',
      )}
    >
      {expandable ? (
        open ? (
          <ChevronDown className="h-3 w-3 shrink-0 text-muted-foreground" />
        ) : (
          <ChevronRight className="h-3 w-3 shrink-0 text-muted-foreground" />
        )
      ) : (
        <span className="w-3 shrink-0" />
      )}
      {children}
    </button>
  );
}

function dotFor(span: SpanRow): string {
  if (span.status.status === 'error') return 'bg-danger';
  if (span.step_type) return 'bg-span-step';
  if (span.name === 'run') return 'bg-span-run';
  if (span.operation === 'invoke_agent') return 'bg-span-agent';
  if (span.operation === 'execute_tool') return 'bg-span-tool';
  return 'bg-span-llm';
}

// ── Messages ─────────────────────────────────────────────────────────────────

function Messages({
  selection,
  onSelect,
}: {
  selection: Selection;
  onSelect: (next: Partial<Selection>) => void;
}) {
  const [draft, setDraft] = React.useState(selection.q ?? '');
  const q = selection.q ?? '';

  React.useEffect(() => setDraft(q), [q]);
  React.useEffect(() => {
    if (draft === q) return;
    const timer = setTimeout(() => onSelect({ q: draft || undefined }), 250);
    return () => clearTimeout(timer);
  }, [draft, q, onSelect]);

  const events = useInfiniteQuery({
    queryKey: ['events', selection.run, selection.span, q],
    initialPageParam: undefined as number | undefined,
    queryFn: async ({ pageParam }) => {
      const response = await getRunEvents({
        path: { run_id: selection.run! },
        query: {
          after: pageParam,
          limit: EVENT_PAGE,
          q: q || undefined,
          span_id: selection.span,
        },
      });
      if (response.error) throw new Error('failed to load events');
      return response.data;
    },
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    enabled: Boolean(selection.run),
    refetchInterval: 5_000,
  });

  const pages = events.data?.pages ?? [];
  const rows = React.useMemo(() => pages.flatMap((page) => page.events), [pages]);
  const scanned = pages.reduce((total, page) => total + page.scanned, 0);

  // A filter is applied per page, so a page can come back empty while the run
  // still holds matches further along. Without this the list would stop at the
  // first page that happened to contain none.
  React.useEffect(() => {
    if (rows.length === 0 && events.hasNextPage && !events.isFetchingNextPage) {
      void events.fetchNextPage();
    }
  }, [rows.length, events]);

  if (!selection.run) {
    return (
      <div className="p-6">
        <EmptyState
          title="Nothing selected"
          hint="Pick a run in the tree to see its messages. Selecting a span narrows them to that span."
        />
      </div>
    );
  }

  return (
    <div className="flex max-h-[38rem] flex-col">
      <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border p-2">
        <span className="shrink-0 text-xs text-muted-foreground">
          {rows.length} of {scanned} read
          {selection.span ? ` · span ${shortId(selection.span)}` : ''}
          {events.hasNextPage ? ' · more' : ''}
        </span>
        <div className="flex min-w-40 flex-1 items-center gap-2 justify-self-end">
          <Search className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
          <input
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            placeholder="Search messages…"
            className="w-full bg-transparent text-sm outline-none placeholder:text-muted-foreground"
          />
          {events.isFetching ? <Spinner className="shrink-0 text-muted-foreground" /> : null}
        </div>
      </div>

      {rows.length === 0 && !events.isFetching ? (
        <p className="p-6 text-center text-sm text-muted-foreground">
          {q ? `Nothing in this run matches “${q}”.` : 'No messages for this selection.'}
        </p>
      ) : (
        <VirtualList
          items={rows}
          className="max-h-[34rem]"
          estimateSize={30}
          keyOf={(event) => event.metadata.message_id}
          onReachEnd={() => {
            if (events.hasNextPage && !events.isFetchingNextPage) void events.fetchNextPage();
          }}
          isFetchingMore={events.isFetchingNextPage}
          renderRow={(event) => <MessageRow event={event} />}
        />
      )}
    </div>
  );
}

function MessageRow({ event }: { event: RecordedEvent }) {
  const [open, setOpen] = React.useState(false);
  const metadata = event.metadata;
  const type = String(event.event_type);
  const tone = type.endsWith('.failed')
    ? 'text-danger'
    : type.endsWith('.completed')
      ? 'text-success'
      : 'text-foreground';

  return (
    <div className="border-b border-border/20 px-3 py-1.5 last:border-b-0 hover:bg-accent/30">
      <button
        type="button"
        onClick={() => setOpen((value) => !value)}
        className="flex w-full items-center gap-3 text-left"
      >
        <span className="w-20 shrink-0 text-xs tabular-nums text-muted-foreground">
          {formatTime(metadata.occurred_at)}
        </span>
        <span className={cn('w-36 shrink-0 truncate text-sm font-medium', tone)}>{type}</span>
        <span className="flex-1 truncate text-xs text-muted-foreground">
          {preview(event.data as Record<string, unknown>)}
        </span>
        {metadata.agent_id ? (
          <span className="shrink-0 text-[10px] text-muted-foreground">{metadata.agent_id}</span>
        ) : null}
      </button>

      {open ? (
        <div className="mt-2 flex flex-col gap-2 pl-20">
          {/* The correlation ids, at the level they matter: one message. */}
          <div className="flex flex-wrap items-center gap-2 text-[10px] text-muted-foreground">
            <span>trace</span>
            <IdChip value={shortId(metadata.trace_id)} full={metadata.trace_id} label="trace" />
            <span>span</span>
            <IdChip value={shortId(metadata.span_id)} full={metadata.span_id} label="span" />
            {metadata.parent_span_id ? (
              <>
                <span>parent</span>
                <IdChip
                  value={shortId(metadata.parent_span_id)}
                  full={metadata.parent_span_id}
                  label="parent span"
                />
              </>
            ) : null}
            {metadata.workflow_id ? (
              <>
                <span>workflow</span>
                <IdChip value={metadata.workflow_id} full={metadata.workflow_id} />
              </>
            ) : null}
            <span>correlation</span>
            <IdChip value={shortId(metadata.correlation_id)} full={metadata.correlation_id} />
            <span>causation</span>
            <IdChip value={shortId(metadata.causation_id)} full={metadata.causation_id} />
          </div>
          <pre className="id max-h-64 overflow-auto whitespace-pre-wrap rounded bg-muted p-2 text-muted-foreground">
            {JSON.stringify(event.data, null, 2)}
          </pre>
        </div>
      ) : null}
    </div>
  );
}

function preview(data: Record<string, unknown>): string {
  const entries = Object.entries(data ?? {});
  if (entries.length === 0) return '—';
  return entries
    .slice(0, 4)
    .map(([key, value]) => `${key}=${short(value)}`)
    .join('  ');
}

function short(value: unknown): string {
  if (typeof value === 'string') return value.length > 20 ? `${value.slice(0, 20)}…` : value;
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  return Array.isArray(value) ? `[${value.length}]` : '{…}';
}
