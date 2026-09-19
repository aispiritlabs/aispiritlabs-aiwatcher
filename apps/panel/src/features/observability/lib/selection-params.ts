import type { LiveSelection } from '@/shared/lib/live';
import {
  filterFromSearch,
  filterToSearch,
  objectFilterSchema,
  type ObjectFilter,
  type ObjectFilterSearch,
} from '@/shared/lib/object-filter';
import type { AttributeSelection } from '@/features/observability/components/attribute-picker';
import type { AttributeId } from '@/features/observability/lib/query-builder';

/**
 * The shared object filter, as the two views that ask it of something other
 * than the read model see it.
 *
 * The vocabulary, the URL schema and the translation to a route's parameters
 * live in `shared/lib/object-filter.ts` — one filter for every table and every
 * chart, and the agent pages read it too, which is why it is in `shared`.
 * What stays here is what is true of *these* two subjects and of nothing else:
 * the query builder has no column for a variant, and the live stream carries
 * no span-level fact at all.
 */

export const attributeSearchSchema = objectFilterSchema;

export type AttributeSearch = ObjectFilterSearch;

/**
 * The attributes the builder and the picker know, out of the filter's nine.
 *
 * `variant` is dropped rather than offered: the explorer pivots on it, but the
 * query engines' catalog has no column for it, so a chip whose filter no
 * engine can express is one the builder would have to drop the moment it was
 * clicked. See `query-builder.ts`.
 */
const IDS = [
  'agent',
  'runtime',
  'workflow',
  'session',
  'model',
  'tool',
  'trace',
  'status',
] as const satisfies readonly AttributeId[];

export function selectionFromSearch(search: AttributeSearch): AttributeSelection {
  const filter = filterFromSearch(search);
  const selection: AttributeSelection = {};
  for (const id of IDS) {
    const chosen = filter[id];
    if (chosen && chosen.length > 0) selection[id] = chosen;
  }
  return selection;
}

/**
 * The search patch a selection is.
 *
 * Every axis is written, including the cleared ones as `undefined`: leaving one
 * out of the patch would merge the old value back in, and the chip would come
 * off the screen while the filter stayed on the query. A selection made here
 * never carries a variant, so the patch clears one a link arrived with rather
 * than leaving a filter on screen that this view has no chip for.
 */
export function selectionToSearch(selection: AttributeSelection): AttributeSearch {
  return filterToSearch(selection as ObjectFilter);
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
