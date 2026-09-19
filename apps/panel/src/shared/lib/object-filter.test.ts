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
  const everything = {
    agent: ['researcher'],
    runtime: ['planner'],
    workflow: ['import'],
    session: ['s-1'],
    variant: ['v-1'],
    trace: ['t-1'],
    model: ['opus'],
    tool: ['search'],
    prompt: ['extract'],
    status: ['failed'],
  };

  const asParameters = {
    agent_id: 'researcher',
    runtime: 'planner',
    workflow: 'import',
    conversation_id: 's-1',
    variant_id: 'v-1',
    trace_id: 't-1',
    model: 'opus',
    tool: 'search',
    prompt: 'extract',
    status: 'failed',
  };

  it('sends every axis to the runs route, under the route’s own parameter names', () => {
    const { query, unapplied } = queryFor('runs', everything);
    expect(unapplied).toEqual([]);
    expect(query).toEqual(asParameters);
  });

  it('sends the same ten to the dimension and metrics routes', () => {
    // The three reads fold one population through one predicate, so a question
    // one of them can be asked can be asked of all three. Until the projector
    // grew the rest, "the failed runs of this workflow" was a chart the runs
    // list could draw and the metrics page could only name as unapplied.
    for (const target of ['dimensions', 'metrics'] as const) {
      const { query, unapplied } = queryFor(target, everything);
      expect(unapplied).toEqual([]);
      expect(query).toEqual(asParameters);
    }
  });

  it('names an axis a route has no parameter for instead of sending it', () => {
    const { query, unapplied } = queryFor('spans', { agent: ['a'], workflow: ['import'] });
    expect(query).toEqual({ agent_id: 'a' });
    expect(unapplied.map((entry) => entry.axis)).toEqual(['workflow']);
    expect(unapplied[0]?.why).toMatch(/span does not name its workflow/);
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
  it('says a model narrows the LLM half of the metrics and leaves the tool half alone', () => {
    const note = queryFor('metrics', { model: ['opus'], tool: ['search'] }).notes[0] ?? '';
    expect(note).toMatch(/Runs are the ones matching the filter/);
    expect(note).toMatch(/LLM calls, tokens, cost and LLM latency are that model’s/);
    expect(note).toMatch(/tool counters are that tool’s/);
    expect(note).toMatch(/every other counter covers every call in those runs/);
  });

  it('names a prompt beside the model, because both are properties of a call', () => {
    expect(queryFor('metrics', { model: ['opus'], prompt: ['extract'] }).notes[0]).toMatch(
      /are that model’s and that prompt’s/,
    );
  });

  it('says the opposite on a list whose rows are runs', () => {
    // The failure this is here for: `/runs` and `/dimensions` count nothing
    // below the run — a row's `llm_calls` was folded when the run was ingested
    // and is the same number whatever the filter says. Lending them the metrics
    // sentence would put two populations on one screen with nothing to tell
    // them apart, which is the thing this vocabulary exists to end.
    for (const target of ['runs', 'dimensions'] as const) {
      const note = queryFor(target, { model: ['opus'] }).notes[0] ?? '';
      expect(note).toMatch(/the run's own totals/);
      expect(note).toMatch(/over every call in it rather than only that model’s/);
    }
  });

  it('says nothing where nothing below the run could have been narrowed', () => {
    expect(queryFor('runs', { agent: ['a'], status: ['failed'] }).notes).toEqual([]);
    expect(queryFor('metrics', { agent: ['a'] }).notes).toEqual([]);
    // The span list's rows *are* the calls the chips name, so a sentence
    // restating that would be one more line to read and nothing to learn.
    expect(queryFor('spans', { model: ['opus'] }).notes).toEqual([]);
  });
});
