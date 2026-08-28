import * as React from 'react';
import { cn, formatTime, shortId } from '@/lib/utils';
import { IdChip } from '@/components/ui/primitives';
import type { LiveEventFrame } from '@/lib/live';

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

export function EventFeed({
  events,
  autoScroll = true,
}: {
  events: LiveEventFrame[];
  autoScroll?: boolean;
}) {
  const bottom = React.useRef<HTMLDivElement>(null);

  React.useEffect(() => {
    if (autoScroll) bottom.current?.scrollIntoView({ block: 'end' });
  }, [events.length, autoScroll]);

  if (events.length === 0) {
    return <p className="p-6 text-center text-sm text-muted-foreground">No events yet.</p>;
  }

  return (
    <div className="max-h-[28rem] overflow-y-auto">
      <table className="w-full text-left text-sm">
        <thead className="sticky top-0 bg-card">
          <tr className="border-b border-border text-xs uppercase tracking-wide text-muted-foreground">
            <th className="px-3 py-2 font-medium">Time</th>
            <th className="px-3 py-2 font-medium">Event</th>
            <th className="px-3 py-2 font-medium">Span</th>
            <th className="px-3 py-2 font-medium">Payload</th>
          </tr>
        </thead>
        <tbody>
          {events.map((event) => (
            <tr
              key={`${event.checkpoint}-${event.span_id}`}
              className="border-b border-border/40 align-top last:border-b-0 hover:bg-accent/40"
            >
              <td className="whitespace-nowrap px-3 py-1.5 text-xs tabular-nums text-muted-foreground">
                {formatTime(event.occurred_at)}
              </td>
              <td
                className={cn(
                  'whitespace-nowrap px-3 py-1.5 font-medium',
                  toneFor(event.event_type),
                )}
              >
                {event.event_type}
              </td>
              <td className="px-3 py-1.5">
                <IdChip value={shortId(event.span_id)} full={event.span_id} label="span" />
              </td>
              <td className="px-3 py-1.5">
                <Payload data={event.data} />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <div ref={bottom} />
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
