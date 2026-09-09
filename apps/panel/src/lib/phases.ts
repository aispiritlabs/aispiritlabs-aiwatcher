import type { BlockSpec, PipelineBlock, PipelineEdge } from '@/api/generated/types.gen';
import { orderOf } from '@/lib/pipeline';

/**
 * A curation, one level up: the four or five things it does, rather than the
 * ten boxes it does them with.
 *
 * Ten blocks on a canvas is a chain somebody traces with a finger. The same
 * ten as *ingest → data engineering → ML → output* is a shape somebody reads
 * in a second, and it is the level most questions are actually asked at.
 *
 * These are **derived, never authored**, and that is the whole reason to trust
 * them. A phase is a block's kind, and the order they appear in is guaranteed
 * by the registry rather than by this file: a Flow PHP block cannot read past
 * something it cannot read, so a transform is refused after a notebook or an
 * approval by name. There is no arrangement of a *valid* chain whose phases
 * come out in a different order.
 *
 * An authored `group` field was the other way to do this. It would be a second
 * description of the chain, free to disagree with the blocks under it, and the
 * day it did somebody would trust the label rather than the code.
 */

export type PhaseId = 'ingest' | 'engineering' | 'ml' | 'gate' | 'output';

export type Phase = {
  id: PhaseId;
  label: string;
  /** What this phase is, for somebody who has not met the block kinds. */
  hint: string;
  blocks: string[];
};

const OF_KIND: Record<BlockSpec['kind'], PhaseId> = {
  source: 'ingest',
  transform: 'engineering',
  notebook: 'ml',
  approval: 'gate',
  view: 'output',
};

/** Canonical order. Only phases with blocks in them are ever returned. */
const ORDER: { id: PhaseId; label: string; hint: string }[] = [
  { id: 'ingest', label: 'Ingest', hint: 'where the rows come from' },
  { id: 'engineering', label: 'Data engineering', hint: 'one compiled Flow query' },
  { id: 'ml', label: 'ML', hint: 'code a query has no vocabulary for' },
  { id: 'gate', label: 'Gate', hint: 'a person answers' },
  { id: 'output', label: 'Output', hint: 'a dataset version' },
];

/**
 * The phases these blocks form, in order.
 *
 * Chain order is used when the blocks *are* a chain, because that is the order
 * they run in. A draft halfway through being wired is not one, and falls back
 * to the order they were added — which keeps the strip present and honest
 * while somebody is still connecting things up, rather than blinking out at
 * exactly the moment they are working.
 */
export function phasesOf(blocks: PipelineBlock[], edges: PipelineEdge[]): Phase[] {
  const ordered = orderOf(blocks, edges) ?? blocks;
  const held = new Map<PhaseId, string[]>();
  for (const block of ordered) {
    const phase = OF_KIND[block.spec.kind];
    held.set(phase, [...(held.get(phase) ?? []), block.id]);
  }
  return ORDER.filter((phase) => held.has(phase.id)).map((phase) => ({
    ...phase,
    blocks: held.get(phase.id) ?? [],
  }));
}

/** The phase a block belongs to, for the canvas to tint by. */
export function phaseOf(spec: BlockSpec): PhaseId {
  return OF_KIND[spec.kind];
}

/** One thing on the canvas: a block, or a phase standing in for several. */
export type CanvasItem =
  | { kind: 'block'; id: string; block: PipelineBlock }
  | { kind: 'group'; id: string; phase: Phase; blocks: PipelineBlock[] };

/** The id a collapsed phase stands under. Namespaced so it cannot be a block. */
export function groupId(phase: PhaseId): string {
  return `phase:${phase}`;
}

/**
 * The canvas with some phases folded shut.
 *
 * A ten-block chain laid out along one line is the right *shape* and an
 * unreadable *picture*: it fits at half zoom, where a block's title is seven
 * pixels. Folding the six data-engineering steps into one box leaves four
 * boxes, which fit at full size — so this is what makes the tidy layout usable
 * rather than a separate nicety.
 *
 * Folding is a **view**, not an edit. No position moves, nothing is saved, and
 * the blocks are still all there — which is the difference between this and
 * every other control on this page. An edge that ran between two folded blocks
 * has both ends inside one box and simply stops being drawn; an edge that
 * crossed the boundary is redrawn to the box.
 */
export function foldPhases(
  blocks: PipelineBlock[],
  edges: PipelineEdge[],
  collapsed: ReadonlySet<PhaseId>,
): { items: CanvasItem[]; edges: PipelineEdge[] } {
  const phases = phasesOf(blocks, edges);
  const shut = phases.filter((phase) => collapsed.has(phase.id));
  if (shut.length === 0) {
    return {
      items: blocks.map((block) => ({ kind: 'block', id: block.id, block })),
      edges,
    };
  }

  const byId = new Map(blocks.map((block) => [block.id, block]));
  // Which box a block is drawn as. A block in an open phase is itself.
  const standsAs = new Map<string, string>();
  for (const phase of shut) {
    for (const id of phase.blocks) standsAs.set(id, groupId(phase.id));
  }

  const items: CanvasItem[] = [];
  const emitted = new Set<string>();
  for (const block of blocks) {
    const group = standsAs.get(block.id);
    if (group === undefined) {
      items.push({ kind: 'block', id: block.id, block });
      continue;
    }
    if (emitted.has(group)) continue;
    emitted.add(group);
    const phase = shut.find((candidate) => groupId(candidate.id) === group);
    if (phase === undefined) continue;
    items.push({
      kind: 'group',
      id: group,
      phase,
      // Chain order, so the box carries the blocks in the order they run and
      // the first of them is the one whose position it takes.
      blocks: phase.blocks.map((id) => byId.get(id)).filter((b): b is PipelineBlock => !!b),
    });
  }

  const seen = new Set<string>();
  const folded: PipelineEdge[] = [];
  for (const edge of edges) {
    const from = standsAs.get(edge.from) ?? edge.from;
    const to = standsAs.get(edge.to) ?? edge.to;
    // Both ends inside one box: the edge is interior and has nowhere to go.
    if (from === to) continue;
    const key = `${from}\u0000${to}`;
    if (seen.has(key)) continue;
    seen.add(key);
    folded.push({ from, to });
  }
  return { items, edges: folded };
}
