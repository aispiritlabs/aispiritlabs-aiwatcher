import { z } from 'zod';

/**
 * A curation, assembled out of blocks and run one engine at a time.
 *
 * Four things happen on this page and they belong to three different systems.
 * The source and the transforms are one Flow PHP query; a notebook block is a
 * marimo notebook the `ml_pipeline` service runs over the rows that query
 * produced; the view publishes an immutable dataset version through the Rust
 * registry. The chain is driven from the browser because that is the only place
 * that can see all three — see ADR_0024.
 *
 * Both notebook-running services are optional and the page says which one is
 * missing rather than failing: a chain of a source and a transform is a
 * perfectly good curation, and it runs with the notebook runtime switched off.
 *
 * There is a second way to run one, and it is the opposite arrangement: **Run
 * on the server** hands the saved revision to `POST /api/v1/executions` and
 * the browser stops being part of it (ADR_0025). The ad-hoc path above stays,
 * because it is what an editor needs — a preview, a block at a time, an answer
 * in the tab you are already looking at. What it is not is a thing to leave
 * running.
 */

export const searchSchema = z.object({
  name: z.string().optional(),
  block: z.string().optional(),
  // What the canvas is tracing from the selected block, if anything. In the
  // URL with the rest of the selection: a link to a traced view is the whole
  // reason to trace one, and it is meaningless without the `block` beside it.
  reach: z.enum(['upstream', 'downstream', 'both']).optional(),
  // The stretch of the chain being isolated. Derived from the blocks, so it
  // is never saved — but it is in the URL, because "look at the ML half of
  // this" is a thing somebody sends to somebody else.
  phase: z.enum(['ingest', 'engineering', 'ml', 'gate', 'output']).optional(),
  // Phases drawn as one box. A comma-joined list, in the URL with the rest of
  // the view: "here it is with the six transform steps folded away" is the
  // form of this canvas most worth sending to somebody.
  fold: z.string().optional(),
  view: z.enum(['canvas', 'notebook']).optional(),
  window: z.number().int().nonnegative().optional(),
  // The managed run this page is following. In the URL rather than in state
  // for the usual reason and one that is load-bearing here: ADR_0025's whole
  // claim is that the browser may close, and a run held in `useState` is a run
  // a reload loses. The run itself survives either way — it is on the log,
  // under this id, in the Workflows view — but the page could not be pointed
  // back at it.
  execution: z.string().optional(),
});
