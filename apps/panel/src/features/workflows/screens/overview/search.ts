import { windowSearchSchema } from '@/shared/components/time-range';
import { z } from 'zod';

/**
 * Workflows: the graph an orchestration declared, and one traversal of it live.
 *
 * The level above a run. Everything else in the observability area answers
 * "what did this run do"; this answers "where is this pipeline, and what has it
 * not reached yet" — a question a runs list cannot express, because a
 * stage-per-pod orchestrator publishes each stage from a different run.
 *
 * Nothing here knows which orchestrator produced the events. The graph is
 * folded from the log and the rerun goes to one endpoint this deployment
 * configured, so swapping Flyte for something else changes nothing on this
 * page. See ADR_0012.
 *
 * Three panes, all selection in the URL so a link lands the reader on the same
 * view: the workflows, the executions of the selected one, and the graph.
 */

export const searchSchema = z.object({
  ...windowSearchSchema,
  workflow: z.string().optional(),
  execution: z.string().optional(),
  node: z.string().optional(),
  // What the graph is tracing from the selected node. Beside `node` in the
  // URL because it is meaningless without one, and because a traced view is
  // exactly the kind of thing somebody pastes into a channel.
  reach: z.enum(['upstream', 'downstream', 'both']).optional(),
  find: z.string().optional(),
});
