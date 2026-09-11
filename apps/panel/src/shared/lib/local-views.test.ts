import { describe, expect, it } from 'vitest';
import { readViews, searchFromView, storageScope, viewFromSearch } from './local-views';

describe('local analysis views', () => {
  it('stores only selection metadata and round-trips an explicit baseline', () => {
    const view = viewFromSearch('Review', 'evaluation', {
      report: 'candidate',
      baseline: 'base',
      metrics: 'score,time',
      suite: 'test',
      window: 0,
      report_body: { secret: 'private' },
      conversation: 'private',
      token: 'secret',
    });
    expect(JSON.stringify(view)).not.toMatch(/private|secret|token|report_body|conversation/);
    expect(searchFromView(readViews(JSON.stringify([view]))[0]!)).toEqual({
      suite: 'test',
      window: 0,
      report: 'candidate',
      baseline: 'base',
      metrics: 'score,time',
    });
  });
  it('keeps instanced identities separate, including delimiter-like names', () => {
    expect(storageScope('one', 'alice')).not.toBe(storageScope('two', 'alice'));
    expect(storageScope('one', 'alice')).not.toBe(storageScope('one', 'bob'));
    expect(storageScope('a:b', 'c')).not.toBe(storageScope('a', 'b:c'));
  });
  it('refuses corrupted or future storage instead of replacing it', () => {
    expect(() => readViews('{')).toThrow();
    expect(() => readViews('[{"schema_version":2}]')).toThrow();
  });
});
