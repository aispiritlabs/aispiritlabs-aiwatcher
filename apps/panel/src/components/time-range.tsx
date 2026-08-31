import { z } from 'zod';

import { Button } from '@/components/ui/primitives';

/**
 * One time window, one control, every tab.
 *
 * Before this only the metrics page could say "the last hour"; every list
 * showed the whole retention window and left the reader to date the rows
 * themselves. The presets are the same everywhere on purpose — a control that
 * offers 15m here and 5m there teaches people to read the label before every
 * click.
 *
 * The window lives in the URL like every other filter, so a link to "the failed
 * runs of the last hour" lands the reader on that view. It is **relative**: the
 * server resolves it against its own clock (see the projector's `window`
 * module), which is what makes such a link mean the last hour whenever it is
 * opened rather than the hour it was copied.
 *
 * Zero means everything, and is sent as a zero rather than as an absent
 * parameter so that "all" is a choice in the URL rather than the absence of
 * one — the difference matters when someone shares the link.
 */

export const TIME_WINDOWS = [
  { label: '15m', seconds: 900 },
  { label: '1h', seconds: 3600 },
  { label: '6h', seconds: 21_600 },
  { label: '24h', seconds: 86_400 },
  { label: '7d', seconds: 604_800 },
  { label: 'all', seconds: 0 },
] as const;

/** What a list defaults to when the URL says nothing. */
export const DEFAULT_WINDOW_SECONDS = 86_400;

/** Merge into a route's search schema to give it the control. */
export const windowSearchSchema = { window: z.number().optional() };

/** `0` is a window; `undefined` is not. Sent to the API only when it bounds. */
export function windowParam(seconds: number | undefined): number | undefined {
  return seconds ? seconds : undefined;
}

export function TimeRange({
  value,
  onChange,
}: {
  value: number;
  onChange: (seconds: number) => void;
}) {
  return (
    <div className="flex items-center gap-1">
      <span className="mr-1 text-xs text-muted-foreground">last</span>
      {TIME_WINDOWS.map((option) => (
        <Button
          key={option.label}
          size="sm"
          variant={value === option.seconds ? 'default' : 'outline'}
          onClick={() => onChange(option.seconds)}
        >
          {option.label}
        </Button>
      ))}
    </div>
  );
}
