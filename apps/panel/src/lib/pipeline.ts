import type {
  BlockSpec,
  PipelineBlock,
  PipelineEdge,
  StepBlocks,
  StepState,
} from '@/api/generated/types.gen';
import { runQuery, simulateQuery, type FlowResult } from '@/lib/flow';
import { getNotebook, runNotebook, type NotebookRun } from '@/lib/ml-pipeline';

/**
 * A curation as a chain of blocks: what compiles, what runs, and in what order.
 *
 * The chain is driven from here rather than from a service, because each block
 * belongs to a different engine and two of the three engines are optional
 * services the aiwatcher binary does not know exist. What the browser does is
 * hand one block's rows to the next one — see ADR_0024.
 *
 * The rules a chain has to satisfy are **not** here. The registry refuses a
 * pipeline that is not runnable and reports every problem at once, and the
 * canvas renders exactly those lines, exactly as the annotation canvas does
 * not re-implement the shape validator. What this file does is *traverse*: it
 * answers "in what order do these blocks run", and `null` when there is no one
 * order — which is a question, not a second rule set.
 */

export type Row = Record<string, unknown>;

/**
 * The chain, head first, or `null` when these blocks are not one.
 *
 * A pure traversal: one block with nothing feeding it, one next per block,
 * everything reached. Nothing here explains *why* a shape was refused — that
 * answer comes from the registry, in one place, with every reason in it.
 */
export function orderOf(blocks: PipelineBlock[], edges: PipelineEdge[]): PipelineBlock[] | null {
  if (blocks.length === 0) return null;

  const byId = new Map(blocks.map((block) => [block.id, block]));
  const next = new Map<string, string>();
  const fed = new Set<string>();

  for (const edge of edges) {
    if (!byId.has(edge.from) || !byId.has(edge.to)) return null;
    if (next.has(edge.from) || fed.has(edge.to)) return null;
    next.set(edge.from, edge.to);
    fed.add(edge.to);
  }

  const heads = blocks.filter((block) => !fed.has(block.id));
  const head = heads.length === 1 ? heads[0] : undefined;
  if (!head) return null;

  const chain: PipelineBlock[] = [];
  const walked = new Set<string>();
  let cursor: string | undefined = head.id;
  while (cursor && !walked.has(cursor)) {
    walked.add(cursor);
    const block = byId.get(cursor);
    if (!block) return null;
    chain.push(block);
    cursor = next.get(cursor);
  }
  return chain.length === blocks.length ? chain : null;
}

/**
 * The Flow PHP script the source and transform blocks add up to.
 *
 * Flow executes one pipeline, so every block before the first notebook is one
 * query. That is worth showing rather than hiding: the panel puts this script
 * in front of the reader, and it is the same text `check` and `simulate` are
 * given, so a diagnostic's character offset points into what they can see.
 */
export function compileFlow(chain: PipelineBlock[]): string {
  const source = chain.find((block) => block.spec.kind === 'source');
  const steps = chain
    .filter((block) => block.spec.kind === 'transform')
    .flatMap((block) => (block.spec.kind === 'transform' ? (block.spec.steps ?? '') : ''))
    .join('\n');

  const lines = [
    'data_frame()',
    `    ${source ? readCall(source.spec) : '->read(default)'}`,
    ...steps
      .split('\n')
      .map((line) => line.trim())
      .filter(Boolean)
      .map((line) => `    ${line}`),
    '    ->write(to_output(truncate: false))',
    '    ->run();',
  ];
  return lines.join('\n');
}

/** One `read()`, with the arguments the catalog declares for that dataset. */
export function readCall(spec: BlockSpec): string {
  if (spec.kind !== 'source') return '->read(default)';
  const args = Object.entries(spec.arguments ?? {})
    .filter(([, value]) => value.trim() !== '')
    .map(([name, value]) => `${name}: ${literal(value)}`);
  return `->read(${[spec.dataset, ...args].join(', ')})`;
}

/**
 * A value as the query language writes it.
 *
 * Numbers unquoted because `limit: 25` is how somebody would type it; anything
 * else single-quoted, because the lexer refuses double quotes outright. A
 * value carrying a quote is escaped rather than dropped: the service will
 * refuse what it cannot lex, and it says where.
 */
function literal(value: string): string {
  if (/^-?\d+(\.\d+)?$/.test(value)) return value;
  return `'${value.replace(/\\/g, '\\\\').replace(/'/g, "\\'")}'`;
}

/**
 * Every notebook block, pinned to the source that is there now.
 *
 * A pipeline records which code it ran, and the code lives in the notebook
 * directory — so the pin is taken at save time. A runtime that is not answering
 * leaves each block's existing pin alone rather than clearing it: an unpinned
 * block is a pipeline that cannot say what it ran, which is worse than one
 * pinned to a revision somebody has since edited (the editor says so when they
 * have).
 */
export async function withPinnedNotebooks(blocks: PipelineBlock[]): Promise<PipelineBlock[]> {
  return Promise.all(
    blocks.map(async (block) => {
      if (block.spec.kind !== 'notebook') return block;
      try {
        const notebook = await getNotebook(block.spec.notebook);
        return { ...block, spec: { ...block.spec, revision: notebook.revision } };
      } catch {
        return block;
      }
    }),
  );
}

/**
 * How one block is drawn, from either of the two things that can drive it.
 *
 * The ad-hoc path knows how many rows a block produced and how long it took,
 * because the browser ran it. A managed run knows neither — the projection is
 * the run's *state*, and its timings are the log's answer (the guardrail
 * against a second copy in the workflow store). So the counts are optional and
 * `note` is what a managed step says instead: the word the server used.
 */
export type BlockOutcome =
  | { status: 'idle'; note?: string }
  | { status: 'running'; note?: string }
  | { status: 'done'; rows?: number; columns?: string[]; tookMs?: number; note?: string }
  | { status: 'failed'; message: string; stderr?: string };

export type PipelineOutcomes = Record<string, BlockOutcome>;

export type PipelineResult = {
  script: string;
  flow: FlowResult;
  notebooks: NotebookRun[];
  rows: Row[];
  columns: string[];
  outcomes: PipelineOutcomes;
};

/**
 * Run the chain, one engine at a time, reporting each block as it goes.
 *
 * `preview` simulates 25 rows and persists nothing; `full` runs the query for
 * real. Both stage their rows into every notebook they reach, which is what
 * makes the live app in the block's editor show the rows this chain actually
 * produced rather than whatever was there last week.
 */
export async function runPipeline(options: {
  chain: PipelineBlock[];
  mode: 'preview' | 'full';
  windowSeconds?: number;
  onOutcome?: (id: string, outcome: BlockOutcome) => void;
}): Promise<PipelineResult> {
  const { chain, mode, windowSeconds, onOutcome } = options;
  const outcomes: PipelineOutcomes = {};
  const report = (id: string, outcome: BlockOutcome) => {
    outcomes[id] = outcome;
    onOutcome?.(id, outcome);
  };

  const script = compileFlow(chain);
  const flowBlocks = chain.filter(
    (block) => block.spec.kind === 'source' || block.spec.kind === 'transform',
  );
  for (const block of flowBlocks) report(block.id, { status: 'running' });

  let flow: FlowResult;
  try {
    flow =
      mode === 'preview'
        ? await simulateQuery(script, windowSeconds)
        : await runQuery(script, windowSeconds);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    for (const block of flowBlocks) report(block.id, { status: 'failed', message });
    throw error;
  }

  for (const block of flowBlocks) {
    report(block.id, {
      status: 'done',
      rows: flow.row_count,
      columns: flow.columns,
      tookMs: flow.took_ms,
      // Said out loud because it is surprising the first time: three boxes
      // lighting up together is one query, not three.
      note: flowBlocks.length > 1 ? 'run as one Flow query' : undefined,
    });
  }

  let rows: Row[] = flow.rows;
  let columns = flow.columns;
  const notebooks: NotebookRun[] = [];

  for (const block of chain) {
    if (block.spec.kind !== 'notebook') continue;
    report(block.id, { status: 'running' });
    try {
      const run = await runNotebook(block.spec.notebook, rows, block.spec.params ?? {});
      notebooks.push(run);
      rows = run.rows;
      columns = run.columns;
      report(block.id, {
        status: 'done',
        rows: run.row_count,
        columns: run.columns,
        tookMs: run.took_ms,
        note: run.truncated ? 'more rows than this service returns' : undefined,
      });
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      const stderr = error instanceof Error && 'stderr' in error ? String(error.stderr) : undefined;
      report(block.id, { status: 'failed', message, stderr });
      throw error;
    }
  }

  for (const block of chain) {
    if (block.spec.kind !== 'view') continue;
    report(block.id, {
      status: 'done',
      rows: rows.length,
      columns,
      tookMs: 0,
    });
  }

  return { script, flow, notebooks, rows, columns, outcomes };
}

/**
 * A chain somebody can load, run and then edit into their own.
 *
 * Each one is a real chain over a public corpus rather than a shape with
 * placeholder names in it: loading one and pressing Preview reaches Hugging
 * Face, the Flow service and — where the chain has a notebook — the notebook
 * runtime. `title` is what the button says; `needsNotebooks` is why a chain
 * that has no notebook block is worth shipping, because it is the one that
 * still runs with that service switched off.
 */
export type CurationExample = {
  name: string;
  title: string;
  description: string;
  needsNotebooks: boolean;
  blocks: PipelineBlock[];
  edges: PipelineEdge[];
};

/**
 * The example the area shipped with first, end to end.
 *
 * A public corpus of text with personal data in it, narrowed to something a
 * person can read, scanned by a notebook that masks what it finds, and a view
 * that publishes the result as a dataset version. Four blocks, four engines'
 * worth of work, and nothing in it is a mock.
 *
 * The corpus is named, not endorsed: `usage` on a hub row is `unclear` unless
 * somebody read the licence at the original, and that rule does not bend for
 * an example (ADR_0019).
 */
export const PII_DETECTION_EXAMPLE = {
  title: 'PII detection',
  needsNotebooks: true,
  name: 'curation/pii-detection',
  description:
    'Read a public PII corpus, keep the English rows, mask what a shape detector finds, publish what is left.',
  blocks: [
    {
      id: 'corpus',
      title: 'Hugging Face',
      position: { x: 0, y: 0 },
      spec: {
        kind: 'source',
        dataset: 'hub_rows',
        arguments: {
          dataset: 'ai4privacy/pii-masking-200k',
          split: 'train',
          limit: '50',
        },
      },
    },
    {
      id: 'shape',
      title: 'Flow PHP',
      position: { x: 300, y: 0 },
      spec: {
        kind: 'transform',
        steps: [
          "->withEntry('text', array_get(ref('row'), 'source_text'))",
          "->withEntry('language', array_get(ref('row'), 'language'))",
          "->filter(ref('language')->same(lit('en')))",
          "->select(ref('row_index'), ref('language'), ref('text'))",
        ].join('\n'),
      },
    },
    {
      id: 'detect',
      title: 'marimo',
      position: { x: 600, y: 0 },
      spec: {
        kind: 'notebook',
        notebook: 'pii_detection',
        params: { text_column: 'text', only_matches: false },
      },
    },
    {
      id: 'result',
      title: 'View',
      position: { x: 900, y: 0 },
      spec: { kind: 'view', dataset: 'curation/pii-masked-sample' },
    },
  ],
  edges: [
    { from: 'corpus', to: 'shape' },
    { from: 'shape', to: 'detect' },
    { from: 'detect', to: 'result' },
  ],
} satisfies CurationExample;

/**
 * The Titanic corpus, shaped the way a Kaggle notebook shapes it.
 *
 * A chain across three engines, and — since the query surface stopped being a
 * hand-written list — a chain that no longer *has* to be. Flow reaches into
 * the corpus's own columns, recodes `Sex`, pulls the title out of `Name` and
 * collapses it to a status; the notebook fills a missing age from its status
 * group and sizes the family. Both halves are expressible in one query now
 * (`titanic/features` in the Recipe view does exactly that, with a window
 * function and `->plus(...)`), so what this example demonstrates is the
 * *notebook block* — its contract, its widgets and its output — rather than a
 * limit of the language it sits after.
 *
 * The recode is written as a chain of `when(…, ref('status'))` steps rather
 * than as one nested expression. Both compile to the same thing; this one fits
 * on a line, and a line is what the editor shows and what a diagnostic points
 * into.
 *
 * `phihung/titanic` is the Kaggle competition's training split, mirrored: 891
 * rows, the twelve columns everybody's notebook starts from. Named, not
 * endorsed — the mirror declares `license:other` and `usage` stays `unclear`
 * until somebody reads the licence at the original (ADR_0019).
 */
export const TITANIC_FEATURES_EXAMPLE = {
  title: 'Titanic features',
  needsNotebooks: true,
  name: 'curation/titanic-features',
  description:
    'Read the Titanic training split, recode sex and the title in the name, then let a notebook fill a missing age from its status group and size the family.',
  blocks: [
    {
      id: 'corpus',
      title: 'Hugging Face',
      position: { x: 0, y: 0 },
      spec: {
        kind: 'source',
        dataset: 'hub_rows',
        arguments: {
          dataset: 'phihung/titanic',
          split: 'train',
          // The hub rows route caps a read at 100, so this is the first
          // hundred passengers rather than the whole split.
          limit: '100',
        },
      },
    },
    {
      id: 'shape',
      title: 'Flow PHP',
      position: { x: 300, y: 0 },
      spec: {
        kind: 'transform',
        steps: [
          "->withEntry('passenger_id', array_get(ref('row'), 'PassengerId'))",
          "->withEntry('name', array_get(ref('row'), 'Name'))",
          "->withEntry('sex', array_get(ref('row'), 'Sex'))",
          "->withEntry('age', array_get(ref('row'), 'Age'))",
          "->withEntry('pclass', array_get(ref('row'), 'Pclass'))",
          "->withEntry('sib_sp', array_get(ref('row'), 'SibSp'))",
          "->withEntry('parch', array_get(ref('row'), 'Parch'))",
          "->withEntry('fare', array_get(ref('row'), 'Fare'))",
          "->withEntry('cabin', array_get(ref('row'), 'Cabin'))",
          "->withEntry('survived', array_get(ref('row'), 'Survived'))",
          "->withEntry('sex_code', when(ref('sex')->same(lit('female')), lit(1), lit(0)))",
          "->withEntry('title', regex_replace(lit('/^[^,]*,\\s*([^.]+)\\..*$/'), lit('$1'), ref('name')))",
          "->withEntry('status', lit('rare'))",
          "->withEntry('status', when(ref('title')->same(lit('Mr')), lit('mr'), ref('status')))",
          "->withEntry('status', when(ref('title')->same(lit('Master')), lit('master'), ref('status')))",
          "->withEntry('status', when(any(ref('title')->same(lit('Mrs')), ref('title')->same(lit('Mme'))), lit('mrs'), ref('status')))",
          "->withEntry('status', when(any(ref('title')->same(lit('Miss')), ref('title')->same(lit('Mlle')), ref('title')->same(lit('Ms'))), lit('miss'), ref('status')))",
          "->withEntry('outcome', when(ref('survived')->same(lit(1)), lit('survived'), lit('died')))",
          "->withEntry('deck', when(ref('cabin')->isNull(), lit('unknown'), regex_replace(lit('/^(.).*$/'), lit('$1'), ref('cabin'))))",
          "->select(ref('passenger_id'), ref('name'), ref('sex'), ref('sex_code'), ref('title'), ref('status'), ref('age'), ref('pclass'), ref('sib_sp'), ref('parch'), ref('fare'), ref('deck'), ref('survived'), ref('outcome'))",
        ].join('\n'),
      },
    },
    {
      id: 'features',
      title: 'marimo',
      position: { x: 600, y: 0 },
      spec: {
        kind: 'notebook',
        notebook: 'titanic_features',
        params: {
          age_column: 'age',
          status_column: 'status',
          siblings_column: 'sib_sp',
          parents_column: 'parch',
          fare_column: 'fare',
          age_strategy: 'status_median',
          child_max: 12,
          senior_min: 60,
        },
      },
    },
    {
      id: 'result',
      title: 'View',
      position: { x: 900, y: 0 },
      spec: { kind: 'view', dataset: 'curation/titanic-features' },
    },
  ],
  edges: [
    { from: 'corpus', to: 'shape' },
    { from: 'shape', to: 'features' },
    { from: 'features', to: 'result' },
  ],
} satisfies CurationExample;

/**
 * The same corpus, asked a question instead of reshaped.
 *
 * Three blocks and no notebook, which is the point of shipping it beside the
 * other two: a source and a transform are a perfectly good curation, and this
 * one runs with the notebook runtime switched off. The aggregation is the
 * classic first table — survival rate by status and class — and it lands as
 * one Flow query, so the two boxes light up together.
 *
 * `average` answers to two decimals, which is why nothing here rounds: a
 * second rounding would be a rule this file invented about somebody else's
 * number. `median` is not Flow's at all — it is one of the four distribution
 * aggregations `services/flow` adds (ADR_0008's amendment), and it is here
 * beside the rate because "who survived" and "how old were they" are the same
 * question asked twice.
 */
export const TITANIC_SURVIVAL_EXAMPLE = {
  title: 'Titanic survival rates',
  needsNotebooks: false,
  name: 'curation/titanic-survival',
  description:
    'Survival rate, headcount and median age by status and class over the Titanic training split, as one Flow PHP query.',
  blocks: [
    {
      id: 'corpus',
      title: 'Hugging Face',
      position: { x: 0, y: 0 },
      spec: {
        kind: 'source',
        dataset: 'hub_rows',
        arguments: { dataset: 'phihung/titanic', split: 'train', limit: '100' },
      },
    },
    {
      id: 'rates',
      title: 'Flow PHP',
      position: { x: 300, y: 0 },
      spec: {
        kind: 'transform',
        steps: [
          "->withEntry('name', array_get(ref('row'), 'Name'))",
          "->withEntry('sex', array_get(ref('row'), 'Sex'))",
          "->withEntry('pclass', array_get(ref('row'), 'Pclass'))",
          "->withEntry('age', array_get(ref('row'), 'Age'))",
          "->withEntry('survived', array_get(ref('row'), 'Survived'))",
          "->withEntry('title', regex_replace(lit('/^[^,]*,\\s*([^.]+)\\..*$/'), lit('$1'), ref('name')))",
          "->withEntry('status', lit('rare'))",
          "->withEntry('status', when(ref('title')->same(lit('Mr')), lit('mr'), ref('status')))",
          "->withEntry('status', when(ref('title')->same(lit('Master')), lit('master'), ref('status')))",
          "->withEntry('status', when(any(ref('title')->same(lit('Mrs')), ref('title')->same(lit('Mme'))), lit('mrs'), ref('status')))",
          "->withEntry('status', when(any(ref('title')->same(lit('Miss')), ref('title')->same(lit('Mlle')), ref('title')->same(lit('Ms'))), lit('miss'), ref('status')))",
          "->groupBy(ref('status'), ref('sex'), ref('pclass'))",
          "->aggregate(count(ref('survived')->as('passengers')), average(ref('survived')->as('survival_rate')), median(ref('age')))",
          "->sortBy(ref('survival_rate')->desc())",
        ].join('\n'),
      },
    },
    {
      id: 'result',
      title: 'View',
      position: { x: 600, y: 0 },
      spec: { kind: 'view', dataset: 'curation/titanic-survival' },
    },
  ],
  edges: [
    { from: 'corpus', to: 'rates' },
    { from: 'rates', to: 'result' },
  ],
} satisfies CurationExample;

/**
 * What the Load menu offers, in the order it offers them.
 *
 * The notebook-free one is last rather than hidden when the runtime is down:
 * an example that disappears is an example nobody learns exists, and the
 * button says which ones need that service.
 */
export const CURATION_EXAMPLES: CurationExample[] = [
  PII_DETECTION_EXAMPLE,
  TITANIC_FEATURES_EXAMPLE,
  TITANIC_SURVIVAL_EXAMPLE,
];

/**
 * Whether the canvas on screen is the canvas a run compiled.
 *
 * Three answers, and the third is the one worth having a name for.
 * `undefined` means there is no managed run to compare against — the ad-hoc
 * path is what drives the boxes then. `false` means somebody has edited the
 * draft since, which is the ordinary state of working and is *not* a warning
 * about something being wrong; what it forbids is lighting a block with an
 * outcome that belongs to a different drawing.
 *
 * `atRevision` is `undefined` for an edited draft, because a revision is a
 * content address the server computes over the whole authored request and the
 * browser cannot work out what this draft would be saved as.
 */
export function followsTheRun(
  atRevision: string | undefined,
  runRevision: string | undefined | null,
): boolean | undefined {
  if (runRevision === undefined || runRevision === null) return undefined;
  return runRevision === atRevision;
}

/**
 * A managed run's step states, arranged over the blocks somebody drew.
 *
 * The mapping is the **server's** — `GET /executions/{id}/blocks`, from the
 * pinned plan — and never worked out here: three source and transform boxes
 * fold into one Flow query, and a browser deciding which is which would decide
 * it from the draft on screen rather than from what the run compiled
 * (section 19).
 *
 * What this does is presentation and only presentation: a state type becomes
 * one of four ways a box can look. It decides nothing about the run.
 */
export function managedOutcomes(
  steps: readonly StepState[],
  mapping: readonly StepBlocks[],
): PipelineOutcomes {
  const outcomes: PipelineOutcomes = {};
  const byStep = new Map(steps.map((step) => [step.step_id, step]));
  for (const entry of mapping) {
    const step = byStep.get(entry.step_id);
    if (!step) continue;
    const outcome = outcomeOf(step);
    for (const block of entry.blocks) outcomes[block] = outcome;
  }
  return outcomes;
}

function outcomeOf(step: StepState): BlockOutcome {
  const attempt = step.current_attempt > 1 ? ` · attempt ${step.current_attempt}` : '';
  const named = step.state.name || step.state.state_type;
  switch (step.state.state_type) {
    case 'running':
      return { status: 'running', note: `running…${attempt}` };
    case 'completed':
      return { status: 'done', note: `completed${attempt}` };
    case 'failed':
    case 'crashed':
    case 'cancelled':
      return { status: 'failed', message: `${named}${attempt}` };
    case 'awaiting_input':
      // Not `running`: the pulse says the server is working on it, and it is
      // waiting for a person. The card below is where the question is answered.
      return { status: 'idle', note: 'waiting for an answer' };
    case 'paused':
      return { status: 'idle', note: 'paused' };
    // `scheduled` and `pending` are the reason a declaration exists: a step
    // nothing has started yet, drawn as one.
    default:
      return { status: 'idle', note: named };
  }
}
