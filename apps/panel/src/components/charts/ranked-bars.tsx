import { ChartEmpty } from './primitives';

/**
 * Identity comparison, ranked.
 *
 * Horizontal because the labels are names — model ids, agent ids, tool names —
 * and a horizontal bar gives them a full line instead of a rotated axis tick.
 *
 * Every row is direct-labelled with its value, so this reads without colour and
 * without a legend. A single accent hue carries magnitude; rank is the y-order.
 *
 * The bar's colour is **not** a status channel. An earlier version turned the
 * whole bar red when a row had any failure, which put two encodings on one
 * mark: length meant tokens, colour meant health, and three rows each with one
 * failure painted the entire chart red at an 83% success rate. Failures get
 * their own marker and label instead, so a long bar still reads as "expensive"
 * and a red dot reads as "and it has failures".
 */

export interface RankedRow {
  key: string;
  label: string;
  value: number;
  /** Shown on the right, e.g. "24 calls · p95 1.2 s". */
  detail?: string;
  /** Marks a row as having failures. Ships with a label that names the count. */
  warn?: boolean;
}

export function RankedBars({
  rows,
  formatValue = (value) => value.toLocaleString(),
  emptyMessage = 'Nothing recorded yet.',
  max: explicitMax,
}: {
  rows: readonly RankedRow[];
  formatValue?: (value: number) => string;
  emptyMessage?: string;
  max?: number;
}) {
  const max = explicitMax ?? Math.max(...rows.map((row) => row.value), 0);
  if (rows.length === 0 || max === 0) return <ChartEmpty message={emptyMessage} />;

  return (
    <div className="flex flex-col">
      {rows.map((row) => (
        <div
          key={row.key}
          className="grid grid-cols-[minmax(7rem,12rem)_1fr_auto] items-center gap-3 border-b border-border/40 px-3 py-2 last:border-b-0"
        >
          <span className="flex min-w-0 items-center gap-1.5 text-sm">
            {/* A status marker, always beside a label that names the count. */}
            {row.warn ? (
              <span
                aria-hidden
                className="h-1.5 w-1.5 shrink-0 rounded-full"
                style={{ backgroundColor: 'var(--color-status-critical)' }}
              />
            ) : null}
            <span className="truncate" title={row.label}>
              {row.label}
            </span>
          </span>
          <div className="relative h-3">
            <div
              className="absolute inset-y-0 left-0 rounded-r-[3px]"
              style={{
                width: `${Math.max((row.value / max) * 100, 0.6)}%`,
                backgroundColor: 'var(--color-series-1)',
              }}
            />
          </div>
          <span className="whitespace-nowrap text-right text-xs tabular-nums">
            <span className="font-medium">{formatValue(row.value)}</span>
            {row.detail ? <span className="ml-2 text-muted-foreground">{row.detail}</span> : null}
          </span>
        </div>
      ))}
    </div>
  );
}
