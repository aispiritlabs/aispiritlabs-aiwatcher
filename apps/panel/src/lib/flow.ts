import { z } from 'zod';

/**
 * The client for the Flow query service.
 *
 * Hand-written, unlike everything under `api/generated`: this service is not in
 * the Rust OpenAPI document and deliberately so — the panel talks to it
 * directly, and aiwatcher's binary does not know it exists (ADR_0008). That
 * makes runtime validation the right call here for the same reason it is for
 * the SSE frames: there is no codegen to lean on.
 *
 * It is also the one part of the panel that must work when its backend is
 * absent. `probe()` answering `false` is a normal state, not an error.
 */

const columnSchema = z.object({ name: z.string(), type: z.string() });

/**
 * A named argument a `read()` may carry, beyond the dataset name.
 *
 * Part of the contract rather than a hint: the aiwatcher API rejects unknown
 * query parameters, so a `read()` argument the dataset never declared turns
 * the whole query into a 400.
 */
const parameterSchema = z.object({
  name: z.string(),
  required: z.boolean(),
  description: z.string(),
  values: z.array(z.string()),
});

const datasetSchema = z.object({
  name: z.string(),
  aliases: z.array(z.string()),
  grain: z.string(),
  description: z.string(),
  requires_run: z.boolean(),
  columns: z.array(columnSchema),
  /** Optional so a panel build stays compatible with an older Flow service. */
  parameters: z.array(parameterSchema).optional().default([]),
});

const datasetsSchema = z.object({
  datasets: z.array(datasetSchema),
  source: z.string(),
  max_rows: z.number(),
});

const resultSchema = z.object({
  columns: z.array(z.string()),
  rows: z.array(z.record(z.string(), z.unknown())),
  row_count: z.number(),
  truncated: z.boolean(),
  /** From `to_output(truncate:)` — whether long cells may be shortened. */
  truncate_cells: z.boolean(),
  dataset: z.string().nullable(),
  grain: z.string().nullable(),
  source: z.string(),
  /** The window the rows were read through, so a short table reads as scoped. */
  window_seconds: z.number().nullable().optional(),
  took_ms: z.number(),
});

const diagnosticSchema = z.object({
  level: z.string(),
  message: z.string(),
  /** Offset into the query as typed — enrichment maps it back for us. */
  offset: z.number(),
  line: z.number(),
  help: z.string().nullable(),
});

const checkSchema = z.object({
  ok: z.boolean(),
  diagnostics: z.array(diagnosticSchema),
  /** Which checkers ran: `mago` for syntax, `aiwatcher` for the grammar and schema. */
  checked_by: z.array(z.string()),
});

const errorSchema = z.object({
  error: z.object({
    message: z.string(),
    column: z.number(),
    near: z.string().nullable().optional(),
  }),
});

export type FlowDiagnostic = z.infer<typeof diagnosticSchema>;
export type FlowCheck = z.infer<typeof checkSchema>;
export type FlowDataset = z.infer<typeof datasetSchema>;
export type FlowParameter = z.infer<typeof parameterSchema>;
export type FlowDatasets = z.infer<typeof datasetsSchema>;
export type FlowResult = z.infer<typeof resultSchema>;

/** A query the service refused, with where it gave up. */
export class FlowQueryError extends Error {
  constructor(
    message: string,
    readonly column: number,
  ) {
    super(message);
    this.name = 'FlowQueryError';
  }
}

/** Raised when the service itself is not there. Its own type, because the page treats it differently. */
export class FlowUnavailableError extends Error {
  constructor() {
    super('The Flow query service is not running.');
    this.name = 'FlowUnavailableError';
  }
}

async function call(path: string, init?: RequestInit): Promise<unknown> {
  let response: Response;

  try {
    response = await fetch(`/flow${path}`, init);
  } catch {
    // A refused connection is the service being absent, not a failed query.
    throw new FlowUnavailableError();
  }

  const body: unknown = await response.json().catch(() => null);
  const serviceError = errorSchema.safeParse(body);
  // A structured upstream failure proves Flow answered. Preserve its reason
  // (e.g. a dataset hub's 502); only an unstructured proxy failure means absent.
  if ([500, 502, 503, 504].includes(response.status) && !serviceError.success) {
    throw new FlowUnavailableError();
  }

  // 404 is a *different* process answering. Every path here is one the service
  // serves, so it never 404s on its own; what does is whatever else happens to
  // hold the port the proxy points at — measured against a Go service on :8081
  // answering `404 page not found` to everything. Reading that as a refused
  // query blames the pipeline for a port collision, and the message it produces
  // ("The service answered 404") sends the reader to the query they just wrote.
  if (response.status === 404) {
    throw new FlowUnavailableError();
  }

  if (!response.ok) {
    const parsed = serviceError;
    throw new FlowQueryError(
      parsed.success ? parsed.data.error.message : `The service answered ${response.status}.`,
      parsed.success ? parsed.data.error.column : 0,
    );
  }

  return body;
}

export async function isFlowAvailable(): Promise<boolean> {
  try {
    await call('/healthz');
    return true;
  } catch {
    return false;
  }
}

export async function fetchDatasets(): Promise<FlowDatasets> {
  return datasetsSchema.parse(await call('/datasets'));
}

/**
 * What is wrong with a query, without running it.
 *
 * Two checkers behind this: Mago parses the query as PHP (after the service
 * substitutes the bareword dataset names, which are not valid PHP) and reports
 * where the brackets stopped making sense; the service's own parser knows the
 * grammar, the whitelist and every column. Neither executes anything.
 */
export async function checkQuery(pipeline: string): Promise<FlowCheck> {
  return checkSchema.parse(
    await call('/check', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ pipeline }),
    }),
  );
}

/**
 * Run a query, scoped to the script's period or the panel's fallback window.
 *
 * `read(default, period: '24h')` is reproducible and wins when present. The
 * request parameter remains a fallback for older/ad-hoc scripts and is
 * forwarded only to aiwatcher routes that accept one.
 */
export async function runQuery(pipeline: string, windowSeconds?: number): Promise<FlowResult> {
  return resultSchema.parse(
    await call('/query', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(
        windowSeconds ? { pipeline, window_seconds: windowSeconds } : { pipeline },
      ),
    }),
  );
}

/**
 * Execute the same plan against a 25-row preview and persist nothing.
 *
 * This has its own endpoint rather than a client-side slice: the cap is
 * applied while Flow fetches, so a simulation cannot accidentally walk the
 * full result just to hide most of it in the browser.
 */
export async function simulateQuery(pipeline: string, windowSeconds?: number): Promise<FlowResult> {
  return resultSchema.parse(
    await call('/simulate', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(
        windowSeconds ? { pipeline, window_seconds: windowSeconds } : { pipeline },
      ),
    }),
  );
}

/**
 * The query the editor starts with.
 *
 * Deliberately the corrected form of the obvious first query rather than the
 * obvious one: `runs` carries `agents` as a list, so grouping by agent needs
 * the expansion. Starting from something that runs teaches the shape; starting
 * from something that errors teaches nothing.
 */
export const STARTER_QUERY = `data_frame()
    ->read(default)
    ->withEntry('agent', array_expand(ref('agents')))
    ->groupBy(ref('agent'))
    ->aggregate(
        count(ref('run_id')->as('runs')),
        sum(ref('input_tokens')->as('input_tokens'))
    )
    ->sortBy(ref('runs')->desc())
    ->write(to_output(truncate: false))
    ->run();`;

/** A useful curation: successful production runs, one case per session. */
export const STARTER_CURATION = `data_frame()
    ->read(default, period: '24h')
    ->filter(ref('status')->same(lit('succeeded')))
    ->dropDuplicates(ref('conversation_id'))
    ->rename('run_id', 'source_run_id')
    ->rename('conversation_id', 'source_session_id')
    ->rename('trace_id', 'source_trace_id')
    ->select(
        ref('source_run_id'),
        ref('source_session_id'),
        ref('source_trace_id'),
        ref('agents'),
        ref('started_at')
    )
    ->write(to_output(truncate: false))
    ->run();`;

/**
 * A curation somebody can load, read and edit into their own.
 *
 * The editor is one script over one engine, so an example here is one script —
 * and every one of them runs as it stands. Two read a public corpus through
 * `hub_rows` rather than the event log, because "what can this language
 * actually say" is a question about the language and not about aiwatcher's
 * own data, and a mixed-type corpus everybody already knows answers it faster
 * than a synthetic one.
 *
 * The Titanic set is what a Kaggle notebook starts with: recode the columns,
 * ask the table a question, and — since the query surface stopped being a
 * hand-written list — do the feature engineering that used to need a second
 * engine. `family_size` is arithmetic Flow always had, and filling a missing
 * age from its group's median is a window function; both were unreachable
 * because nobody had written their names down, not because the engine lacked
 * them.
 */
export type QueryExample = {
  title: string;
  /** What loading it puts in the recipe name box. */
  name: string;
  /** And in the target dataset box. */
  dataset: string;
  description: string;
  query: string;
};

export const QUERY_EXAMPLES: QueryExample[] = [
  {
    title: 'Successful sessions',
    name: 'production/successful-sessions',
    dataset: 'evaluation/production-sessions',
    description: 'Successful production runs from the last day, one case per session.',
    query: STARTER_CURATION,
  },
  {
    title: 'Titanic · sex and status',
    name: 'titanic/passengers',
    dataset: 'curation/titanic-passengers',
    description:
      'Recode sex, pull the title out of the name and collapse it to a status, band the age, and say what Survived and Embarked mean in words.',
    // The title is extracted with a replace rather than a match, because what
    // is wanted is the captured group and not whether the pattern fired. The
    // status is then narrowed one `when` at a time: each step reads the column
    // the step before it wrote, which keeps every line short enough to read
    // and to point a diagnostic at.
    //
    // The age band is the one recode that stays nested, and the null check has
    // to be the outermost one: a fifth of this corpus reports no age, and a
    // comparison is only reached when `when` takes that branch. `between()`
    // would be the natural way to write the bands and is the wrong one here —
    // it *throws* on a null rather than answering false, which turns a missing
    // age into a failed query.
    query: `data_frame()
    ->read(hub_rows, dataset: 'phihung/titanic', split: 'train', limit: 100)
    ->withEntry('name', array_get(ref('row'), 'Name'))
    ->withEntry('sex', array_get(ref('row'), 'Sex'))
    ->withEntry('age', array_get(ref('row'), 'Age'))
    ->withEntry('pclass', array_get(ref('row'), 'Pclass'))
    ->withEntry('fare', array_get(ref('row'), 'Fare'))
    ->withEntry('embarked', array_get(ref('row'), 'Embarked'))
    ->withEntry('survived', array_get(ref('row'), 'Survived'))
    ->withEntry('sex_code', when(ref('sex')->same(lit('female')), lit(1), lit(0)))
    ->withEntry('title', regex_replace(lit('/^[^,]*,\\s*([^.]+)\\..*$/'), lit('$1'), ref('name')))
    ->withEntry('status', lit('rare'))
    ->withEntry('status', when(ref('title')->same(lit('Mr')), lit('mr'), ref('status')))
    ->withEntry('status', when(ref('title')->same(lit('Master')), lit('master'), ref('status')))
    ->withEntry('status', when(any(ref('title')->same(lit('Mrs')), ref('title')->same(lit('Mme'))), lit('mrs'), ref('status')))
    ->withEntry('status', when(any(ref('title')->same(lit('Miss')), ref('title')->same(lit('Mlle')), ref('title')->same(lit('Ms'))), lit('miss'), ref('status')))
    ->withEntry('outcome', when(ref('survived')->same(lit(1)), lit('survived'), lit('died')))
    ->withEntry('port', lit('unknown'))
    ->withEntry('port', when(ref('embarked')->same(lit('S')), lit('Southampton'), ref('port')))
    ->withEntry('port', when(ref('embarked')->same(lit('C')), lit('Cherbourg'), ref('port')))
    ->withEntry('port', when(ref('embarked')->same(lit('Q')), lit('Queenstown'), ref('port')))
    ->withEntry('age_band', when(ref('age')->isNull(), lit('unknown'),
        when(ref('age')->lessThan(lit(13)), lit('child'),
        when(ref('age')->lessThan(lit(20)), lit('teen'),
        when(ref('age')->lessThan(lit(60)), lit('adult'), lit('senior'))))))
    ->select(
        ref('name'),
        ref('sex'),
        ref('sex_code'),
        ref('title'),
        ref('status'),
        ref('age'),
        ref('age_band'),
        ref('pclass'),
        ref('fare'),
        ref('port'),
        ref('outcome')
    )
    ->write(to_output(truncate: false))
    ->run();`,
  },
  {
    title: 'Titanic · features in one query',
    name: 'titanic/features',
    dataset: 'curation/titanic-features-query',
    description:
      "The whole feature engineering as one Flow query: recode, fill a missing age from its status group's median, size the family, and keep the guess distinguishable from the measurement.",
    // Three things here were impossible in this language a day ago, and none
    // of them was ever impossible in Flow:
    //
    //   ->over(window()->partitionBy(...))   a group's answer, beside the row
    //   ->plus(...) / ->divide(...)          arithmetic, on Flow's reference
    //   coalesce(ref('age'), ref('typical')) the fill itself
    //
    // `age` keeps its null and `age_imputed` says what happened, because a
    // dataset in which a measurement and a guess are the same column is one
    // nothing downstream can take apart again.
    //
    // The division names its rounding, and has to: Flow's scale defaults to
    // "no rounding necessary", which throws the moment a fare does not divide
    // exactly.
    query: `data_frame()
    ->read(hub_rows, dataset: 'phihung/titanic', split: 'train', limit: 100)
    ->withEntry('name', array_get(ref('row'), 'Name'))
    ->withEntry('sex', array_get(ref('row'), 'Sex'))
    ->withEntry('age', array_get(ref('row'), 'Age'))
    ->withEntry('sib_sp', array_get(ref('row'), 'SibSp'))
    ->withEntry('parch', array_get(ref('row'), 'Parch'))
    ->withEntry('fare', array_get(ref('row'), 'Fare'))
    ->withEntry('sex_code', when(ref('sex')->same(lit('female')), lit(1), lit(0)))
    ->withEntry('title', regex_replace(lit('/^[^,]*,\\s*([^.]+)\\..*$/'), lit('$1'), ref('name')))
    ->withEntry('status', lit('rare'))
    ->withEntry('status', when(ref('title')->same(lit('Mr')), lit('mr'), ref('status')))
    ->withEntry('status', when(ref('title')->same(lit('Master')), lit('master'), ref('status')))
    ->withEntry('status', when(any(ref('title')->same(lit('Mrs')), ref('title')->same(lit('Mme'))), lit('mrs'), ref('status')))
    ->withEntry('status', when(any(ref('title')->same(lit('Miss')), ref('title')->same(lit('Mlle')), ref('title')->same(lit('Ms'))), lit('miss'), ref('status')))
    ->withEntry('age_imputed', ref('age')->isNull())
    ->withEntry('typical_age', median(ref('age'))->over(window()->partitionBy(ref('status'))))
    ->withEntry('age_filled', coalesce(ref('age'), ref('typical_age')))
    ->withEntry('family_size', ref('sib_sp')->plus(ref('parch'))->plus(lit(1)))
    ->withEntry('is_alone', ref('family_size')->same(lit(1)))
    ->withEntry('fare_per_person', ref('fare')->divide(ref('family_size'), lit(2), 'half_up'))
    ->select(
        ref('name'),
        ref('status'),
        ref('sex_code'),
        ref('age'),
        ref('age_filled'),
        ref('age_imputed'),
        ref('family_size'),
        ref('is_alone'),
        ref('fare_per_person')
    )
    ->write(to_output(truncate: false))
    ->run();`,
  },
  {
    title: 'Titanic · age and fare spread',
    name: 'titanic/age-and-fare',
    dataset: 'curation/titanic-age-and-fare',
    description:
      'Median, deviation and a 90th percentile per class — the half of describe() Flow does not ship, and this service does.',
    // `median`, `stddev`, `variance` and `percentile` are not Flow's: they are
    // `services/flow`'s own aggregations over hi-folks/statistics, listed in
    // the whitelist by name (ADR_0008's amendment). A mean and a median are
    // different numbers on fares, and a curation that published only the first
    // leaves its reader unable to tell which one they got.
    //
    // A group with fewer values than a statistic is defined over answers null
    // rather than zero, so a class with one passenger is visibly unmeasured.
    query: `data_frame()
    ->read(hub_rows, dataset: 'phihung/titanic', split: 'train', limit: 100)
    ->withEntry('pclass', array_get(ref('row'), 'Pclass'))
    ->withEntry('age', array_get(ref('row'), 'Age'))
    ->withEntry('fare', array_get(ref('row'), 'Fare'))
    ->groupBy(ref('pclass'))
    ->aggregate(
        count(ref('pclass')->as('passengers')),
        average(ref('age')->as('mean_age')),
        median(ref('age')),
        stddev(ref('age')->as('age_spread')),
        median(ref('fare')),
        percentile(ref('fare')->as('fare_p90'), 90)
    )
    ->withEntry('age_spread', round(ref('age_spread'), lit(2)))
    ->sortBy(ref('pclass')->asc())
    ->write(to_output(truncate: false))
    ->run();`,
  },
  {
    title: 'Titanic · survival by status',
    name: 'titanic/survival-by-status',
    dataset: 'curation/titanic-survival-by-status',
    description:
      'Survival rate and headcount by status and class, sorted by rate. The first table every Titanic notebook draws.',
    // `average` answers to two decimals, so nothing here rounds: a second
    // rounding would be a rule this file invented about somebody else's
    // number. `count` is over `survived` rather than over a row, which is the
    // honest headcount — a passenger the corpus says nothing about is not one
    // this rate was measured on.
    query: `data_frame()
    ->read(hub_rows, dataset: 'phihung/titanic', split: 'train', limit: 100)
    ->withEntry('name', array_get(ref('row'), 'Name'))
    ->withEntry('sex', array_get(ref('row'), 'Sex'))
    ->withEntry('pclass', array_get(ref('row'), 'Pclass'))
    ->withEntry('survived', array_get(ref('row'), 'Survived'))
    ->withEntry('title', regex_replace(lit('/^[^,]*,\\s*([^.]+)\\..*$/'), lit('$1'), ref('name')))
    ->withEntry('status', lit('rare'))
    ->withEntry('status', when(ref('title')->same(lit('Mr')), lit('mr'), ref('status')))
    ->withEntry('status', when(ref('title')->same(lit('Master')), lit('master'), ref('status')))
    ->withEntry('status', when(any(ref('title')->same(lit('Mrs')), ref('title')->same(lit('Mme'))), lit('mrs'), ref('status')))
    ->withEntry('status', when(any(ref('title')->same(lit('Miss')), ref('title')->same(lit('Mlle')), ref('title')->same(lit('Ms'))), lit('miss'), ref('status')))
    ->groupBy(ref('status'), ref('sex'), ref('pclass'))
    ->aggregate(
        count(ref('survived')->as('passengers')),
        average(ref('survived')->as('survival_rate'))
    )
    ->sortBy(ref('survival_rate')->desc())
    ->write(to_output(truncate: false))
    ->run();`,
  },
];
