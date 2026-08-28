import * as React from 'react';
import { cn } from '@/lib/utils';

/**
 * The pieces every chart here is built from.
 *
 * Hand-rolled SVG rather than a charting library: there are two chart forms in
 * this panel, both simple, and a library would cost ~150kB and a fight with its
 * defaults to get the mark specs right — thin marks, rounded data-ends anchored
 * to the baseline, a 2px surface gap between stacked segments, recessive grid.
 */

/** The fixed series order. Assigned by identity, never cycled. */
export const SERIES = [
  'var(--color-series-1)',
  'var(--color-series-2)',
  'var(--color-series-3)',
  'var(--color-series-4)',
] as const;

export interface SeriesDef {
  key: string;
  label: string;
  color: string;
}

/**
 * A legend is present whenever there are two or more series, so identity is
 * never carried by colour alone.
 */
export function Legend({ series }: { series: readonly SeriesDef[] }) {
  if (series.length < 2) return null;
  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-1">
      {series.map((s) => (
        <span key={s.key} className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <span
            aria-hidden
            className="h-2 w-2 rounded-[2px]"
            style={{ backgroundColor: s.color }}
          />
          {s.label}
        </span>
      ))}
    </div>
  );
}

/**
 * Tooltip anchored inside the plot.
 *
 * It hangs *below* its anchor rather than above. Above is the conventional
 * placement and it is wrong here: the anchor sits near the top of a short plot,
 * so the tooltip escaped the card and was clipped by the viewport whenever the
 * chart was scrolled near the top of the window.
 *
 * `x` is clamped so a bar at either end keeps the whole box on the plot.
 */
export function Tooltip({
  x,
  y,
  width,
  children,
}: {
  x: number;
  y: number;
  /** Plot width, for the horizontal clamp. */
  width: number;
  children: React.ReactNode;
}) {
  const half = 76; // half the min-width, plus padding
  const left = Math.min(Math.max(x, half), Math.max(width - half, half));
  return (
    <div
      className="pointer-events-none absolute z-10 min-w-36 rounded-md border border-border bg-card px-2.5 py-2 text-xs shadow-lg"
      style={{ left, top: y, transform: 'translate(-50%, 0)' }}
    >
      {children}
    </div>
  );
}

export function TooltipRow({
  color,
  label,
  value,
}: {
  color?: string;
  label: string;
  value: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between gap-4">
      <span className="flex items-center gap-1.5 text-muted-foreground">
        {color ? (
          <span aria-hidden className="h-2 w-2 rounded-[2px]" style={{ backgroundColor: color }} />
        ) : null}
        {label}
      </span>
      <span className="tabular-nums">{value}</span>
    </div>
  );
}

export function ChartEmpty({ message }: { message: string }) {
  return (
    <div className="flex h-40 items-center justify-center text-sm text-muted-foreground">
      {message}
    </div>
  );
}

/** Axis and value labels. Text wears text tokens, never a series colour. */
export function AxisLabel({
  children,
  className,
}: {
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <span className={cn('text-[10px] tabular-nums text-muted-foreground', className)}>
      {children}
    </span>
  );
}
