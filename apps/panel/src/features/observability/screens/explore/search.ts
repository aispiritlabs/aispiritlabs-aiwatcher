import { windowSearchSchema } from '@/shared/components/time-range';
import { z } from 'zod';

/**
 * One place to move between every level of a run.
 *
 * ```text
 * <pivot> → run (trace) → agent (span) → llm / tool call (span) → events
 * ```
 *
 * The tree on the left is the whole hierarchy, expandable in place; the pane on
 * the right is the messages for whatever is selected. Selecting deeper never
 * loses the levels above it. Every selection lives in the URL, so a level is
 * linkable and the back button walks the hierarchy.
 *
 * **One grouping control, not two.** The pivot decides the shape; below it the
 * list is flat and in order. A second control in the message pane grouped
 * already-narrowed data by the thing it had just been narrowed to, and
 * collapsed the one view that has to stay chronological.
 *
 * **Nothing loads eagerly.** Every list is a cursor page from the server and a
 * virtual window in the browser, and the searches are server-side for the same
 * reason: filtering in the browser means downloading everything first.
 */

/** What the tree's top level is. Everything below it stays the same. */
export const PIVOTS = [
  'session',
  'agent',
  'runtime',
  'workflow',
  'trace',
  'model',
  'tool',
  'span',
] as const;

export const searchSchema = z.object({
  ...windowSearchSchema,
  /** The dimension the tree is rooted on. */
  by: z.enum(PIVOTS).optional(),
  /** The selected row of that dimension. */
  key: z.string().optional(),
  run: z.string().optional(),
  span: z.string().optional(),
  /** Filters the tree. Server-side. */
  find: z.string().optional(),
  /** Filters the messages. Server-side. */
  q: z.string().optional(),
});
