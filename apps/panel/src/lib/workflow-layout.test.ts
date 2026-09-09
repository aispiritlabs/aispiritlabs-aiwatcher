import { describe, expect, it } from 'vitest';

import { NODE_WIDTH, layoutGraph } from '@/lib/workflow-layout';

describe('layout', () => {
  it('puts a chain on one line, which is the whole point of tidying one', () => {
    const ids = ['a', 'b', 'c', 'd'];
    const placed = layoutGraph(ids, [
      { from: 'a', to: 'b' },
      { from: 'b', to: 'c' },
      { from: 'c', to: 'd' },
    ]);
    expect(placed.map((p) => p.position.y)).toEqual([0, 0, 0, 0]);
    const xs = placed.map((p) => p.position.x).sort((l, r) => l - r);
    expect(xs[0]).toBe(0);
    expect(xs[3]).toBeGreaterThan(NODE_WIDTH * 3);
  });

  it('stacks siblings instead of putting them one after the other', () => {
    const placed = layoutGraph(
      ['head', 'left', 'right', 'join'],
      [
        { from: 'head', to: 'left' },
        { from: 'head', to: 'right' },
        { from: 'left', to: 'join' },
        { from: 'right', to: 'join' },
      ],
    );
    const at = (id: string) => placed.find((p) => p.id === id)?.position;
    expect(at('left')?.x).toBe(at('right')?.x);
    expect(at('left')?.y).not.toBe(at('right')?.y);
    // A fan-in sits past *both* of its inputs, not beside the earlier one.
    expect(at('join')?.x ?? 0).toBeGreaterThan(at('left')?.x ?? 0);
  });

  it('terminates on a cycle rather than ranking forever', () => {
    const placed = layoutGraph(
      ['a', 'b', 'c'],
      [
        { from: 'a', to: 'b' },
        { from: 'b', to: 'c' },
        { from: 'c', to: 'a' },
      ],
    );
    expect(placed).toHaveLength(3);
    expect(placed.every((p) => Number.isFinite(p.position.x))).toBe(true);
  });
});
