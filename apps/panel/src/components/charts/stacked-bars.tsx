import * as React from 'react';
import { Legend, Tooltip, TooltipRow, ChartEmpty } from './primitives';
import type { SeriesDef } from './primitives';

/**
 * Magnitude over time, split by category.
 *
 * Bars rather than a stacked area: the buckets are discrete counts over an
 * interval, and an area implies a continuous quantity sampled between points.
 *
 * Mark specs: 2px surface gap between stacked segments so adjacent categories
 * read as separate rather than as one block, 2px rounded cap on the topmost
 * segment only (the data-end), square where a segment continues underneath.
 */

export interface Bucket {
  at: string;
  values: Record<string, number>;
}

interface Props {
  buckets: readonly Bucket[];
  series: readonly SeriesDef[];
  height?: number;
  formatValue?: (value: number) => string;
  emptyMessage?: string;
}

const GAP = 2;

export function StackedBars({
  buckets,
  series,
  height = 160,
  formatValue = (value) => value.toLocaleString(),
  emptyMessage = 'No data in this window.',
}: Props) {
  const [hover, setHover] = React.useState<{
    index: number;
    x: number;
    width: number;
  } | null>(null);

  const totals = React.useMemo(
    () => buckets.map((b) => series.reduce((sum, s) => sum + (b.values[s.key] ?? 0), 0)),
    [buckets, series],
  );
  const max = Math.max(...totals, 0);

  if (buckets.length === 0 || max === 0) return <ChartEmpty message={emptyMessage} />;

  const count = buckets.length;
  // Percentage geometry, so the chart is responsive without measuring the DOM.
  const slot = 100 / count;
  const barWidth = Math.max(slot * 0.62, 0.4);

  return (
    <div className="flex flex-col gap-2">
      <div className="relative">
        <svg
          viewBox={`0 0 100 ${height}`}
          preserveAspectRatio="none"
          className="w-full"
          style={{ height }}
          role="img"
          aria-label={`Stacked bars: ${series.map((s) => s.label).join(', ')}`}
          onMouseLeave={() => setHover(null)}
        >
          {/* Recessive grid: four hairlines, no labels on the plot itself. */}
          {[0.25, 0.5, 0.75, 1].map((fraction) => (
            <line
              key={fraction}
              x1={0}
              x2={100}
              y1={height - height * fraction}
              y2={height - height * fraction}
              stroke="var(--color-gridline)"
              strokeWidth={1}
              vectorEffect="non-scaling-stroke"
            />
          ))}

          {buckets.map((bucket, index) => {
            const x = slot * index + (slot - barWidth) / 2;
            let cursor = height;
            const stack = series
              .map((s) => ({ series: s, value: bucket.values[s.key] ?? 0 }))
              .filter((entry) => entry.value > 0);

            return (
              <g key={bucket.at}>
                {/* Hit target spans the whole slot, not just the bar. */}
                <rect
                  x={slot * index}
                  y={0}
                  width={slot}
                  height={height}
                  fill="transparent"
                  onMouseMove={(event) => {
                    const box = event.currentTarget.ownerSVGElement?.getBoundingClientRect();
                    if (!box) return;
                    setHover({
                      index,
                      x: ((slot * index + slot / 2) / 100) * box.width,
                      width: box.width,
                    });
                  }}
                />
                {stack.map((entry, position) => {
                  const barHeight = Math.max((entry.value / max) * (height - 4), 1);
                  const gap = position === 0 ? 0 : GAP;
                  cursor -= barHeight + gap;
                  const isTop = position === stack.length - 1;
                  return (
                    <rect
                      key={entry.series.key}
                      x={x}
                      y={cursor}
                      width={barWidth}
                      height={barHeight}
                      rx={isTop ? 1.2 : 0}
                      fill={entry.series.color}
                      opacity={hover === null || hover.index === index ? 1 : 0.45}
                    />
                  );
                })}
              </g>
            );
          })}
        </svg>

        {hover !== null && buckets[hover.index] ? (
          <Tooltip x={hover.x} y={8} width={hover.width}>
            <div className="mb-1 font-medium">{formatTime(buckets[hover.index]!.at)}</div>
            {series.map((s) => (
              <TooltipRow
                key={s.key}
                color={s.color}
                label={s.label}
                value={formatValue(buckets[hover.index]!.values[s.key] ?? 0)}
              />
            ))}
            <div className="mt-1 border-t border-border pt-1">
              <TooltipRow label="total" value={formatValue(totals[hover.index] ?? 0)} />
            </div>
          </Tooltip>
        ) : null}
      </div>

      <div className="flex items-center justify-between">
        <span className="text-[10px] tabular-nums text-muted-foreground">
          {formatTime(buckets[0]!.at)}
        </span>
        <Legend series={series} />
        <span className="text-[10px] tabular-nums text-muted-foreground">
          {formatTime(buckets[buckets.length - 1]!.at)}
        </span>
      </div>
    </div>
  );
}

function formatTime(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime())
    ? '—'
    : date.toLocaleTimeString(undefined, {
        hour12: false,
        hour: '2-digit',
        minute: '2-digit',
      });
}
