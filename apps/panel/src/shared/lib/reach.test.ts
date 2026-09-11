import { describe, expect, it } from 'vitest';

import { edgeInReach, nodeInReach, reachFrom, type ReachEdge } from '@/shared/lib/reach';

/**
 *   a → b → c → e
 *       ↓       ↑
 *       d ──────┘
 */
const DIAMOND: ReachEdge[] = [
  { from: 'a', to: 'b' },
  { from: 'b', to: 'c' },
  { from: 'b', to: 'd' },
  { from: 'c', to: 'e' },
  { from: 'd', to: 'e' },
];

describe('reach', () => {
  it('separates what feeds a node from what it feeds', () => {
    const reach = reachFrom(DIAMOND, 'c');
    expect([...reach.upstream].sort()).toEqual(['a', 'b']);
    expect([...reach.downstream].sort()).toEqual(['e']);
  });

  it('does not put the focus on either side of itself', () => {
    const reach = reachFrom(DIAMOND, 'c');
    expect(reach.upstream.has('c')).toBe(false);
    expect(reach.downstream.has('c')).toBe(false);
    expect(nodeInReach(reach, 'both', 'c')).toBe(true);
  });

  it('leaves a sibling branch out of the reach entirely', () => {
    // `d` is neither before nor after `c`; they merely share both ends.
    const reach = reachFrom(DIAMOND, 'c');
    expect(nodeInReach(reach, 'both', 'd')).toBe(false);
  });

  it('narrows to one direction when asked', () => {
    const reach = reachFrom(DIAMOND, 'c');
    expect(nodeInReach(reach, 'upstream', 'a')).toBe(true);
    expect(nodeInReach(reach, 'upstream', 'e')).toBe(false);
    expect(nodeInReach(reach, 'downstream', 'a')).toBe(false);
    expect(nodeInReach(reach, 'downstream', 'e')).toBe(true);
  });

  it('excludes an edge that goes round the focus rather than through it', () => {
    // b → d → e reaches e without passing through c, so none of it is on
    // c's reach even though b is upstream of c and e is downstream of it.
    const reach = reachFrom(DIAMOND, 'c');
    expect(edgeInReach(reach, 'both', 'b', 'c')).toBe(true);
    expect(edgeInReach(reach, 'both', 'c', 'e')).toBe(true);
    expect(edgeInReach(reach, 'both', 'b', 'd')).toBe(false);
    expect(edgeInReach(reach, 'both', 'd', 'e')).toBe(false);
  });

  it('terminates on a cycle instead of walking it forever', () => {
    // Going round a ring, everything really is both before and after
    // everything else. The answer worth checking is that there is one.
    const looped: ReachEdge[] = [
      { from: 'a', to: 'b' },
      { from: 'b', to: 'c' },
      { from: 'c', to: 'a' },
    ];
    const reach = reachFrom(looped, 'b');
    expect([...reach.downstream].sort()).toEqual(['a', 'c']);
    expect([...reach.upstream].sort()).toEqual(['a', 'c']);
    expect(reach.downstream.has('b')).toBe(false);
  });

  it('answers for a node with nothing attached to it', () => {
    const reach = reachFrom(DIAMOND, 'lonely');
    expect(reach.upstream.size).toBe(0);
    expect(reach.downstream.size).toBe(0);
    expect(nodeInReach(reach, 'both', 'lonely')).toBe(true);
    expect(nodeInReach(reach, 'both', 'a')).toBe(false);
  });
});
