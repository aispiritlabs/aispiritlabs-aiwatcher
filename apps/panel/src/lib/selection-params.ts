import { z } from 'zod';

import type { LiveSelection } from '@/lib/live';
import type { AttributeSelection } from '@/components/attribute-picker';
import type { AttributeId } from '@/lib/query-builder';

/**
 * The attribute filter, in the URL.
 *
 * One schema shared by the Query builder and the Live view, because they are
 * the same selection asked of two things — the read model through a Flow
 * query, and the durable log through the live stream. Two schemas would let
 * "watch this live" arrive at a page that understood half the link.
 *
 * Every filter lives in the URL for the reason every filter in this panel
 * does: a selection worth looking at twice is worth sending to somebody, and
 * a link that carries the shape but not the filter lands the reader somewhere
 * else.
 */

const values = z.array(z.string()).optional();

export const attributeSearchSchema = {
  agent: values,
  runtime: values,
  workflow: values,
  session: values,
  model: values,
  tool: values,
  trace: values,
  status: values,
};

export type AttributeSearch = z.infer<z.ZodObject<typeof attributeSearchSchema>>;

const IDS: AttributeId[] = [
  'agent',
  'runtime',
  'workflow',
  'session',
  'model',
  'tool',
  'trace',
  'status',
];

export function selectionFromSearch(search: AttributeSearch): AttributeSelection {
  const selection: AttributeSelection = {};
  for (const id of IDS) {
    const chosen = search[id];
    if (chosen && chosen.length > 0) selection[id] = chosen;
  }
  return selection;
}

/**
 * The search patch a selection is.
 *
 * Every id is written, including the empty ones as `undefined`: leaving a
 * cleared attribute out of the patch would merge the old value back in, and
 * the chip would come off the screen while the filter stayed on the query.
 */
export function selectionToSearch(selection: AttributeSelection): AttributeSearch {
  return Object.fromEntries(
    IDS.map((id) => {
      const chosen = (selection[id] ?? []).filter(Boolean);
      return [id, chosen.length > 0 ? chosen : undefined];
    }),
  );
}

/**
 * The part of a selection the live stream can answer.
 *
 * Model and tool are dropped rather than sent, because an event carries
 * neither: they are span-level facts assembled from several events (ADR_0003).
 * Dropping them here rather than sending them and receiving nothing is what
 * lets the Live view *say* it dropped them — a stream that silently went quiet
 * would read as nothing happening.
 */
export function liveSelectionOf(selection: AttributeSelection): LiveSelection {
  return {
    agents: selection.agent,
    runtimes: selection.runtime,
    workflows: selection.workflow,
    sessions: selection.session,
  };
}

/** Which chosen attributes the live stream would have to ignore. */
export function notLiveFilterable(selection: AttributeSelection): AttributeId[] {
  return (['model', 'tool', 'trace', 'status'] as const).filter(
    (id) => (selection[id] ?? []).length > 0,
  );
}
