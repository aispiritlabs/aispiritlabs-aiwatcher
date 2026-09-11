import { describe, expect, it } from 'vitest';

import type { BlockSpec, PipelineBlock, StepBlocks, StepState } from '@/api/generated/types.gen';

import {
  chainEngine,
  compilePython,
  describeOutcome,
  followsTheRun,
  managedOutcomes,
} from '@/features/data-curation/lib/pipeline';
import { writtenElsewhere } from '@/shared/lib/query';

const block = (id: string, spec: BlockSpec): PipelineBlock => ({
  id,
  title: id,
  position: { x: 0, y: 0 },
  spec,
});

describe('the Python a chain compiles to (AW-3)', () => {
  it('is, byte for byte, the text the Rust compiler writes', () => {
    // compile.rs's a_datafusion_chain_compiles_to_one_datafusion_step_whose_script_binds_df:
    // the preview has to be the text the server runs, or it previews something else.
    const chain = [
      block('read', {
        kind: 'source',
        dataset: 'hub_rows',
        arguments: { limit: '500', dataset: 'ai4privacy/pii' },
      }),
      block('clean', {
        kind: 'transform',
        steps: 'df.filter(col("text").is_not_null())',
        engine: 'datafusion',
      }),
      block('trim', { kind: 'transform', steps: 'df.limit(100)', engine: 'datafusion' }),
    ];

    expect(compilePython(chain)).toBe(
      'df = read("hub_rows", dataset="ai4privacy/pii", limit="500")\n' +
        'df = (\ndf.filter(col("text").is_not_null())\n)\n' +
        'df = (\ndf.limit(100)\n)\n' +
        'df',
    );
    expect(chainEngine(chain)).toBe('datafusion');
  });

  it('refuses an argument name Python cannot pass, as the Rust compiler does', () => {
    const odd = block('read', {
      kind: 'source',
      dataset: 'hub_rows',
      arguments: { 'not a name': 'x' },
    });

    expect(() => compilePython([odd])).toThrow('not a name= is not an argument name');
  });

  it('gives a chain with no transform no engine of its own', () => {
    // A source is a catalog read every engine serves, so it runs where it is.
    expect(chainEngine([block('read', { kind: 'source', dataset: 'runs' })])).toBeUndefined();
    expect(chainEngine([block('t', { kind: 'transform', steps: '->limit(1)' })])).toBe('flow');
  });
});

describe('content written for another engine', () => {
  it('says which engine it needs and which one this deployment runs', () => {
    expect(writtenElsewhere('flow', 'datafusion')).toBe(
      'Written for Flow PHP, and this deployment runs DataFusion, so it is shown here and not run.',
    );
  });

  it('is nothing foreign while the engine is its own, or not known yet', () => {
    expect(writtenElsewhere('datafusion', 'datafusion')).toBeNull();
    expect(writtenElsewhere('flow', undefined)).toBeNull();
  });
});

function step(
  step_id: string,
  state_type: StepState['state']['state_type'],
  attempt = 1,
): StepState {
  return { step_id, runtime: 'flow_php', current_attempt: attempt, state: { state_type } };
}

/** The mapping the server sends: one Flow query covering two authored boxes. */
const MAPPING: StepBlocks[] = [
  { step_id: 'read', runtime: 'flow_php', blocks: ['read', 'clean'] },
  { step_id: 'detect', runtime: 'marimo', blocks: ['detect'] },
  { step_id: 'write', runtime: 'publish_dataset', blocks: ['write'] },
];

describe('drawing a managed run over the canvas', () => {
  it('lights every block one folded step covers', () => {
    // The reason the mapping is the server's: a source and its transforms are
    // one Flow query, and they start and finish together.
    const outcomes = managedOutcomes([step('read', 'running')], MAPPING);

    expect(outcomes.read).toEqual({ status: 'running', note: 'running…' });
    expect(outcomes.clean).toEqual({ status: 'running', note: 'running…' });
    // And says nothing about a step the run has not reported.
    expect(outcomes.detect).toBeUndefined();
  });

  it('draws a step nothing has started as one, rather than as nothing at all', () => {
    // `Pending` is the whole reason a declaration exists (ADR_0012): a box that
    // is waiting is different from a box nobody drew.
    const outcomes = managedOutcomes([step('detect', 'pending')], MAPPING);

    expect(outcomes.detect).toEqual({ status: 'idle', note: 'pending' });
  });

  it('never claims a row count the projection does not hold', () => {
    // A step's timings and counts are the log's answer, not the workflow
    // store's — so a completed managed step says "completed" and not
    // "0 rows · 0 ms", which would be a measurement nobody took.
    const outcomes = managedOutcomes([step('write', 'completed')], MAPPING);

    expect(outcomes.write).toEqual({ status: 'done', note: 'completed' });
  });

  it('carries the attempt when there has been more than one', () => {
    const outcomes = managedOutcomes([step('detect', 'failed', 3)], MAPPING);

    expect(outcomes.detect).toEqual({ status: 'failed', message: 'failed · attempt 3' });
  });

  it('shows a step waiting for a person as waiting rather than as working', () => {
    const outcomes = managedOutcomes([step('detect', 'awaiting_input')], MAPPING);

    expect(outcomes.detect).toEqual({ status: 'idle', note: 'waiting for an answer' });
  });
});

describe('deciding whether the canvas may draw a run at all', () => {
  it('follows the run while the draft is still what it compiled', () => {
    expect(followsTheRun('abc', 'abc')).toBe(true);
  });

  it('stops following once the draft has been edited', () => {
    // An edited draft has no revision — the browser cannot work out what it
    // would be saved as — so this is how drift arrives.
    expect(followsTheRun(undefined, 'abc')).toBe(false);
    expect(followsTheRun('def', 'abc')).toBe(false);
  });

  it('has no opinion when there is no managed run to compare against', () => {
    // Distinct from `false`: the ad-hoc path drives the boxes here, and a
    // "this canvas is not what the run compiled" line would be about a run
    // that does not exist.
    expect(followsTheRun('abc', undefined)).toBeUndefined();
    expect(followsTheRun('abc', null)).toBeUndefined();
  });
});

describe('what a block says about itself', () => {
  it('says a gate is waiting rather than that it never ran', () => {
    // The two views had drifted here. The canvas printed the note; the notebook
    // read only `running` and a result, so a gate — `idle` with a note — was
    // drawn as "not run" beside blocks that had finished, which is a box
    // somebody goes looking for the failure of.
    expect(describeOutcome({ status: 'idle', note: 'asked on a managed run' })).toBe(
      'asked on a managed run',
    );
  });

  it('says a failure failed', () => {
    expect(describeOutcome({ status: 'failed', message: 'no such dataset' })).toBe(
      'no such dataset',
    );
  });

  it('prints counts only where somebody took them', () => {
    // A managed run reports the word the server used and no timings, because a
    // step's duration is the log's answer. `0 rows · 0 ms` would be a
    // measurement nobody took.
    expect(describeOutcome({ status: 'done', note: 'run as one Flow query' })).toBe(
      'run as one Flow query',
    );
    expect(describeOutcome({ status: 'done', rows: 12, tookMs: 34 })).toBe('12 rows · 34 ms');
  });

  it('calls a block neither path reached not run', () => {
    expect(describeOutcome(undefined)).toBe('not run');
  });
});
