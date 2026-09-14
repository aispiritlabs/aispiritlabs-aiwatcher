import { useInfiniteQuery, useQuery, useQueryClient } from '@tanstack/react-query';
import { getRouteApi } from '@tanstack/react-router';
import * as React from 'react';

import { getRun, getRunEvents } from '@/api/generated/sdk.gen';
import type { RecordedEvent, RunStatus } from '@/api/generated/types.gen';
import { EventFeed, type EventFeedEvent } from '@/features/observability/components/event-feed';
import { SpanDetail } from '@/features/observability/components/span-detail';
import { Waterfall } from '@/features/observability/components/waterfall';
import type { Span } from '@/features/observability/lib/span-facts';
import { StatusBadge, StreamBadge } from '@/shared/components/status-badge';
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  EmptyState,
  IdChip,
  Stat,
} from '@/shared/components/ui/primitives';
import { openRunStream, type LiveEventFrame, type StreamPhase } from '@/shared/lib/live';
import { formatAge, formatCount, formatDuration, formatUsd, shortId } from '@/shared/lib/utils';

const routeApi = getRouteApi('/runs/$runId');

const EVENT_PAGE_SIZE = 1_000;

/**
 * The handoff from history to live is the whole trick on this page.
 *
 * 1. Fetch the run. Its `summary.last_checkpoint` is the newest event folded
 *    into what we just rendered.
 * 2. Open the stream *at that checkpoint*. The server replays anything that
 *    landed between the fetch and the connection, then says `caught_up`.
 * 3. From there it is live, and a dropped connection resumes itself via
 *    `Last-Event-ID`.
 *
 * Skipping step 2's cursor is the classic bug: the page looks fine and quietly
 * misses whatever happened during the round trip.
 */
export function RunPage() {
  const { runId } = routeApi.useParams();
  const { span: selectedSpanId, attrs = false, chunks = true } = routeApi.useSearch();
  const navigate = routeApi.useNavigate();
  const queryClient = useQueryClient();

  // Clicking the open span closes it: the row is the control that opened it,
  // and a control that only ever opens leaves the reader hunting for a Close.
  const selectSpan = React.useCallback(
    (spanId: string | undefined) =>
      void navigate({
        search: (previous) => ({
          ...previous,
          span: spanId === previous.span ? undefined : spanId,
        }),
        replace: true,
      }),
    [navigate],
  );

  const [liveEvents, setLiveEvents] = React.useState<LiveEventFrame[]>([]);
  const [phase, setPhase] = React.useState<StreamPhase>('catching-up');
  const [resyncedFrom, setResyncedFrom] = React.useState<string | null>(null);

  const query = useQuery({
    queryKey: ['run', runId],
    queryFn: async () => {
      const response = await getRun({ path: { run_id: runId } });
      if (response.error) throw new Error(`run ${runId} not found`);
      return response.data;
    },
  });

  // History is the durable audit log, not the live buffer. Load it page by
  // page until the whole run is present; EventFeed virtualises the result so a
  // chatty trace still mounts only the rows on screen.
  const history = useInfiniteQuery({
    queryKey: ['run-events', runId],
    initialPageParam: undefined as number | undefined,
    queryFn: async ({ pageParam }) => {
      const response = await getRunEvents({
        path: { run_id: runId },
        query: { after: pageParam, limit: EVENT_PAGE_SIZE },
      });
      if (response.error) throw new Error(`failed to load events for run ${runId}`);
      return response.data;
    },
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });

  React.useEffect(() => {
    if (history.hasNextPage && !history.isFetchingNextPage) {
      void history.fetchNextPage();
    }
  }, [history.hasNextPage, history.isFetchingNextPage, history.fetchNextPage]);

  const canStream = query.data !== undefined;
  const isRunning = query.data?.summary.status === 'running';

  React.useEffect(() => {
    setLiveEvents([]);
    setPhase('catching-up');
    setResyncedFrom(null);
  }, [runId]);

  React.useEffect(() => {
    if (!canStream) return undefined;

    // Deliberately captured only when the stream first opens. Refetching the
    // summary advances its cursor, but must not tear down and reopen the live
    // connection for every event.
    const startCheckpoint = query.data?.summary.last_checkpoint;
    let refreshTimer: ReturnType<typeof setTimeout> | undefined;

    const close = openRunStream(runId, startCheckpoint, {
      onEvent: (frame) => {
        setLiveEvents((previous) => {
          // The server never resends within one connection, but a reconnect can
          // overlap by a frame. Keyed by checkpoint, which is unique per event.
          if (previous.some((seen) => seen.checkpoint === frame.checkpoint)) return previous;
          return [...previous, frame];
        });

        // Counts and last activity change for every message, while spans appear
        // on closing messages. Coalesce chatty token streams and let the
        // projector remain the one place that assembles both.
        if (!refreshTimer) {
          refreshTimer = setTimeout(() => {
            refreshTimer = undefined;
            void queryClient.invalidateQueries({ queryKey: ['run', runId] });
          }, 250);
        }
      },
      onPhase: setPhase,
      onResync: setResyncedFrom,
    });
    return () => {
      if (refreshTimer) clearTimeout(refreshTimer);
      close();
    };
  }, [runId, canStream, queryClient]);

  if (query.isError) {
    return (
      <EmptyState
        title={`No run ${runId}`}
        hint="It may have been evicted from the read model. Its trace is still in the trace store."
      />
    );
  }
  if (!query.data) {
    return <p className="text-sm text-muted-foreground">Loading…</p>;
  }

  const { summary, spans } = query.data;
  const historicalEvents = (history.data?.pages ?? []).flatMap((page) =>
    page.events.map(toEventFeedEvent),
  );
  const currentLiveEvents = liveEvents.filter((event) => event.run_id === runId);
  const events = mergeEvents(historicalEvents, currentLiveEvents);

  // The API types spans as bare objects because `CompletedSpan` is the trace
  // store's shape rather than one of the read model's own.
  const runSpans = spans as unknown as Span[];
  const selectedSpan = runSpans.find((span) => span.span_id === selectedSpanId);
  const spanEvents = selectedSpan
    ? events.filter((event) => event.span_id === selectedSpan.span_id)
    : [];
  // A streaming call is mostly `llm.chunk`, which is why the feed can drop it —
  // and why it never does so without being asked.
  const feedEvents = chunks ? events : events.filter((event) => event.event_type !== 'llm.chunk');

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="flex flex-col gap-1">
          <div className="flex items-center gap-3">
            <h1 className="text-lg font-semibold">{summary.run_id}</h1>
            <StatusBadge status={summary.status as RunStatus} lastEventAt={summary.last_event_at} />
            {isRunning ? <StreamBadge phase={phase} /> : null}
          </div>
          <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
            <span>trace</span>
            <IdChip value={shortId(summary.trace_id, 16)} full={summary.trace_id} label="trace" />
            {summary.conversation_id ? (
              <>
                <span>· conversation</span>
                <IdChip value={summary.conversation_id} label="conversation" />
              </>
            ) : null}
            <span>· last event</span>
            <span className="tabular-nums">{formatAge(summary.last_event_at)} ago</span>
            <span>· cursor</span>
            <IdChip value={summary.last_checkpoint} label="checkpoint" />
          </div>
          {/* Who ran it, which the six stats below cannot say. A run is
              normally one service and one agent; the case worth seeing is the
              one where it is two. */}
          <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
            {summary.agents.length > 0 ? <Named label="agent" values={summary.agents} /> : null}
            {summary.runtimes && summary.runtimes.length > 0 ? (
              <Named label="runtime" values={summary.runtimes} />
            ) : null}
            {summary.workflow ? <Named label="workflow" values={[summary.workflow]} /> : null}
            {summary.variant_id ? <Named label="variant" values={[summary.variant_id]} /> : null}
            {summary.published_by ? (
              <Named label="published by" values={[summary.published_by]} />
            ) : null}
          </div>
        </div>

        <ViewMenu
          attrs={attrs}
          chunks={chunks}
          onChange={(next) =>
            void navigate({ search: (previous) => ({ ...previous, ...next }), replace: true })
          }
        />
      </div>

      {resyncedFrom ? (
        <Card className="border-warning/40 bg-warning/5">
          <CardContent className="p-3 text-xs text-warning">
            This tab was behind further than the live buffer reaches, so the gap was replayed from
            the durable log. Nothing was lost.
          </CardContent>
        </Card>
      ) : null}

      {summary.error ? (
        <Card className="border-danger/40 bg-danger/5">
          <CardContent className="p-3 text-sm text-danger">{summary.error}</CardContent>
        </Card>
      ) : null}

      <Card>
        <CardContent className="grid grid-cols-2 gap-6 p-4 sm:grid-cols-3 lg:grid-cols-7">
          <Stat label="Duration" value={formatDuration(summary.duration_ms)} />
          <Stat label="Events" value={formatCount(summary.event_count)} />
          <Stat label="LLM calls" value={summary.llm_calls} />
          <Stat label="Tool calls" value={summary.tool_calls} />
          <Stat
            label="Tokens"
            value={formatCount(summary.input_tokens + summary.output_tokens)}
            hint={`${formatCount(summary.input_tokens)} in · ${formatCount(summary.output_tokens)} out`}
          />
          <Stat
            label="Cached"
            value={formatCount(summary.cached_tokens)}
            hint={
              summary.input_tokens > 0
                ? `${Math.round((summary.cached_tokens / summary.input_tokens) * 100)}% of input`
                : undefined
            }
          />
          {/* What the providers billed, where they said so. A run whose calls
              all stayed quiet has an unknown cost, and an unknown cost is not
              a stat worth a column of zeroes. */}
          {summary.cost_usd !== undefined && summary.cost_usd !== null ? (
            <Stat
              label="Cost"
              value={formatUsd(summary.cost_usd)}
              hint={
                (summary.costed_calls ?? 0) < summary.llm_calls
                  ? `${summary.costed_calls ?? 0} of ${summary.llm_calls} calls reported one`
                  : 'as the providers billed it'
              }
            />
          ) : null}
        </CardContent>
      </Card>

      <Card>
        <CardHeader className="flex-row items-center justify-between">
          <CardTitle>Trace</CardTitle>
          <span className="text-xs text-muted-foreground">
            {selectedSpan ? 'Selected span is on the right' : 'Select a span to read it'}
          </span>
        </CardHeader>
        <CardContent className="p-0">
          <div
            className={
              selectedSpan
                ? 'grid grid-cols-1 divide-y divide-border xl:grid-cols-[minmax(0,1fr)_28rem] xl:divide-x xl:divide-y-0'
                : ''
            }
          >
            <Waterfall
              spans={runSpans}
              selected={selectedSpan?.span_id ?? null}
              onSelect={selectSpan}
            />
            {selectedSpan ? (
              <SpanDetail
                span={selectedSpan}
                events={spanEvents}
                everything={attrs}
                onClose={() => selectSpan(undefined)}
              />
            ) : null}
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader className="flex-row items-center justify-between">
          <CardTitle>Events</CardTitle>
          <span className="text-xs text-muted-foreground">
            {feedEvents.length} shown · {events.length} total · {currentLiveEvents.length} received
            live
            {history.isFetchingNextPage ? ' · loading history' : ''}
          </span>
        </CardHeader>
        <CardContent className="p-0">
          {history.isLoading ? (
            <p className="p-6 text-center text-sm text-muted-foreground">Loading event history…</p>
          ) : history.isError ? (
            <p className="p-6 text-center text-sm text-danger">
              Event history could not be loaded. New live events will still appear here.
            </p>
          ) : (
            <EventFeed events={feedEvents} autoScroll={isRunning} />
          )}
        </CardContent>
      </Card>
    </div>
  );
}

/** One dimension of a run, named rather than left to a colour or a position. */
function Named({ label, values }: { label: string; values: string[] }) {
  return (
    <span className="flex items-center gap-1">
      <span>{label}</span>
      {values.map((value) => (
        <IdChip key={value} value={value} label={label} />
      ))}
    </span>
  );
}

/**
 * What the page shows, as a menu rather than as a preference.
 *
 * Both switches add rather than remove, and both live in the URL: a reader who
 * turns the wire on and sends the link sends the same view, which is the whole
 * reason filters are not component state here.
 */
function ViewMenu({
  attrs,
  chunks,
  onChange,
}: {
  attrs: boolean;
  chunks: boolean;
  onChange: (next: { attrs?: boolean; chunks?: boolean }) => void;
}) {
  return (
    <details className="relative shrink-0 text-xs">
      <summary className="cursor-pointer rounded border border-border px-2 py-1 text-muted-foreground hover:bg-accent hover:text-foreground">
        View
      </summary>
      <div className="absolute right-0 z-30 mt-2 grid w-72 gap-2 rounded border border-border bg-card p-3 shadow-lg">
        <Toggle
          checked={attrs}
          onChange={(value) => onChange({ attrs: value ? true : undefined })}
          label="All span attributes"
          hint="Includes the correlation ids a span carries for other systems."
        />
        <Toggle
          checked={chunks}
          onChange={(value) => onChange({ chunks: value ? undefined : false })}
          label="Token chunks"
          hint="`llm.chunk` is most of a streaming call's log by volume."
        />
      </div>
    </details>
  );
}

function Toggle({
  checked,
  onChange,
  label,
  hint,
}: {
  checked: boolean;
  onChange: (value: boolean) => void;
  label: string;
  hint: string;
}) {
  return (
    <label className="flex cursor-pointer items-start gap-2">
      <input
        type="checkbox"
        checked={checked}
        onChange={(event) => onChange(event.target.checked)}
        className="mt-0.5"
      />
      <span className="flex flex-col gap-0.5">
        <span>{label}</span>
        <span className="text-muted-foreground">{hint}</span>
      </span>
    </label>
  );
}

function toEventFeedEvent(event: RecordedEvent): EventFeedEvent {
  return {
    checkpoint: event.metadata.checkpoint,
    span_id: event.metadata.span_id,
    event_type: String(event.event_type),
    occurred_at: event.metadata.occurred_at,
    data: event.data,
  };
}

function mergeEvents(historical: EventFeedEvent[], live: LiveEventFrame[]): EventFeedEvent[] {
  const byCheckpoint = new Map<string, EventFeedEvent>();
  for (const event of historical) byCheckpoint.set(event.checkpoint, event);
  for (const event of live) byCheckpoint.set(event.checkpoint, event);
  return [...byCheckpoint.values()].sort((left, right) =>
    left.checkpoint.localeCompare(right.checkpoint),
  );
}
