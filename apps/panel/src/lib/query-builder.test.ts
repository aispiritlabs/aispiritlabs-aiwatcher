import { describe, expect, it } from 'vitest';

import {
  EMPTY_DRAFT,
  compile,
  selectedMetrics,
  sortableColumns,
  type QueryDraft,
} from './query-builder';

/**
 * What these are for.
 *
 * Not that the compiler produces *some* text — that a snapshot would prove and
 * nothing would be learnt from. The three things worth pinning are the ones a
 * change would silently get wrong: that a list column is expanded before it is
 * grouped, that two values on one attribute are an *or* rather than two
 * conjoined filters that can never both hold, and that nothing reaches the
 * text through string interpolation without being quoted.
 *
 * `flow-check` in CI runs the same shapes through the real parser
 * (`services/query/flow/tests`), which is what says the text is *valid*. These say
 * it is the text we meant.
 */

const draft = (overrides: Partial<QueryDraft>): QueryDraft => ({ ...EMPTY_DRAFT, ...overrides });

describe('compile', () => {
  it('expands a list column before grouping by it', () => {
    // The single most common first query, and the one the query service ships
    // a hint for: `runs` carries `agents`, so a plain groupBy sees no column.
    const query = compile(draft({ groupBy: ['agent'] }));

    expect(query).toContain("->withEntry('agent', array_expand(ref('agents')))");
    expect(query.indexOf('withEntry')).toBeLessThan(query.indexOf('groupBy'));
  });

  it('expands a list column once when it is both filtered and grouped', () => {
    // Twice would multiply every row by the number of agents a second time,
    // and every count in the result would be wrong rather than absent.
    const query = compile(draft({ filters: { agent: ['planner'] }, groupBy: ['agent'] }));

    expect(query.match(/array_expand/g)).toHaveLength(1);
  });

  it('reads two values on one attribute as either of them', () => {
    const query = compile(draft({ filters: { agent: ['planner', 'estimator'] } }));

    expect(query).toContain(
      "->filter(ref('agent')->same(lit('planner'))->or(ref('agent')->same(lit('estimator'))))",
    );
  });

  it('reads two attributes as both of them', () => {
    const query = compile(draft({ filters: { agent: ['planner'], status: ['failed'] } }));

    expect(query.match(/->filter\(/g)).toHaveLength(2);
  });

  it('drops an attribute the grain cannot answer rather than naming a column that is not there', () => {
    // Model is a span-level fact. Emitting `ref('model')` against `runs` would
    // be a refusal from the service blamed on the person who clicked a chip
    // the builder itself offered.
    const query = compile(draft({ grain: 'runs', filters: { model: ['claude-opus-5'] } }));

    expect(query).not.toContain('model');
  });

  it('never lets a value reach the text unquoted', () => {
    const query = compile(draft({ filters: { agent: ["o'brien"] } }));

    expect(query).toContain("lit('o\\'brien')");
  });

  it('uses same rather than equals, because every column is nullable', () => {
    const query = compile(draft({ filters: { status: ['failed'] } }));

    expect(query).toContain('->same(');
    expect(query).not.toContain('->equals(');
  });

  it('selects columns rather than aggregating when nothing is grouped', () => {
    const query = compile(draft({ filters: { status: ['failed'] } }));

    expect(query).toContain('->select(');
    expect(query).not.toContain('->aggregate(');
    expect(query).toContain("->sortBy(ref('started_at')->desc())");
  });

  it('sorts a grouped query by its first number', () => {
    const query = compile(draft({ groupBy: ['agent'], metrics: ['input_tokens'] }));

    expect(query).toContain("->sortBy(ref('runs')->desc())");
  });

  it('always closes with an output sink and a run', () => {
    expect(compile(EMPTY_DRAFT).trimEnd()).toMatch(
      /->write\(to_output\(truncate: false\)\)\n {4}->run\(\);$/,
    );
  });
});

describe('selectedMetrics', () => {
  it('keeps the grain count first and unremovable', () => {
    // `aggregate()` with nothing in it is a refusal, and a group of keys with
    // no number beside it is a question somebody has to ask twice.
    const metrics = selectedMetrics(draft({ metrics: [] }));

    expect(metrics.map((metric) => metric.id)).toEqual(['runs']);
  });

  it('does not report the count twice when it is also chosen', () => {
    const metrics = selectedMetrics(draft({ metrics: ['runs', 'input_tokens'] }));

    expect(metrics.map((metric) => metric.id)).toEqual(['runs', 'input_tokens']);
  });

  it('drops a metric the grain does not have', () => {
    const metrics = selectedMetrics(draft({ grain: 'spans', metrics: ['input_tokens'] }));

    expect(metrics.map((metric) => metric.id)).toEqual(['spans']);
  });
});

describe('sortableColumns', () => {
  it('offers the keys and the numbers a grouped query produces', () => {
    expect(sortableColumns(draft({ groupBy: ['agent'], metrics: ['input_tokens'] }))).toEqual([
      'agent',
      'runs',
      'input_tokens',
    ]);
  });
});
