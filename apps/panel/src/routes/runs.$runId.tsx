import * as React from 'react';
import { createFileRoute } from '@tanstack/react-router';
import { useInfiniteQuery, useQuery, useQueryClient } from '@tanstack/react-query';

import { getRun, getRunEvents } from '@/api/generated/sdk.gen';
import type { RecordedEvent, RunStatus } from '@/api/generated/types.gen';
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  EmptyState,
  IdChip,
  Stat,
} from '@/components/ui/primitives';
import { StatusBadge, StreamBadge } from '@/components/status-badge';
import { EventFeed, type EventFeedEvent } from '@/components/event-feed';
import { Waterfall, type Span } from '@/components/waterfall';
import { openRunStream, type LiveEventFrame, type StreamPhase } from '@/lib/live';
import { formatAge, formatCount, formatDuration, shortId } from '@/lib/utils';

export const Route = createFileRoute('/runs/$runId')({
  component: RunPage,
});

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
function RunPage() {
  const { runId } = Route.useParams();
  const queryClient = useQueryClient();

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
        </div>
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
        <CardContent className="grid grid-cols-2 gap-6 p-4 sm:grid-cols-3 lg:grid-cols-6">
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
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Trace</CardTitle>
        </CardHeader>
        <CardContent className="p-0">
          <Waterfall spans={spans as unknown as Span[]} />
        </CardContent>
      </Card>

      <Card>
        <CardHeader className="flex-row items-center justify-between">
          <CardTitle>Events</CardTitle>
          <span className="text-xs text-muted-foreground">
            {events.length} total · {currentLiveEvents.length} received live
            {history.isFetchingNextPage ? ' · loading history' : ''}
          </span>
        </CardHeader>
        <CardContent className="p-0">
          {history.isLoading ? (
            <p className="p-6 text-center text-sm text-muted-foreground">
              Loading event history…
            </p>
          ) : history.isError ? (
            <p className="p-6 text-center text-sm text-danger">
              Event history could not be loaded. New live events will still appear here.
            </p>
          ) : (
            <EventFeed events={events} autoScroll={isRunning} />
          )}
        </CardContent>
      </Card>
    </div>
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

function mergeEvents(
  historical: EventFeedEvent[],
  live: LiveEventFrame[],
): EventFeedEvent[] {
  const byCheckpoint = new Map<string, EventFeedEvent>();
  for (const event of historical) byCheckpoint.set(event.checkpoint, event);
  for (const event of live) byCheckpoint.set(event.checkpoint, event);
  return [...byCheckpoint.values()].sort((left, right) =>
    left.checkpoint.localeCompare(right.checkpoint),
  );
}
