import * as React from 'react';
import { cn, formatDuration, shortId } from '@/lib/utils';
import { IdChip } from '@/components/ui/primitives';

/**
 * The trace waterfall.
 *
 * Spans arrive flat with `parent_span_id` pointing up. This builds the tree,
 * lays it out against the run's wall-clock window, and draws one row per span.
 *
 * Rendered here rather than fetched from Grafana on purpose: for a run that is
 * still going, the spans that exist are the ones the projector has closed, and
 * this view updates as they close. A trace store cannot show a run in progress.
 */

export interface Span {
  trace_id: string;
  span_id: string;
  parent_span_id?: string | null;
  name: string;
  kind: string;
  start: string;
  end: string;
  status: { status: string; message?: string };
  attributes: Array<[string, unknown]>;
}

interface Node {
  span: Span;
  depth: number;
  startMs: number;
  endMs: number;
}

/**
 * Which hue a bar gets.
 *
 * Read from the span's attributes where possible rather than parsed out of its
 * name: a step names itself after what it is (`knowledge_base`), so the name
 * says nothing about the family it belongs to.
 */
function subjectOf(span: Span): 'run' | 'agent' | 'llm' | 'tool' | 'step' {
  if (stepType(span)) return 'step';
  if (span.name === 'run') return 'run';
  if (span.name.startsWith('invoke_agent')) return 'agent';
  if (span.name.startsWith('execute_tool')) return 'tool';
  return 'llm';
}

/** `retriever`, `embedding`, `guardrail`… when this span is a step. */
export function stepType(span: Span): string | undefined {
  for (const [key, value] of span.attributes ?? []) {
    if (key === 'aiwatcher.span.step_type' && typeof value === 'string') return value;
  }
  return undefined;
}

const barColor: Record<string, string> = {
  run: 'bg-span-run',
  agent: 'bg-span-agent',
  llm: 'bg-span-llm',
  tool: 'bg-span-tool',
  step: 'bg-span-step',
};

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

export function Waterfall({ spans }: { spans: Span[] }) {
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
        const subject = subjectOf(node.span);

        return (
          <div
            key={node.span.span_id}
            // `minmax(0, …)` rather than a `14rem` floor: the id chip and the
            // step badge are unshrinkable, so a floor lets the track refuse to
            // go below its content on a narrow viewport. Precautionary — the
            // layout measured clean at 1186px — but the failure it prevents is
            // the duration column sliding off the right edge.
            className="group grid grid-cols-[minmax(0,20rem)_1fr_4.5rem] items-center gap-3 overflow-hidden border-b border-border/50 px-3 py-1.5 last:border-b-0 hover:bg-accent/40"
          >
            <div
              className="flex min-w-0 items-center gap-2"
              style={{ paddingLeft: `${node.depth * 14}px` }}
            >
              <span
                className={cn('h-2 w-2 shrink-0 rounded-full', barColor[subject])}
                aria-hidden
              />
              <span className="truncate text-sm" title={node.span.name}>
                {node.span.name}
              </span>
              {/* The kind as a label, so the family reads without colour. */}
              {stepType(node.span) ? (
                <span className="shrink-0 rounded bg-muted px-1 py-0.5 text-[10px] text-muted-foreground">
                  {stepType(node.span)}
                </span>
              ) : null}
              <span className="shrink-0">
                <IdChip value={shortId(node.span.span_id)} full={node.span.span_id} label="span" />
              </span>
            </div>

            <div className="relative h-5" title={node.span.status.message ?? node.span.name}>
              <div
                className={cn(
                  'absolute top-1 h-3 rounded-sm',
                  failed ? 'bg-danger' : barColor[subject],
                  'opacity-80 group-hover:opacity-100',
                )}
                style={{ left: `${offset}%`, width: `${width}%` }}
              />
            </div>

            <span className="text-right text-xs tabular-nums text-muted-foreground">
              {formatDuration(node.endMs - node.startMs)}
            </span>
          </div>
        );
      })}
    </div>
  );
}
