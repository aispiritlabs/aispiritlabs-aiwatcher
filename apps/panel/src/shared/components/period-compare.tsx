import { z } from 'zod';

import { Button } from '@/shared/components/ui/primitives';

/**
 * The period before this one, on the same filter.
 *
 * "Is this worse than it was" is the question every aggregate on a page is
 * really being read for, and until this the only way to ask it was to change
 * the window, remember six numbers and change it back. Two reads answer it,
 * and the whole design is in which instant the second one ends at.
 *
 * **The baseline's end is the current window's start, as the server reported
 * it.** Not `now - window` computed here: a relative window is resolved
 * against the server's clock (`crate::window`), so a browser running a few
 * minutes fast would ask for a period overlapping the one beside it, and the
 * overlap would be invisible in the answer. The metrics response carries the
 * window it actually used, so the second read is derived from the first and
 * the two halves are adjacent by construction.
 *
 * Minus one second, because both ends of a window are inclusive: a run that
 * started exactly at the boundary would otherwise be counted on both sides.
 *
 * **A relative pair, not a pinned one.** `?compare=previous` means "the period
 * before whatever this link opens on", the way `?window=3600` means the last
 * hour whenever it is opened rather than the hour it was copied. Pinning the
 * baseline to a fixed instant is a different question — one worth a control of
 * its own the day somebody needs it, and not one this parameter should quietly
 * become.
 *
 * **A change is never coloured here.** The panel colours a delta in exactly
 * one place, where a pinned evaluation context declares which way each metric
 * is better (`apps/panel/CLAUDE.md`). Nothing declares that about a run count
 * or a bill, so the sign is drawn and the reading is left to the reader.
 */

/** Merge into a route's search schema to give it the second period. */
export const compareSearchSchema = { compare: z.enum(['previous']).optional() };

/**
 * The instant the period before this one ended.
 *
 * `windowFrom` is the start of the window the server answered, as it came
 * back. `undefined` for anything that is not a date — a caller that cannot
 * name the boundary asks no second question rather than asking a vague one.
 */
export function endOfPeriodBefore(windowFrom: string): number | undefined {
  const at = Date.parse(windowFrom);
  return Number.isFinite(at) ? Math.floor(at / 1000) - 1 : undefined;
}

/** A period, in the words a header says it in. Both ends are the server's. */
export function periodLabel(from: string, to: string): string {
  const start = new Date(from);
  const end = new Date(to);
  if (Number.isNaN(start.getTime()) || Number.isNaN(end.getTime())) return 'the previous period';
  const day: Intl.DateTimeFormatOptions = { month: 'short', day: 'numeric' };
  const clock: Intl.DateTimeFormatOptions = { hour: '2-digit', minute: '2-digit' };
  const sameDay = start.toDateString() === end.toDateString();
  return sameDay
    ? `${start.toLocaleDateString(undefined, day)} ${start.toLocaleTimeString(
        undefined,
        clock,
      )}–${end.toLocaleTimeString(undefined, clock)}`
    : `${start.toLocaleDateString(undefined, day)} ${start.toLocaleTimeString(
        undefined,
        clock,
      )} – ${end.toLocaleDateString(undefined, day)} ${end.toLocaleTimeString(undefined, clock)}`;
}

/**
 * The control, next to the period it is a second of.
 *
 * Disabled on "all", and it says why rather than going missing: a window with
 * no width has no period before it, and a control that vanished would read as
 * a feature this page does not have.
 */
export function CompareToggle({
  on,
  windowSeconds,
  onChange,
}: {
  on: boolean;
  windowSeconds: number;
  onChange: (on: boolean) => void;
}) {
  const impossible = windowSeconds === 0;
  return (
    <Button
      size="sm"
      variant={on ? 'default' : 'outline'}
      aria-pressed={on}
      disabled={impossible}
      title={
        impossible
          ? 'A window of everything has no period before it. Pick a period to compare one.'
          : 'Read the period before this one, on the same filter'
      }
      onClick={() => onChange(!on)}
    >
      vs previous
    </Button>
  );
}

/**
 * One figure against the same figure in the period before.
 *
 * Four answers, and the three that are not a percentage are the point. A
 * figure nothing reported in the baseline is *unknown*, never nought — the
 * same rule the cost tile keeps about a call that reported no price. A
 * baseline of zero has no percentage to give: nothing is not a denominator,
 * and "+∞%" or a quiet "+100%" would both be inventions.
 *
 * `points` is for a figure that is already a rate. A success rate going from
 * 75% to 80% has risen by five points and by six and a half percent, and only
 * the first is the sentence anybody means.
 */
export function AgainstPeriod({
  now,
  before,
  format,
  points = false,
}: {
  now: number | null | undefined;
  before: number | null | undefined;
  format: (value: number) => string;
  points?: boolean;
}) {
  if (before === null || before === undefined) {
    return <span className="text-xs text-muted-foreground">nothing reported before</span>;
  }
  if (now === null || now === undefined) {
    return <span className="text-xs text-muted-foreground">was {format(before)}</span>;
  }
  const change = changeText(now, before, points);
  return (
    <span className="text-xs text-muted-foreground">
      was {format(before)}
      {change ? ` · ${change}` : ''}
    </span>
  );
}

function changeText(now: number, before: number, points: boolean): string | undefined {
  if (points) {
    const moved = Math.round((now - before) * 100);
    return moved === 0 ? 'unchanged' : `${sign(moved)}${Math.abs(moved)} pts`;
  }
  if (now === before) return 'unchanged';
  if (before === 0) return undefined;
  const ratio = Math.round(((now - before) / before) * 100);
  // A change too small to round to a percent is still a change, and reporting
  // it as 0% would read as "the same".
  return ratio === 0 ? `${sign(now - before)}under 1%` : `${sign(ratio)}${Math.abs(ratio)}%`;
}

function sign(value: number): string {
  return value > 0 ? '+' : '−';
}
