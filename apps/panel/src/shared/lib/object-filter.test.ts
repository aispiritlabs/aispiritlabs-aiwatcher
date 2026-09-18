import { describe, expect, it } from 'vitest';
import { z } from 'zod';

import {
  OBJECT_AXES,
  filterFromSearch,
  filterToSearch,
  objectFilterSchema,
  only,
  queryFor,
  toggle,
} from '@/shared/lib/object-filter';

const schema = z.object(objectFilterSchema);

describe('reading the filter out of a URL', () => {
  it('reads a bare value as one, so a link written before this vocabulary still works', () => {
    // `?status=failed` is in the command palette, in saved pins and in
    // whatever chat somebody pasted it into.
    expect(filterFromSearch(schema.parse({ status: 'failed' }))).toEqual({ status: ['failed'] });
    expect(filterFromSearch(schema.parse({ agent: ['a', 'b'] }))).toEqual({ agent: ['a', 'b'] });
  });

  it('writes every axis back, including the ones just cleared', () => {
    // Leaving a cleared axis out of the patch merges the old value back in,
    // and the chip comes off the screen while the filter stays on the request.
    const patch = filterToSearch({ agent: ['a'], model: [] });
    expect(patch.agent).toEqual(['a']);
    for (const axis of OBJECT_AXES) {
      if (axis !== 'agent') expect(patch[axis]).toBeUndefined();
    }
  });

  it('reads one axis as a single value only when it is unambiguous', () => {
    expect(only({ agent: ['a'] }, 'agent')).toBe('a');
    expect(only({ agent: ['a', 'b'] }, 'agent')).toBeUndefined();
    expect(only({}, 'agent')).toBeUndefined();
  });

  it('toggles a value on and off the axis it belongs to', () => {
    expect(toggle({}, 'status', 'failed').status).toEqual(['failed']);
    expect(toggle({ status: ['failed'] }, 'status', 'failed').status).toEqual([]);
  });
});

describe('translating the filter for one route', () => {
  it('sends every axis to the runs route, under the route’s own parameter names', () => {
    const { query, unapplied } = queryFor('runs', {
      agent: ['researcher'],
      runtime: ['planner'],
      workflow: ['import'],
      session: ['s-1'],
      variant: ['v-1'],
      trace: ['t-1'],
      model: ['opus'],
      tool: ['search'],
      status: ['failed'],
    });
    expect(unapplied).toEqual([]);
    expect(query).toEqual({
      agent_id: 'researcher',
      runtime: 'planner',
      workflow: 'import',
      conversation_id: 's-1',
      variant_id: 'v-1',
      trace_id: 't-1',
      model: 'opus',
      tool: 'search',
      status: 'failed',
    });
  });

  it('names an axis a route has no parameter for instead of sending it', () => {
    const { query, unapplied } = queryFor('dimensions', { agent: ['a'], workflow: ['import'] });
    expect(query).toEqual({ agent_id: 'a' });
    expect(unapplied.map((entry) => entry.axis)).toEqual(['workflow']);
    expect(unapplied[0]?.why).toMatch(/narrows by agent alone/);
  });

  it('does not take the first of two values on a route that narrows to one', () => {
    // The failure this is here for: sending `a` would put a narrower answer on
    // the screen than the two chips beside it claim.
    const { query, unapplied } = queryFor('runs', { agent: ['a', 'b'] });
    expect(query).toEqual({});
    expect(unapplied[0]?.why).toMatch(/narrows to one agent; 2 are chosen/);
  });

  it('refuses a status no run state has a word for', () => {
    // A typo in a pasted link becomes a sentence rather than a 400 from serde
    // and an error page.
    const { query, unapplied } = queryFor('runs', { status: ['broken'] });
    expect(query).toEqual({});
    expect(unapplied[0]?.why).toMatch(/running, succeeded or failed/);
  });

  it('keeps the span list’s outcome out of the run’s status', () => {
    // `ok | error` is a span's own outcome; a failed span inside a run that
    // succeeded is an ordinary thing, and sharing one word would make the
    // filter lie about what it selected.
    const { query, unapplied } = queryFor('spans', { status: ['failed'], model: ['opus'] });
    expect(query).toEqual({ model: 'opus' });
    expect(unapplied[0]?.why).toMatch(/span carries its own outcome/);
  });
});

describe('what the sent axes do to the numbers', () => {
  it('says a model narrows the LLM half and leaves the tool half alone', () => {
    expect(queryFor('runs', { model: ['opus'] }).notes[0]).toMatch(
      /Runs are the ones matching the filter/,
    );
    expect(queryFor('runs', { model: ['opus'] }).notes[0]).toMatch(/tool and step counters/);
  });

  it('says the opposite on metrics, because that route does something else', () => {
    // `/metrics` skips other models' spans and leaves the run set alone
    // (UX-02). The two sentences differ in what they claim about the run count
    // beside them, which is the difference a reader is entitled to.
    expect(queryFor('metrics', { model: ['opus'] }).notes[0]).toMatch(/does not select the runs/);
  });

  it('says nothing where nothing below the run was narrowed', () => {
    expect(queryFor('runs', { agent: ['a'], status: ['failed'] }).notes).toEqual([]);
    expect(queryFor('metrics', { agent: ['a'] }).notes).toEqual([]);
  });
});
