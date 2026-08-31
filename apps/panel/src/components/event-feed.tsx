import * as React from 'react';
import { cn, formatTime, shortId } from '@/lib/utils';
import { IdChip } from '@/components/ui/primitives';
import { VirtualList } from '@/components/virtual-list';

/**
 * The raw event feed.
 *
 * Deliberately close to the wire: this is where someone goes when the
 * waterfall does not explain what happened, so it shows the correlation ids and
 * the payload rather than a friendlier summary.
 */

const toneFor = (eventType: string) => {
  if (eventType.endsWith('.failed')) return 'text-danger';
  if (eventType.endsWith('.completed')) return 'text-success';
  if (eventType === 'llm.chunk') return 'text-muted-foreground';
  return 'text-foreground';
};

/** The common fields shared by durable history and a live stream frame. */
export interface EventFeedEvent {
  checkpoint: string;
  span_id: string;
  event_type: string;
  occurred_at: string;
  data: Record<string, unknown>;
}

export function EventFeed({
  events,
  autoScroll = true,
}: {
  events: EventFeedEvent[];
  autoScroll?: boolean;
}) {
  if (events.length === 0) {
    return <p className="p-6 text-center text-sm text-muted-foreground">No events yet.</p>;
  }

  return (
    <div className="overflow-x-auto">
      <div className="min-w-[46rem] text-left text-sm">
        <div className="grid grid-cols-[5.5rem_minmax(10rem,0.8fr)_8rem_minmax(16rem,1.6fr)] border-b border-border text-xs uppercase tracking-wide text-muted-foreground">
          <span className="px-3 py-2 font-medium">Time</span>
          <span className="px-3 py-2 font-medium">Event</span>
          <span className="px-3 py-2 font-medium">Span</span>
          <span className="px-3 py-2 font-medium">Payload</span>
        </div>
        <VirtualList
          items={events}
          className="max-h-[28rem]"
          estimateSize={33}
          followEnd={autoScroll}
          keyOf={(event) => `${event.checkpoint}-${event.span_id}`}
          renderRow={(event) => (
            <div className="grid grid-cols-[5.5rem_minmax(10rem,0.8fr)_8rem_minmax(16rem,1.6fr)] border-b border-border/40 align-top last:border-b-0 hover:bg-accent/40">
              <span className="whitespace-nowrap px-3 py-1.5 text-xs tabular-nums text-muted-foreground">
                {formatTime(event.occurred_at)}
              </span>
              <span
                className={cn(
                  'whitespace-nowrap px-3 py-1.5 font-medium',
                  toneFor(event.event_type),
                )}
              >
                {event.event_type}
              </span>
              <span className="px-3 py-1.5">
                <IdChip value={shortId(event.span_id)} full={event.span_id} label="span" />
              </span>
              <span className="px-3 py-1.5">
                <Payload data={event.data} />
              </span>
            </div>
          )}
        />
      </div>
    </div>
  );
}

/**
 * Payloads vary wildly — an empty object for `run.started`, a token count block
 * for `llm.completed`, a text fragment for a chunk. Show the compact form and
 * let a click open the whole thing.
 */
function Payload({ data }: { data: Record<string, unknown> }) {
  const [open, setOpen] = React.useState(false);
  const entries = Object.entries(data);
  if (entries.length === 0) return <span className="text-muted-foreground">—</span>;

  if (!open) {
    const preview = entries
      .slice(0, 3)
      .map(([key, value]) => `${key}=${formatValue(value)}`)
      .join('  ');
    return (
      <button
        type="button"
        onClick={() => setOpen(true)}
        className="id text-left text-muted-foreground hover:text-foreground"
      >
        {preview}
        {entries.length > 3 ? ` +${entries.length - 3}` : ''}
      </button>
    );
  }

  return (
    <button type="button" onClick={() => setOpen(false)} className="text-left">
      <pre className="id whitespace-pre-wrap rounded bg-muted p-2 text-muted-foreground">
        {JSON.stringify(data, null, 2)}
      </pre>
    </button>
  );
}

function formatValue(value: unknown): string {
  if (typeof value === 'string') return value.length > 24 ? `${value.slice(0, 24)}…` : value;
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  return Array.isArray(value) ? `[${value.length}]` : '{…}';
}
