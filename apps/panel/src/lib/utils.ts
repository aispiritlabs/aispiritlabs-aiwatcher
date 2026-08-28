import { clsx, type ClassValue } from 'clsx';
import { twMerge } from 'tailwind-merge';

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

/** `1240` → `1.24 s`, `85` → `85 ms`. */
export function formatDuration(ms: number | null | undefined): string {
  if (ms === null || ms === undefined) return '—';
  if (ms < 1000) return `${Math.round(ms)} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(2)} s`;
  const minutes = Math.floor(ms / 60_000);
  const seconds = Math.round((ms % 60_000) / 1000);
  return `${minutes}m ${seconds}s`;
}

/** `812` → `812`, `1_240_000` → `1.24M`. Token counts get large. */
export function formatCount(value: number | null | undefined): string {
  if (value === null || value === undefined) return '—';
  if (value < 1000) return String(value);
  if (value < 1_000_000) return `${(value / 1000).toFixed(1)}k`;
  return `${(value / 1_000_000).toFixed(2)}M`;
}

/** Ids are 32 hex characters; show enough to recognise, not enough to wrap. */
export function shortId(id: string | null | undefined, length = 8): string {
  if (!id) return '—';
  return id.length <= length ? id : id.slice(0, length);
}

/**
 * An id shortened from both ends, for lists where the id *is* the row.
 *
 * This began as a workaround: `TraceId::derive` was raw FNV-1a, whose output
 * barely moves when the input differs only near its end, so `run-1` and
 * `run-2` derived trace ids sharing nine leading hex digits and most of the
 * middle — a screen of sequentially-named runs rendered as a screen of
 * identical rows. The derivation now runs an avalanche finalizer and every bit
 * moves (ADR_0001, amendment 2026-08-28), so a prefix would in fact be enough.
 *
 * Kept as a deliberate display choice rather than a fix: 32 hex digits do not
 * belong in a table column at full width, and showing both ends still reads
 * better than a prefix when the id is all there is to tell two rows apart.
 */
export function pinchId(id: string | null | undefined, head = 6, tail = 6): string {
  if (!id) return '—';
  return id.length <= head + tail + 1 ? id : `${id.slice(0, head)}…${id.slice(-tail)}`;
}

export function formatTime(iso: string | null | undefined): string {
  if (!iso) return '—';
  const date = new Date(iso);
  return Number.isNaN(date.getTime())
    ? '—'
    : date.toLocaleTimeString(undefined, {
        hour12: false,
        hour: '2-digit',
        minute: '2-digit',
        second: '2-digit',
        fractionalSecondDigits: 3,
      });
}
