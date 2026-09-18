import { describe, expect, it } from 'vitest';

import { edgeOf, localInputValue, unixFrom } from '@/shared/lib/iam';

describe('a grant window, between the form and the wire', () => {
  it('reads an empty edge as no edge rather than as the epoch', () => {
    // The difference decides whether a grant ends: `null` is "no end", and 0
    // would be "ended in 1970", which the server would accept.
    expect(unixFrom('')).toBeNull();
    expect(unixFrom('not a time')).toBeNull();
  });

  it('round-trips a moment through the input the reader types into', () => {
    const when = new Date('2026-09-18T14:30:00Z');
    expect(unixFrom(localInputValue(when))).toBe(Math.floor(when.getTime() / 1000));
  });

  it('says an absent edge in words, because a blank cell reads as unknown', () => {
    expect(edgeOf(null, 'no end')).toBe('no end');
    expect(edgeOf(undefined, 'always')).toBe('always');
    expect(edgeOf(0, 'no end')).not.toBe('no end');
  });
});
