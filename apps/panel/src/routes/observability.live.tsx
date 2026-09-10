import * as React from 'react';
import { Link, createFileRoute } from '@tanstack/react-router';
import { Pause, Play, Radio, Table2 } from 'lucide-react';
import { z } from 'zod';

import { AttributePicker } from '@/components/attribute-picker';
import { EventFeed } from '@/components/event-feed';
import { StreamBadge } from '@/components/status-badge';
import { Badge, Button, Card, EmptyState, Stat } from '@/components/ui/primitives';
import { DEFAULT_WINDOW_SECONDS, TimeRange, windowSearchSchema } from '@/components/time-range';
import {
  openSelectionStream,
  selectionQuery,
  type LiveEventFrame,
  type StreamPhase,
} from '@/lib/live';
import { ATTRIBUTES, attribute, type AttributeId } from '@/lib/query-builder';
import {
  attributeSearchSchema,
  liveSelectionOf,
  notLiveFilterable,
  selectionFromSearch,
  selectionToSearch,
} from '@/lib/selection-params';
import { cn, formatCount, formatTime } from '@/lib/utils';

/**
 * Watching a selection of agents work, as it happens.
 *
 * ## What this is for
 *
 * Testing an agent is a different job from reading about one that finished.
 * You run it, and you want to see it: which tools it reached for, where it
 * stalled, whether the second agent picked up what the first handed over.
 * Explore answers that afterwards, from the read model; this answers it now,
 * from the log, and the difference is that a run in progress has no row in a
 * finished-runs table to look at.
 *
 * ## Why the server does the filtering
 *
 * `/api/v1/events/stream` takes the selection as repeated parameters and
 * `Scope::Selection` applies it before anything is sent. Subscribing to
 * everything and discarding most of it here would be the same mistake as
 * filtering a list after downloading it — and worse than usual, because
 * `llm.chunk` is most of the log by volume and every one of them would cross
 * the wire to be thrown away.
 *
 * ## Why some filters are refused rather than ignored
 *
 * Model and tool are span-level facts assembled from several events (ADR_0003)
 * and status is a fold over a whole run, so an event carries none of them. A
 * selection naming one arrives here from the Query builder, where it is
 * perfectly valid, and this page says which parts it cannot follow. Passing
 * them to a stream that does not understand them would look identical to
 * nothing happening.
 *
 * ## Why the buffer is bounded and the pause is a freeze
 *
 * A busy selection produces thousands of events a minute. The feed keeps the
 * last few hundred and the counters keep totals, so memory is flat however
 * long the tab is left open. Pause stops the *rendering*, not the
 * subscription: closing the connection would mean the resume had a gap in it,
 * and a live view with a silent hole is worse than one that fell behind.
 */

const searchSchema = z.object({
  ...windowSearchSchema,
  ...attributeSearchSchema,
});

export const Route = createFileRoute('/observability/live')({
  validateSearch: searchSchema,
  component: LivePage,
});

/**
 * How much of the tail is kept.
 *
 * The panel's own memory contract, the read model's rule one layer out: this
 * is a tab somebody leaves open for an afternoon, and an unbounded array of
 * events with their payloads is the one thing here that grows without limit.
 */
const TAIL = 400;

interface Totals {
  events: number;
  /**
   * How many distinct runs have been seen — the count, not the set.
   *
   * The set that produces it lives in a ref beside this. Kept in state it
   * would be copied on every event to stay immutable, which is a copy per
   * `llm.chunk`, or mutated in place, which makes the old and new state the
   * same object and is the bug that copy exists to avoid.
   */
  runs: number;
  byType: Map<string, number>;
  byAgent: Map<string, number>;
  failures: number;
}

const emptyTotals = (): Totals => ({
  events: 0,
  runs: 0,
  byType: new Map(),
  byAgent: new Map(),
  failures: 0,
});

function LivePage() {
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const windowSeconds = search.window ?? DEFAULT_WINDOW_SECONDS;

  const selection = React.useMemo(() => selectionFromSearch(search), [search]);
  const live = React.useMemo(() => liveSelectionOf(selection), [selection]);
  // The query string is the subscription's identity. Comparing the objects by
  // reference would reopen the stream on every render; comparing them deeply
  // on every render is the same work with more code.
  const key = React.useMemo(() => selectionQuery(live), [live]);
  const ignored = React.useMemo(() => notLiveFilterable(selection), [selection]);

  const [phase, setPhase] = React.useState<StreamPhase>('catching-up');
  const [events, setEvents] = React.useState<LiveEventFrame[]>([]);
  const [totals, setTotals] = React.useState<Totals>(emptyTotals);
  const [paused, setPaused] = React.useState(false);
  const [resynced, setResynced] = React.useState(false);

  // A paused feed still counts. The pause is about what the eye can follow,
  // not about what happened — a rate that stopped moving while paused would
  // be a number about the reader rather than about the system.
  const pausedRef = React.useRef(paused);
  pausedRef.current = paused;

  // Bounded by the number of distinct runs in a selection rather than by the
  // event count, which is what makes it safe to keep for a whole afternoon.
  const seenRuns = React.useRef(new Set<string>());

  React.useEffect(() => {
    setEvents([]);
    setTotals(emptyTotals());
    seenRuns.current = new Set();
    setResynced(false);

    // No `from`: a live view starts now. Resuming from a checkpoint would
    // replay the whole retained log through a filter on every filter change,
    // which is a different feature — that one is Explore.
    const close = openSelectionStream(live, undefined, {
      onEvent: (event) => {
        seenRuns.current.add(event.run_id);
        setTotals((previous) => {
          const next: Totals = {
            events: previous.events + 1,
            runs: seenRuns.current.size,
            byType: new Map(previous.byType),
            byAgent: new Map(previous.byAgent),
            failures: previous.failures + (event.event_type.endsWith('.failed') ? 1 : 0),
          };
          next.byType.set(event.event_type, (next.byType.get(event.event_type) ?? 0) + 1);
          const agent = event.agent_id ?? event.service;
          next.byAgent.set(agent, (next.byAgent.get(agent) ?? 0) + 1);
          return next;
        });

        if (pausedRef.current) return;
        setEvents((previous) => {
          const next = [...previous, event];
          return next.length > TAIL ? next.slice(next.length - TAIL) : next;
        });
      },
      onPhase: setPhase,
      onResync: () => setResynced(true),
    });

    return close;
    // `key` is the selection; `live` is the object it was computed from and
    // changes with it. Depending on the string is what makes an identical
    // selection not reopen the connection.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  const watching = Object.values(selection).some((values) => (values ?? []).length > 0);

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold">Live</h1>
          <p className="max-w-3xl text-sm text-muted-foreground">
            The log as it is written, narrowed by the server to the agents and runtimes you pick.
            Nothing here is folded or retained — it is what a producer has just said.
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-3">
          <TimeRange
            value={windowSeconds}
            onChange={(seconds) =>
              void navigate({ search: (previous) => ({ ...previous, window: seconds }) })
            }
          />
          {/* Labelled, because the area layout draws one of these too and the
              two are different connections: that one is the whole log, kept
              open so the other tabs refresh, and this one is the selection.
              Unlabelled they read as one badge rendered twice, and a filtered
              stream that dropped while the global one held would look fine. */}
          <span className="flex items-center gap-1.5 text-xs text-muted-foreground">
            this selection
            <StreamBadge phase={phase} />
          </span>
          <Button variant="outline" onClick={() => setPaused((value) => !value)} className="gap-2">
            {paused ? <Play className="h-3.5 w-3.5" /> : <Pause className="h-3.5 w-3.5" />}
            {paused ? 'Resume' : 'Pause'}
          </Button>
        </div>
      </div>

      <div className="grid gap-4 lg:grid-cols-[minmax(15rem,19rem)_1fr]">
        <Card className="overflow-hidden">
          <div className="flex items-center justify-between border-b border-border p-2">
            <span className="text-xs font-medium">Watching</span>
            <Link
              to="/observability/query"
              search={{ ...selectionToSearch(selection), window: windowSeconds, mode: 'build' }}
              className="flex items-center gap-1 text-[11px] text-muted-foreground hover:text-foreground"
            >
              <Table2 className="h-3 w-3" />
              Aggregate this
            </Link>
          </div>

          {/* The period is what the *value lists* are read over — which agents
              have run lately — and not a filter on the stream, which is by
              definition now. Said here rather than left to be inferred from a
              control that means something else three tabs away. */}
          <p className="border-b border-border/40 px-2 py-1.5 text-[11px] leading-relaxed text-muted-foreground">
            The period below decides which agents and runtimes are offered, not what the stream
            carries — a live tail is always now.
          </p>

          <div className="max-h-[30rem] overflow-y-auto">
            <AttributePicker
              attributes={ATTRIBUTES}
              value={selection}
              onChange={(next) =>
                void navigate({
                  search: (previous) => ({ ...previous, ...selectionToSearch(next) }),
                })
              }
              windowSeconds={windowSeconds}
              unavailable={(candidate) =>
                LIVE_FILTERABLE.includes(candidate.id)
                  ? undefined
                  : 'An event does not carry this — it is assembled from several of them, or folded over a whole run. Pick it in Query instead.'
              }
            />
          </div>
        </Card>

        <div className="flex flex-col gap-4">
          {ignored.length > 0 ? (
            <Card className="border-warning/40 p-3">
              <p className="text-xs text-foreground">
                Following the rest of this selection.{' '}
                {ignored.map((id) => attribute(id)?.label ?? id).join(', ')}{' '}
                {ignored.length === 1 ? 'is not something' : 'are not things'} an event carries, so{' '}
                {ignored.length === 1 ? 'it is' : 'they are'} not narrowing the stream.
              </p>
            </Card>
          ) : null}

          {resynced ? (
            <Card className="border-warning/40 p-3 text-xs text-muted-foreground">
              The connection dropped far enough behind that the durable log was read to catch up.
              Nothing is missing, but the tail below is not continuous.
            </Card>
          ) : null}

          <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
            <Stat label="Events" value={formatCount(totals.events)} />
            <Stat label="Runs seen" value={formatCount(totals.runs)} />
            <Stat label="Failures" value={formatCount(totals.failures)} />
            <Stat label="Event types" value={formatCount(totals.byType.size)} />
          </div>

          <div className="grid gap-4 md:grid-cols-2">
            <Breakdown title="By agent" counts={totals.byAgent} empty="Nothing yet." />
            <Breakdown title="By event" counts={totals.byType} empty="Nothing yet." />
          </div>

          <Card className="overflow-hidden">
            <div className="flex items-center justify-between border-b border-border px-3 py-1.5 text-xs text-muted-foreground">
              <span className="flex items-center gap-1.5">
                <Radio className={cn('h-3 w-3', phase === 'live' && !paused && 'text-success')} />
                {paused ? 'Frozen — still counting' : 'The last few hundred events'}
              </span>
              {events.length > 0 ? (
                <span>{formatTime(events[events.length - 1]?.occurred_at)}</span>
              ) : null}
            </div>
            {events.length === 0 ? (
              <EmptyState
                title={watching ? 'Nothing from this selection yet' : 'Watching everything'}
                hint={
                  watching
                    ? 'The stream is open. Run the agent, and its events land here as they are published.'
                    : 'Pick an agent or a runtime on the left to narrow it, or leave it open and watch the whole system.'
                }
              />
            ) : (
              <EventFeed events={events} autoScroll={!paused} />
            )}
          </Card>
        </div>
      </div>
    </div>
  );
}

/** The attributes an event actually carries. The rest are Query's business. */
const LIVE_FILTERABLE: AttributeId[] = ['agent', 'runtime', 'workflow', 'session'];

/**
 * A running tally, biggest first.
 *
 * Deliberately not a chart. What is being asked while an agent runs is "is it
 * doing the thing", and a bar that redraws twenty times a second answers that
 * worse than a number does.
 */
function Breakdown({
  title,
  counts,
  empty,
}: {
  title: string;
  counts: Map<string, number>;
  empty: string;
}) {
  const rows = [...counts.entries()].sort(([, a], [, b]) => b - a).slice(0, 8);
  const top = rows[0]?.[1] ?? 1;

  return (
    <Card className="overflow-hidden">
      <div className="border-b border-border px-3 py-1.5 text-xs font-medium">{title}</div>
      {rows.length === 0 ? (
        <p className="px-3 py-2 text-xs text-muted-foreground">{empty}</p>
      ) : (
        <div className="flex flex-col">
          {rows.map(([label, count]) => (
            <div
              key={label}
              className="relative flex items-center justify-between px-3 py-1 text-xs"
            >
              <div
                className="absolute inset-y-0 left-0 bg-accent/40"
                style={{ width: `${Math.max(4, (count / top) * 100)}%` }}
                aria-hidden
              />
              <span className="relative truncate" title={label}>
                {label}
              </span>
              <Badge className="relative px-1.5 py-0 text-[10px] tabular-nums">
                {formatCount(count)}
              </Badge>
            </div>
          ))}
        </div>
      )}
    </Card>
  );
}
