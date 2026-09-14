import * as React from 'react';

import {
  factsOf,
  stepTypeOf,
  type Span,
  type SpanFamily,
} from '@/features/observability/lib/span-facts';
import { cn, formatCount, formatDuration } from '@/shared/lib/utils';

/**
 * The trace waterfall.
 *
 * Spans arrive flat with `parent_span_id` pointing up. This builds the tree,
 * lays it out against the run's wall-clock window, and draws one row per span.
 *
 * Rendered here rather than read back from the trace store on purpose: for a run
 * that is still going, the spans that exist are the ones the projector has closed,
 * and this view updates as they close. A trace store cannot show a run in progress.
 *
 * A row is a button, because the interesting half of a span is everything a bar
 * cannot draw — which model, on which settings, at what cost in tokens.
 * Selecting one opens `SpanDetail`, and the selection lives in the URL, so a
 * pasted link lands the next reader on the same span.
 */

/** One hue per family, so the tree reads without a legend. */
export const familyColor: Record<SpanFamily, string> = {
  run: 'bg-span-run',
  agent: 'bg-span-agent',
  llm: 'bg-span-llm',
  tool: 'bg-span-tool',
  step: 'bg-span-step',
};

interface Node {
  span: Span;
  depth: number;
  startMs: number;
  endMs: number;
}

/**
 * Depth-first flatten, parents before children, siblings by start time.
 *
 * Orphans — a span whose parent was evicted or never closed — are treated as
 * roots rather than dropped. Losing a span from the view because its parent is
 * missing would hide exactly the case worth looking at.
 */
function flatten(spans: Span[]): Node[] {
  const byId = new Map(spans.map((span) => [span.span_id, span]));
  const children = new Map<string, Span[]>();
  const roots: Span[] = [];

  for (const span of spans) {
    const parent = span.parent_span_id;
    if (parent && byId.has(parent)) {
      const bucket = children.get(parent);
      if (bucket) bucket.push(span);
      else children.set(parent, [span]);
    } else {
      roots.push(span);
    }
  }

  const byStart = (a: Span, b: Span) => Date.parse(a.start) - Date.parse(b.start);
  const out: Node[] = [];
  const visit = (span: Span, depth: number) => {
    out.push({
      span,
      depth,
      startMs: Date.parse(span.start),
      endMs: Date.parse(span.end),
    });
    for (const child of (children.get(span.span_id) ?? []).sort(byStart)) {
      visit(child, depth + 1);
    }
  };
  for (const root of roots.sort(byStart)) visit(root, 0);
  return out;
}

export function Waterfall({
  spans,
  selected,
  onSelect,
}: {
  spans: Span[];
  selected?: string | null;
  onSelect?: (spanId: string) => void;
}) {
  const nodes = React.useMemo(() => flatten(spans), [spans]);

  if (nodes.length === 0) {
    return (
      <p className="p-6 text-center text-sm text-muted-foreground">
        No spans yet. A span is written when its end event arrives, so a run in flight shows its
        completed steps only.
      </p>
    );
  }

  const first = Math.min(...nodes.map((node) => node.startMs));
  const last = Math.max(...nodes.map((node) => node.endMs));
  // A run whose spans all land in the same millisecond would divide by zero.
  const total = Math.max(last - first, 1);

  return (
    <div className="flex flex-col">
      {nodes.map((node) => {
        const offset = ((node.startMs - first) / total) * 100;
        const width = Math.max(((node.endMs - node.startMs) / total) * 100, 0.5);
        const failed = node.span.status.status === 'error';
        const facts = factsOf(node.span);
        const isSelected = selected === node.span.span_id;
        const step = stepTypeOf(node.span);

        return (
          <button
            key={node.span.span_id}
            type="button"
            onClick={() => onSelect?.(node.span.span_id)}
            aria-pressed={isSelected}
            title={node.span.status.message ?? node.span.name}
            // `minmax(0, …)` rather than a `14rem` floor: the step badge and
            // the token count are unshrinkable, so a floor lets the track
            // refuse to go below its content on a narrow viewport.
            className={cn(
              'group grid w-full grid-cols-[minmax(0,18rem)_1fr_auto_4.5rem] items-center gap-3 overflow-hidden border-b border-border/50 px-3 py-1.5 text-left last:border-b-0 hover:bg-accent/40',
              isSelected && 'bg-accent/60',
            )}
          >
            <span
              className="flex min-w-0 items-center gap-2"
              style={{ paddingLeft: `${node.depth * 14}px` }}
            >
              <span
                className={cn('h-2 w-2 shrink-0 rounded-full', familyColor[facts.family])}
                aria-hidden
              />
              <span className="truncate text-sm" title={node.span.name}>
                {node.span.name}
              </span>
              {/* The kind as a label, so the family reads without colour. */}
              {step ? (
                <span className="shrink-0 rounded bg-muted px-1 py-0.5 text-[10px] text-muted-foreground">
                  {step}
                </span>
              ) : null}
            </span>

            <span className="relative block h-5">
              <span
                className={cn(
                  'absolute top-1 block h-3 rounded-sm',
                  failed ? 'bg-danger' : familyColor[facts.family],
                  'opacity-80 group-hover:opacity-100',
                )}
                style={{ left: `${offset}%`, width: `${width}%` }}
              />
            </span>

            {/* What a bar cannot say and a reader is counting anyway. */}
            <span className="shrink-0 text-right text-[11px] tabular-nums text-muted-foreground">
              {facts.tokens
                ? `${formatCount(facts.tokens.input)} → ${formatCount(facts.tokens.output)}`
                : ''}
            </span>

            <span className="text-right text-xs tabular-nums text-muted-foreground">
              {formatDuration(node.endMs - node.startMs)}
            </span>
          </button>
        );
      })}
    </div>
  );
}

export type { Span } from '@/features/observability/lib/span-facts';
