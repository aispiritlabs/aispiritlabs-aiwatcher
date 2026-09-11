import { describe, expect, it } from 'vitest';

import type { PipelineBlock, PipelineEdge } from '@/api/generated/types.gen';
import { foldPhases, groupId, phasesOf } from '@/features/data-curation/lib/phases';

const block = (id: string, kind: PipelineBlock['spec']['kind']): PipelineBlock =>
  ({ id, spec: { kind } }) as PipelineBlock;

describe('phases', () => {
  it('reads a chain as the four things it does', () => {
    const blocks = [
      block('src', 'source'),
      block('t1', 'transform'),
      block('t2', 'transform'),
      block('nb', 'notebook'),
      block('out', 'view'),
    ];
    const edges: PipelineEdge[] = [
      { from: 'src', to: 't1' },
      { from: 't1', to: 't2' },
      { from: 't2', to: 'nb' },
      { from: 'nb', to: 'out' },
    ];
    expect(phasesOf(blocks, edges).map((p) => [p.id, p.blocks])).toEqual([
      ['ingest', ['src']],
      ['engineering', ['t1', 't2']],
      ['ml', ['nb']],
      ['output', ['out']],
    ]);
  });

  it('returns only the phases that have blocks', () => {
    const blocks = [block('src', 'source'), block('out', 'view')];
    const edges: PipelineEdge[] = [{ from: 'src', to: 'out' }];
    expect(phasesOf(blocks, edges).map((p) => p.id)).toEqual(['ingest', 'output']);
  });

  it('still answers for a draft that is not yet a chain', () => {
    // Two heads, so `orderOf` gives up. The strip stays rather than blinking
    // out at the moment somebody is connecting things.
    const blocks = [block('a', 'source'), block('b', 'source'), block('nb', 'notebook')];
    expect(phasesOf(blocks, []).map((p) => [p.id, p.blocks])).toEqual([
      ['ingest', ['a', 'b']],
      ['ml', ['nb']],
    ]);
  });

  it('keeps a gate as its own phase between the code and the output', () => {
    const blocks = [
      block('src', 'source'),
      block('nb', 'notebook'),
      block('ok', 'approval'),
      block('out', 'view'),
    ];
    const edges: PipelineEdge[] = [
      { from: 'src', to: 'nb' },
      { from: 'nb', to: 'ok' },
      { from: 'ok', to: 'out' },
    ];
    expect(phasesOf(blocks, edges).map((p) => p.id)).toEqual(['ingest', 'ml', 'gate', 'output']);
  });

  describe('folding', () => {
    const blocks = [
      block('src', 'source'),
      block('t1', 'transform'),
      block('t2', 'transform'),
      block('nb', 'notebook'),
      block('out', 'view'),
    ];
    const edges: PipelineEdge[] = [
      { from: 'src', to: 't1' },
      { from: 't1', to: 't2' },
      { from: 't2', to: 'nb' },
      { from: 'nb', to: 'out' },
    ];

    it('leaves everything alone when nothing is folded', () => {
      const folded = foldPhases(blocks, edges, new Set());
      expect(folded.items.map((i) => i.id)).toEqual(['src', 't1', 't2', 'nb', 'out']);
      expect(folded.edges).toEqual(edges);
    });

    it('replaces a phase with one box that carries its blocks in chain order', () => {
      const folded = foldPhases(blocks, edges, new Set(['engineering'] as const));
      expect(folded.items.map((i) => i.id)).toEqual(['src', groupId('engineering'), 'nb', 'out']);
      const group = folded.items.find((i) => i.kind === 'group');
      expect(group?.kind === 'group' && group.blocks.map((b) => b.id)).toEqual(['t1', 't2']);
    });

    it('drops the edge that is now inside the box and redraws the ones that cross it', () => {
      const folded = foldPhases(blocks, edges, new Set(['engineering'] as const));
      expect(folded.edges).toEqual([
        { from: 'src', to: groupId('engineering') },
        { from: groupId('engineering'), to: 'nb' },
        { from: 'nb', to: 'out' },
      ]);
    });

    it('does not draw two arrows where two blocks fed the same box', () => {
      const fan = [block('a', 'source'), block('b', 'source'), block('t', 'transform')];
      const fanEdges: PipelineEdge[] = [
        { from: 'a', to: 't' },
        { from: 'b', to: 't' },
      ];
      const folded = foldPhases(fan, fanEdges, new Set(['ingest'] as const));
      expect(folded.edges).toEqual([{ from: groupId('ingest'), to: 't' }]);
    });

    it('folds every phase at once without losing a block', () => {
      const folded = foldPhases(
        blocks,
        edges,
        new Set(['ingest', 'engineering', 'ml', 'output'] as const),
      );
      expect(folded.items).toHaveLength(4);
      const held = folded.items.flatMap((i) =>
        i.kind === 'group' ? i.blocks.map((b) => b.id) : [],
      );
      expect(held.sort()).toEqual(['nb', 'out', 'src', 't1', 't2']);
    });
  });
});
