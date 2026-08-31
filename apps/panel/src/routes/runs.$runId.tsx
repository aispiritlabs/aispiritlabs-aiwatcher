import * as React from 'react';
import { createFileRoute } from '@tanstack/react-router';
import { useQuery, useQueryClient } from '@tanstack/react-query';

import { getRun } from '@/api/generated/sdk.gen';
import type { RunStatus } from '@/api/generated/types.gen';
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
import { EventFeed } from '@/components/event-feed';
import { Waterfall, type Span } from '@/components/waterfall';
import { openRunStream, type LiveEventFrame, type StreamPhase } from '@/lib/live';
import { formatAge, formatCount, formatDuration, shortId } from '@/lib/utils';

export const Route = createFileRoute('/runs/$runId')({
  component: RunPage,
});

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

  const startCheckpoint = query.data?.summary.last_checkpoint;
  const isRunning = query.data?.summary.status === 'running';

  React.useEffect(() => {
    if (startCheckpoint === undefined) return undefined;

    const close = openRunStream(runId, startCheckpoint, {
      onEvent: (frame) => {
        setLiveEvents((previous) => {
          // The server never resends within one connection, but a reconnect can
          // overlap by a frame. Keyed by checkpoint, which is unique per event.
          if (previous.some((seen) => seen.checkpoint === frame.checkpoint)) return previous;
          const next = [...previous, frame];
          // Bound the DOM: a streaming run emits thousands of chunk events and
          // nobody scrolls back through them.
          return next.length > 2000 ? next.slice(-2000) : next;
        });

        // A closing event means new spans exist. Refetch rather than trying to
        // assemble the waterfall in the browser — the projector already does it.
        if (frame.event_type.endsWith('.completed') || frame.event_type.endsWith('.failed')) {
          void queryClient.invalidateQueries({ queryKey: ['run', runId] });
        }
      },
      onPhase: setPhase,
      onResync: setResyncedFrom,
    });
    return close;
  }, [runId, startCheckpoint, queryClient]);

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
            {liveEvents.length} streamed since this page opened
          </span>
        </CardHeader>
        <CardContent className="p-0">
          <EventFeed events={liveEvents} autoScroll={isRunning} />
        </CardContent>
      </Card>
    </div>
  );
}
