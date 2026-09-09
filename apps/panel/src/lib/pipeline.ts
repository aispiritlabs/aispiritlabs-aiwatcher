import type {
  BlockSpec,
  PipelineBlock,
  PipelineEdge,
  StepBlocks,
  StepState,
} from '@/api/generated/types.gen';
import { runQuery, simulateQuery, type FlowResult } from '@/lib/flow';
import { getNotebook, runNotebook, type NotebookRun } from '@/lib/ml-pipeline';
import { formatCount } from '@/lib/utils';

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
 * Pin unpinned blocks, preserving the exact code already selected by the editor.
 *
 * Editing and saving Python updates that block's revision. Saving a flow must
 * not silently replace it with a newer shared notebook head.
 */
export async function withPinnedNotebooks(blocks: PipelineBlock[]): Promise<PipelineBlock[]> {
  return Promise.all(
    blocks.map(async (block) => {
      if (block.spec.kind !== 'notebook' || block.spec.revision) return block;
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

/**
 * The one line a block says about itself, wherever it is drawn.
 *
 * Beside the type rather than in either view, because the two had drifted: the
 * canvas printed the note and the notebook read only `running` and a result, so
 * a gate — which is `idle` with the note "asked on a managed run" — was drawn
 * there as "not run", and so was a block that had failed. A box reading "not
 * run" beside three that finished is a box somebody goes looking for the
 * failure of.
 *
 * The ad-hoc path counted rows and milliseconds; a managed run reports the word
 * the server used and nothing else, because a step's timings are the log's
 * answer rather than the workflow store's. So the counts are printed when they
 * exist and the note carries the rest — never `0 rows · 0 ms`, which would be a
 * measurement nobody took. `undefined` is a block neither path has reached.
 */
export function describeOutcome(outcome: BlockOutcome | undefined): string {
  if (!outcome) return 'not run';
  if (outcome.status === 'failed') return outcome.message;
  if (outcome.status === 'running') return outcome.note ?? 'running…';
  if (outcome.status === 'idle') return outcome.note ?? 'not run';
  const measured =
    outcome.rows === undefined
      ? undefined
      : `${formatCount(outcome.rows)} rows · ${outcome.tookMs ?? 0} ms`;
  return [measured, outcome.note].filter(Boolean).join(' · ') || 'done';
}

export type PipelineOutcomes = Record<string, BlockOutcome>;

export type BlockResult = {
  rows: Row[];
  columns: string[];
  rowCount: number;
  stdout?: string;
};

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
  /** Inspect each PHP prefix without changing Flow's lazy/window semantics. */
  inspectBlocks?: boolean;
  onBlockResult?: (id: string, result: BlockResult) => void;
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
  if (!options.inspectBlocks) {
    for (const block of flowBlocks) report(block.id, { status: 'running' });
  }

  let flow: FlowResult;
  try {
    const query = mode === 'preview' ? simulateQuery : runQuery;
    if (options.inspectBlocks) {
      let latest: FlowResult | undefined;
      for (let index = 0; index < flowBlocks.length; index++) {
        const block = flowBlocks[index]!;
        report(block.id, { status: 'running' });
        try {
          latest = await query(compileFlow(flowBlocks.slice(0, index + 1)), windowSeconds);
        } catch (error) {
          report(block.id, {
            status: 'failed',
            message: error instanceof Error ? error.message : String(error),
          });
          throw error;
        }
        report(block.id, {
          status: 'done',
          rows: latest.row_count,
          columns: latest.columns,
          tookMs: latest.took_ms,
          note: 'source through this block',
        });
        options.onBlockResult?.(block.id, {
          rows: latest.rows.slice(0, 25),
          columns: latest.columns,
          rowCount: latest.row_count,
        });
      }
      if (!latest) throw new Error('A notebook flow needs a source block.');
      flow = latest;
    } else {
      flow = await query(script, windowSeconds);
    }
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (!options.inspectBlocks) {
      for (const block of flowBlocks) report(block.id, { status: 'failed', message });
    }
    throw error;
  }

  for (const block of options.inspectBlocks ? [] : flowBlocks) {
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

  // A gate is not asked here. This path is the browser driving the chain for a
  // preview or an ad-hoc run, and there is no run for anybody to hold up or
  // answer — the question belongs to a managed execution, whose card is where
  // it is put in front of somebody. Said rather than left blank, because a box
  // reading "not run" beside three that finished is a box somebody goes
  // looking for the failure of.
  for (const block of chain) {
    if (block.spec.kind !== 'approval') continue;
    report(block.id, { status: 'idle', note: 'asked on a managed run' });
  }

  let rows: Row[] = flow.rows;
  let columns = flow.columns;
  const notebooks: NotebookRun[] = [];

  for (const block of chain) {
    if (block.spec.kind !== 'notebook') continue;
    report(block.id, { status: 'running' });
    try {
      const run = await runNotebook(
        block.spec.notebook,
        rows,
        block.spec.params ?? {},
        block.spec.revision ?? undefined,
      );
      notebooks.push(run);
      rows = run.rows;
      columns = run.columns;
      options.onBlockResult?.(block.id, {
        rows: rows.slice(0, 25),
        columns,
        rowCount: run.row_count,
        stdout: run.stdout,
      });
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
    options.onBlockResult?.(block.id, { rows: rows.slice(0, 25), columns, rowCount: rows.length });
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
 * it from the draft on screen rather than from what the run compiled.
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
