import type { BlockSpec, PipelineBlock, PipelineEdge } from '@/api/generated/types.gen';
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

export type BlockOutcome =
  | { status: 'idle' }
  | { status: 'running' }
  | { status: 'done'; rows: number; columns: string[]; tookMs: number; note?: string }
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
 * The example the area ships with, end to end.
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
} satisfies { name: string; description: string; blocks: PipelineBlock[]; edges: PipelineEdge[] };
