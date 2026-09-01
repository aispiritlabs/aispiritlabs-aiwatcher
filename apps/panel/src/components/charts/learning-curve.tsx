import * as React from 'react';

import { AxisLabel, ChartEmpty, Legend, SERIES, Tooltip, TooltipRow } from './primitives';

/**
 * A metric against epoch: the third chart form in this panel, and the first
 * one that is a line.
 *
 * A line rather than bars, because what is read here is a *shape* — is it still
 * coming down, did it flatten at epoch nine, did validation turn back up while
 * training kept falling. Bars encode magnitude per category; this is one
 * quantity sampled along an ordered axis, and drawing it as bars would make the
 * reader reconstruct the trend they came to see.
 *
 * Two decisions worth stating:
 *
 * * **Each series gets its own y-scale option, off by default.** A loss at 1.6
 *   and a mIoU at 0.42 on one axis makes the mIoU a flat line at the bottom. But
 *   two axes silently invite a comparison of two magnitudes that share nothing,
 *   so the default is one scale and normalising is a deliberate click.
 * * **Points are drawn only when there are few of them.** Two hundred epochs of
 *   dots is a thick line; twelve epochs with no dots hides that they are
 *   measurements rather than a fit.
 */

export interface CurveSeries {
  key: string;
  label: string;
  points: [number, number][];
}

const PADDING = { top: 12, right: 12, bottom: 26, left: 44 };
const HEIGHT = 220;

export function LearningCurve({
  series,
  xLabel = 'epoch',
  normalise = false,
}: {
  series: CurveSeries[];
  xLabel?: string;
  /** Scale each series to its own range. Off by default — see the note above. */
  normalise?: boolean;
}) {
  const container = React.useRef<HTMLDivElement>(null);
  const [width, setWidth] = React.useState(640);
  const [hover, setHover] = React.useState<number | null>(null);

  React.useEffect(() => {
    const element = container.current;
    if (!element) return;
    const measure = () => setWidth(Math.max(element.clientWidth, 240));
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  const drawn = series.filter((entry) => entry.points.length > 0);
  const defs = drawn.map((entry, index) => ({
    key: entry.key,
    label: entry.label,
    color: SERIES[index % SERIES.length] ?? SERIES[0],
  }));

  const geometry = React.useMemo(() => {
    if (drawn.length === 0) return null;
    const xs = drawn.flatMap((entry) => entry.points.map(([x]) => x));
    const minX = Math.min(...xs);
    const maxX = Math.max(...xs);
    const plotWidth = Math.max(width - PADDING.left - PADDING.right, 10);
    const plotHeight = HEIGHT - PADDING.top - PADDING.bottom;

    const rangeOf = (entry: CurveSeries): [number, number] => {
      const values = entry.points.map(([, y]) => y);
      return [Math.min(...values), Math.max(...values)];
    };
    const shared = drawn.reduce<[number, number]>(
      ([low, high], entry) => {
        const [min, max] = rangeOf(entry);
        return [Math.min(low, min), Math.max(high, max)];
      },
      [Infinity, -Infinity],
    );

    const x = (value: number) =>
      PADDING.left + (maxX === minX ? plotWidth / 2 : ((value - minX) / (maxX - minX)) * plotWidth);
    const y = (value: number, entry: CurveSeries) => {
      const [low, high] = normalise ? rangeOf(entry) : shared;
      const span = high - low || 1;
      return PADDING.top + plotHeight - ((value - low) / span) * plotHeight;
    };

    return { minX, maxX, shared, plotWidth, plotHeight, x, y };
  }, [drawn, normalise, width]);

  if (!geometry) return <ChartEmpty message="No epochs yet." />;

  // A tick per epoch would be unreadable past about twenty; five is what fits.
  const ticks = Array.from({ length: 5 }, (_, index) =>
    Math.round(geometry.minX + ((geometry.maxX - geometry.minX) * index) / 4),
  ).filter((tick, index, all) => all.indexOf(tick) === index);

  const nearest = (event: React.MouseEvent<SVGSVGElement>) => {
    const box = event.currentTarget.getBoundingClientRect();
    const offset = event.clientX - box.left;
    const ratio = (offset - PADDING.left) / geometry.plotWidth;
    const epoch = Math.round(geometry.minX + ratio * (geometry.maxX - geometry.minX));
    setHover(Math.min(Math.max(epoch, geometry.minX), geometry.maxX));
  };

  const hovered =
    hover === null
      ? []
      : defs
          .map((def, index) => {
            const entry = drawn[index];
            const point = entry?.points.find(([x]) => x === hover);
            return point ? { def, value: point[1] } : null;
          })
          .filter((entry): entry is { def: (typeof defs)[number]; value: number } => entry !== null);

  return (
    <div ref={container} className="relative flex flex-col gap-2">
      <Legend series={defs} />
      <svg
        width={width}
        height={HEIGHT}
        role="img"
        aria-label={`${defs.map((def) => def.label).join(', ')} by ${xLabel}`}
        onMouseMove={nearest}
        onMouseLeave={() => setHover(null)}
      >
        {/* Recessive grid: four lines, no box, no ticks on the y-axis. */}
        {[0, 0.25, 0.5, 0.75, 1].map((fraction) => {
          const y = PADDING.top + geometry.plotHeight * fraction;
          return (
            <line
              key={fraction}
              x1={PADDING.left}
              x2={width - PADDING.right}
              y1={y}
              y2={y}
              stroke="var(--color-border)"
              strokeWidth={1}
            />
          );
        })}
        {!normalise &&
          ([
            [1, geometry.shared[1]],
            [0, geometry.shared[0]],
          ] as const).map(([fraction, value]) => (
            <text
              key={fraction}
              x={PADDING.left - 6}
              y={PADDING.top + geometry.plotHeight * (1 - fraction) + 4}
              textAnchor="end"
              className="fill-muted-foreground text-[10px] tabular-nums"
            >
              {format(value)}
            </text>
          ))}
        {ticks.map((tick) => (
          <text
            key={tick}
            x={geometry.x(tick)}
            y={HEIGHT - 8}
            textAnchor="middle"
            className="fill-muted-foreground text-[10px] tabular-nums"
          >
            {tick}
          </text>
        ))}
        {hover !== null && (
          <line
            x1={geometry.x(hover)}
            x2={geometry.x(hover)}
            y1={PADDING.top}
            y2={PADDING.top + geometry.plotHeight}
            stroke="var(--color-border)"
            strokeWidth={1}
          />
        )}
        {drawn.map((entry, index) => {
          const def = defs[index];
          const path = entry.points
            .map(
              ([x, value], at) =>
                `${at === 0 ? 'M' : 'L'}${geometry.x(x).toFixed(1)},${geometry.y(value, entry).toFixed(1)}`,
            )
            .join(' ');
          return (
            <g key={entry.key}>
              <path
                d={path}
                fill="none"
                stroke={def?.color}
                strokeWidth={1.75}
                strokeLinejoin="round"
                strokeLinecap="round"
              />
              {entry.points.length <= 24 &&
                entry.points.map(([x, value]) => (
                  <circle
                    key={x}
                    cx={geometry.x(x)}
                    cy={geometry.y(value, entry)}
                    r={2.5}
                    fill={def?.color}
                  />
                ))}
            </g>
          );
        })}
      </svg>
      {hover !== null && hovered.length > 0 && (
        <Tooltip x={geometry.x(hover)} y={PADDING.top} width={width}>
          <div className="mb-1 font-medium">
            {xLabel} {hover}
          </div>
          {hovered.map(({ def, value }) => (
            <TooltipRow key={def.key} color={def.color} label={def.label} value={format(value)} />
          ))}
        </Tooltip>
      )}
      <AxisLabel className="self-center">{xLabel}</AxisLabel>
    </div>
  );
}

/** Losses run to four decimals and scores to three; one rule covers both. */
function format(value: number): string {
  if (!Number.isFinite(value)) return '—';
  if (Math.abs(value) >= 100) return value.toFixed(0);
  if (Math.abs(value) >= 1) return value.toFixed(3);
  return value.toFixed(4);
}
