import type { DimensionKind } from '@/api/generated/types.gen';
import type { QueryEngineName } from '@/shared/lib/query';

/**
 * Building a query by clicking, and compiling it to the text the deployed engine
 * runs — Flow PHP, DataFusion's Python API or DuckDB's relational API (AW-3).
 *
 * A generator, and one-way. Its text goes through `/query/check` and
 * `/query/query` exactly as hand-written text does, the engine alone decides
 * whether it is valid, and text never parses back into blocks — CLAUDE.md's
 * Panel section and its query-builder guardrail say why. In `build` mode the
 * draft is the truth and the text is derived from it; in `write` mode the text
 * is, and the draft is gone.
 *
 * The attribute names are the live stream's — `agent`, `runtime`, `workflow`,
 * `session` — so "watch this live" is a link rather than a translation. Only the
 * encoding differs: the router writes an array as JSON, the stream takes
 * repeated parameters, and `selectionQuery` in `shared/lib/live.ts` converts.
 */

export type Grain = 'runs' | 'spans';

/** An attribute somebody can click. Seven come from the log, one is a fixed word. */
export type AttributeId = DimensionKind | 'status';

/** How one grain reaches one attribute. Absent means that grain cannot answer it. */
interface Reach {
  /** The column the query filters and groups on. */
  column: string;
  /**
   * The list column it has to be expanded out of first.
   *
   * A run involves several agents and is produced by several services, so on
   * the `runs` grain both are list columns and grouping by one needs a row per
   * value. Getting this wrong is the single most common first query, which is
   * why the query service ships a hint for exactly it — and why the builder
   * emits the expansion rather than leaving somebody to be told.
   */
  expandFrom?: string;
}

export interface Attribute {
  id: AttributeId;
  label: string;
  /** The dimension route its observed values come from. Absent for a fixed vocabulary. */
  dimension?: DimensionKind;
  /** A fixed vocabulary, for an attribute no dimension route lists. */
  values?: string[];
  reach: Partial<Record<Grain, Reach>>;
  /** Said on the chip when the current grain cannot answer it. */
  unavailable: string;
}

export const ATTRIBUTES: Attribute[] = [
  {
    id: 'agent',
    label: 'Agent',
    dimension: 'agent',
    reach: {
      runs: { column: 'agent', expandFrom: 'agents' },
      spans: { column: 'agent_id' },
    },
    unavailable: '',
  },
  {
    id: 'model',
    label: 'Model',
    dimension: 'model',
    reach: { spans: { column: 'model' } },
    unavailable:
      'A run does not carry the models it used; its spans do. Switch the grain to spans.',
  },
  {
    id: 'tool',
    label: 'Tool',
    dimension: 'tool',
    reach: { spans: { column: 'tool' } },
    unavailable:
      'A run does not carry the tools it called; its spans do. Switch the grain to spans.',
  },
  {
    id: 'runtime',
    label: 'Runtime',
    dimension: 'runtime',
    reach: { runs: { column: 'runtime', expandFrom: 'runtimes' } },
    unavailable: 'A span does not name the service that produced it. Switch the grain to runs.',
  },
  {
    id: 'workflow',
    label: 'Workflow',
    dimension: 'workflow',
    reach: { runs: { column: 'workflow' } },
    unavailable: 'A span does not name its workflow. Switch the grain to runs.',
  },
  {
    id: 'session',
    label: 'Session',
    dimension: 'session',
    reach: { runs: { column: 'conversation_id' } },
    unavailable: 'A span does not name its session. Switch the grain to runs.',
  },
  {
    id: 'trace',
    label: 'Trace',
    dimension: 'trace',
    reach: { runs: { column: 'trace_id' }, spans: { column: 'trace_id' } },
    unavailable: '',
  },
  {
    id: 'status',
    label: 'Status',
    // No dimension route lists these, and there are three of them — a fixed
    // vocabulary is honest here in a way it would not be for an agent id.
    values: ['running', 'succeeded', 'failed'],
    reach: { runs: { column: 'status' } },
    unavailable: 'A span has no run status. Switch the grain to runs.',
  },
];

export function attribute(id: AttributeId): Attribute | undefined {
  return ATTRIBUTES.find((candidate) => candidate.id === id);
}

export interface MetricSpec {
  id: string;
  label: string;
  grains: Grain[];
  fn: 'count' | 'sum' | 'average' | 'max' | 'median';
  column: string;
}

/**
 * What a grouped query can count.
 *
 * `average` and `median` rather than `avg` and `mean`: the first is Flow's own
 * name and the second is the query service's own aggregation. A name this file
 * invented would compile to text the parser refuses, which the reader would
 * read as their query being wrong.
 */
export const METRICS: MetricSpec[] = [
  { id: 'runs', label: 'Runs', grains: ['runs'], fn: 'count', column: 'run_id' },
  { id: 'spans', label: 'Spans', grains: ['spans'], fn: 'count', column: 'span_id' },
  {
    id: 'input_tokens',
    label: 'Input tokens',
    grains: ['runs'],
    fn: 'sum',
    column: 'input_tokens',
  },
  {
    id: 'output_tokens',
    label: 'Output tokens',
    grains: ['runs'],
    fn: 'sum',
    column: 'output_tokens',
  },
  {
    id: 'cached_tokens',
    label: 'Cached tokens',
    grains: ['runs'],
    fn: 'sum',
    column: 'cached_tokens',
  },
  { id: 'llm_calls', label: 'LLM calls', grains: ['runs'], fn: 'sum', column: 'llm_calls' },
  { id: 'tool_calls', label: 'Tool calls', grains: ['runs'], fn: 'sum', column: 'tool_calls' },
  {
    id: 'avg_duration_ms',
    label: 'Average duration',
    grains: ['runs', 'spans'],
    fn: 'average',
    column: 'duration_ms',
  },
  {
    id: 'median_duration_ms',
    label: 'Median duration',
    grains: ['runs', 'spans'],
    fn: 'median',
    column: 'duration_ms',
  },
  {
    id: 'max_duration_ms',
    label: 'Slowest',
    grains: ['runs', 'spans'],
    fn: 'max',
    column: 'duration_ms',
  },
];

/** The count every grain has, and the one a grouped query falls back to. */
export function countMetric(grain: Grain): MetricSpec {
  const metric = METRICS.find(
    (candidate) => candidate.fn === 'count' && candidate.grains.includes(grain),
  );
  if (!metric) throw new Error(`no count metric for the ${grain} grain`);
  return metric;
}

/** The columns a flat, ungrouped list shows. */
const COLUMNS: Record<Grain, string[]> = {
  runs: ['run_id', 'status', 'workflow', 'started_at', 'duration_ms', 'llm_calls', 'input_tokens'],
  spans: ['span_id', 'name', 'agent_id', 'model', 'tool', 'start', 'duration_ms'],
};

/** What a flat list is ordered by when nothing was chosen. */
const NEWEST: Record<Grain, string> = { runs: 'started_at', spans: 'start' };

export interface QueryDraft {
  grain: Grain;
  /** Attribute id → the values chosen. Or within one, and across them. */
  filters: Partial<Record<AttributeId, string[]>>;
  groupBy: AttributeId[];
  metrics: string[];
  sort?: { by: string; direction: 'asc' | 'desc' };
  limit?: number;
}

export const EMPTY_DRAFT: QueryDraft = {
  grain: 'runs',
  filters: {},
  groupBy: [],
  metrics: [],
};

/** The filters that this grain can actually express, in a stable order. */
function reachableFilters(draft: QueryDraft): Array<{ reach: Reach; values: string[] }> {
  return ATTRIBUTES.flatMap((candidate) => {
    const values = (draft.filters[candidate.id] ?? []).filter(Boolean);
    const reach = candidate.reach[draft.grain];
    return values.length > 0 && reach ? [{ reach, values }] : [];
  });
}

/** The group-by columns this grain can express, in the order they were chosen. */
function reachableGroups(draft: QueryDraft): Reach[] {
  return draft.groupBy.flatMap((id) => {
    const reach = attribute(id)?.reach[draft.grain];
    return reach ? [reach] : [];
  });
}

const quote = (value: string) => `'${value.replace(/\\/g, '\\\\').replace(/'/g, "\\'")}'`;

/** One dimension's predicate: or within, which is what a chip row means. */
function predicate(column: string, values: string[]): string {
  return (
    values
      .map((value) => `ref(${quote(column)})->same(lit(${quote(value)}))`)
      // `same` rather than `equals`: every column here is nullable and Flow's
      // loose comparison matches null against anything. The Datasets panel says
      // the same thing, and the query service refuses `equals` outright.
      .join('->or(')
  );
}

function closed(column: string, values: string[]): string {
  return `${predicate(column, values)}${')'.repeat(values.length - 1)}`;
}

/**
 * The query this draft is, in the language `engine` reads.
 *
 * Formatted the way the starter query is, because the text is not a hidden
 * intermediate — it is shown beside the builder and is what somebody takes
 * into the editor when the clicking runs out. One vocabulary of chips for
 * every language: a chip compiles to something in each, and one the grain
 * cannot answer is dropped the same way in each, so Build mode still refuses
 * nothing.
 */
export function compile(draft: QueryDraft, engine: QueryEngineName = 'flow'): string {
  switch (engine) {
    case 'flow':
      return compileFlow(draft);
    case 'datafusion':
      return compileDataFusion(draft);
    case 'duckdb':
      return compileDuckDB(draft);
  }
}

/**
 * The list columns to expand first, keyed by the column each becomes.
 *
 * De-duplicated: grouping by agent and filtering on it both need the same
 * row-per-agent, and expanding twice would multiply every row by the number of
 * agents a second time.
 */
function expansions(draft: QueryDraft): Map<string, string> {
  const found = new Map<string, string>();
  for (const { reach } of reachableFilters(draft)) {
    if (reach.expandFrom) found.set(reach.column, reach.expandFrom);
  }
  for (const reach of reachableGroups(draft)) {
    if (reach.expandFrom) found.set(reach.column, reach.expandFrom);
  }
  return found;
}

/** The Flow pipeline this draft is. */
function compileFlow(draft: QueryDraft): string {
  const lines = ['data_frame()', `    ->read(${draft.grain})`];

  for (const [column, from] of expansions(draft)) {
    lines.push(`    ->withEntry(${quote(column)}, array_expand(ref(${quote(from)})))`);
  }

  // One `filter` per dimension rather than one big conjunction: and across
  // them is exactly what a chain of filters is, and each line stays readable
  // as the thing one chip row asked for.
  for (const { reach, values } of reachableFilters(draft)) {
    lines.push(`    ->filter(${closed(reach.column, values)})`);
  }

  const groups = reachableGroups(draft);
  const metrics = selectedMetrics(draft);

  if (groups.length > 0) {
    lines.push(`    ->groupBy(${groups.map((reach) => `ref(${quote(reach.column)})`).join(', ')})`);
    lines.push('    ->aggregate(');
    lines.push(
      metrics
        .map(
          (metric) => `        ${metric.fn}(ref(${quote(metric.column)})->as(${quote(metric.id)}))`,
        )
        .join(',\n'),
    );
    lines.push('    )');
  } else {
    const columns = COLUMNS[draft.grain];
    lines.push(`    ->select(${columns.map((column) => `ref(${quote(column)})`).join(', ')})`);
  }

  const sort = draft.sort ?? defaultSort(draft, groups.length > 0, metrics);
  lines.push(`    ->sortBy(ref(${quote(sort.by)})->${sort.direction}())`);

  if (draft.limit) lines.push(`    ->limit(${draft.limit})`);

  lines.push('    ->write(to_output(truncate: false))');
  lines.push('    ->run();');

  return lines.join('\n');
}

/** A value as Python writes it: JSON's string form is a valid Python string literal. */
const python = (value: string) => JSON.stringify(value);

/** Each metric's DataFusion function. */
const DATAFUSION_FUNCTION: Record<MetricSpec['fn'], string> = {
  count: 'count',
  sum: 'sum',
  average: 'avg',
  max: 'max',
  median: 'median',
};

/**
 * The DataFusion query this draft is: DataFusion's Python API, one method per
 * line inside parentheses, which is how a chain spans lines in Python.
 *
 * The Flow emitter's shapes in DataFusion's words: a list column is unnested
 * before it is grouped, one chip row's values are an `|`, and nothing reaches
 * the text except through `JSON.stringify`. `==` against a null is null, which
 * a filter drops — the answer Flow's `same` gives.
 */
function compileDataFusion(draft: QueryDraft): string {
  const lines = ['(', `    read(${python(draft.grain)})`];

  for (const [column, from] of expansions(draft)) {
    lines.push(`    .with_column(${python(column)}, col(${python(from)}))`);
    lines.push(`    .unnest_columns(${python(column)})`);
  }

  for (const { reach, values } of reachableFilters(draft)) {
    const terms = values.map((value) => `col(${python(reach.column)}) == lit(${python(value)})`);
    const condition = terms.length > 1 ? terms.map((term) => `(${term})`).join(' | ') : terms[0];
    lines.push(`    .filter(${condition})`);
  }

  const groups = reachableGroups(draft);
  const metrics = selectedMetrics(draft);

  if (groups.length > 0) {
    lines.push('    .aggregate(');
    lines.push(`        [${groups.map((reach) => `col(${python(reach.column)})`).join(', ')}],`);
    lines.push('        [');
    for (const metric of metrics) {
      const fn = DATAFUSION_FUNCTION[metric.fn];
      lines.push(`            f.${fn}(col(${python(metric.column)})).alias(${python(metric.id)}),`);
    }
    lines.push('        ],');
    lines.push('    )');
  } else {
    const columns = COLUMNS[draft.grain];
    lines.push(`    .select(${columns.map((column) => `col(${python(column)})`).join(', ')})`);
  }

  const sort = draft.sort ?? defaultSort(draft, groups.length > 0, metrics);
  const ascending = sort.direction === 'asc' ? 'True' : 'False';
  lines.push(`    .sort(col(${python(sort.by)}).sort(ascending=${ascending}))`);

  if (draft.limit) lines.push(`    .limit(${draft.limit})`);
  lines.push(')');

  return lines.join('\n');
}

/** Each metric's DuckDB function, named as `FunctionExpression` takes it. */
const DUCKDB_FUNCTION: Record<MetricSpec['fn'], string> = {
  count: 'count',
  sum: 'sum',
  average: 'avg',
  max: 'max',
  median: 'median',
};

/**
 * The DuckDB query this draft is: DuckDB's relational API, laid out as the
 * DataFusion one is.
 *
 * A relation has no `with_column`, so a list column becomes one row per value
 * by projecting every column beside an `unnest` of it. A grouped query lists
 * its keys and its numbers together and names the keys again as the group —
 * bare column names, the one text a strict DuckDB query hands a relation, since
 * DuckDB parses any other as SQL.
 */
function compileDuckDB(draft: QueryDraft): string {
  const column = (name: string) => `ColumnExpression(${python(name)})`;
  const constant = (value: string) => `ConstantExpression(${python(value)})`;
  const lines = ['(', `    read(${python(draft.grain)})`];

  for (const [name, from] of expansions(draft)) {
    lines.push(
      `    .project(StarExpression(), FunctionExpression("unnest", ${column(from)}).alias(${python(name)}))`,
    );
  }

  for (const { reach, values } of reachableFilters(draft)) {
    const terms = values.map((value) => `${column(reach.column)} == ${constant(value)}`);
    const condition = terms.length > 1 ? terms.map((term) => `(${term})`).join(' | ') : terms[0];
    lines.push(`    .filter(${condition})`);
  }

  const groups = reachableGroups(draft);
  const metrics = selectedMetrics(draft);

  if (groups.length > 0) {
    lines.push('    .aggregate(', '        [');
    for (const reach of groups) lines.push(`            ${column(reach.column)},`);
    for (const metric of metrics) {
      const fn = python(DUCKDB_FUNCTION[metric.fn]);
      lines.push(
        `            FunctionExpression(${fn}, ${column(metric.column)}).alias(${python(metric.id)}),`,
      );
    }
    lines.push('        ],');
    lines.push(`        ${python(groups.map((reach) => reach.column).join(', '))},`);
    lines.push('    )');
  } else {
    lines.push(`    .select(${COLUMNS[draft.grain].map(column).join(', ')})`);
  }

  const sort = draft.sort ?? defaultSort(draft, groups.length > 0, metrics);
  lines.push(`    .sort(${column(sort.by)}.${sort.direction}())`);

  if (draft.limit) lines.push(`    .limit(${draft.limit})`);
  lines.push(')');

  return lines.join('\n');
}

/**
 * The metrics a grouped query reports.
 *
 * The grain's count is always first and cannot be removed: `aggregate()` with
 * nothing in it is a refusal, and a group with no number beside it is a list
 * of keys somebody has to ask a second question about.
 */
export function selectedMetrics(draft: QueryDraft): MetricSpec[] {
  const count = countMetric(draft.grain);
  const chosen = draft.metrics.flatMap((id) => {
    if (id === count.id) return [];
    const metric = METRICS.find((candidate) => candidate.id === id);
    return metric && metric.grains.includes(draft.grain) ? [metric] : [];
  });
  return [count, ...chosen];
}

/** Biggest group first when grouped, newest first when not. */
function defaultSort(
  draft: QueryDraft,
  grouped: boolean,
  metrics: MetricSpec[],
): { by: string; direction: 'asc' | 'desc' } {
  const by = grouped ? (metrics[0]?.id ?? countMetric(draft.grain).id) : NEWEST[draft.grain];
  return { by, direction: 'desc' };
}

/** What the sort control may offer: the keys and the numbers the query produces. */
export function sortableColumns(draft: QueryDraft): string[] {
  const groups = reachableGroups(draft);
  if (groups.length === 0) return COLUMNS[draft.grain];
  return [
    ...groups.map((reach) => reach.column),
    ...selectedMetrics(draft).map((metric) => metric.id),
  ];
}

/** Whether anything at all has been narrowed. Drives the empty state, nothing else. */
export function isBlank(draft: QueryDraft): boolean {
  return (
    draft.groupBy.length === 0 &&
    draft.metrics.length === 0 &&
    Object.values(draft.filters).every((values) => (values ?? []).length === 0)
  );
}
