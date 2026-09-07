import * as React from 'react';
import { Link } from '@tanstack/react-router';
import { useQuery, useQueryClient } from '@tanstack/react-query';

import { getExecution } from '@/api/generated/sdk.gen';
import type { RunProjection, StateType } from '@/api/generated/types.gen';
import { openWorkflowStream } from '@/lib/live';

import { Badge, Card, IdChip, Spinner } from './ui/primitives';

/**
 * A run the server owns, followed rather than driven.
 *
 * The stream is the **signal** and the run's own page is the **truth**. A frame
 * says something happened; the store's projection — written in the same
 * transaction as the decision that caused it — says what the state now is. That
 * split is why nothing here parses a frame's payload: the facts on the log are
 * what the workflow waterfall draws, and what this page needs is the per-step
 * state, which only the projection has.
 *
 * There is no `/executions/{id}/stream`. ADR_0026 put every fact of a managed
 * run on the log carrying the execution as its `workflow_run_id`, so the
 * workflow stream already follows one — a second route would be a second image
 * of one run.
 */
export function useManagedRun(executionId: string | undefined) {
  const queryClient = useQueryClient();
  const key = ['execution', executionId] as const;

  const run = useQuery({
    queryKey: key,
    enabled: Boolean(executionId),
    queryFn: async () => {
      const response = await getExecution({ path: { execution_id: executionId ?? '' } });
      if (!response.data) throw response.error ?? new Error('That run could not be read.');
      return response.data;
    },
  });

  React.useEffect(() => {
    if (!executionId) return undefined;
    return openWorkflowStream(executionId, undefined, {
      onEvent: () => void queryClient.invalidateQueries({ queryKey: ['execution', executionId] }),
      // The phase says whether the stream caught up or went live, which matters
      // to a waterfall and not to a state that is re-read either way.
      onPhase: () => {},
    });
  }, [executionId, queryClient]);

  return run;
}

/** What a state is called, and how loudly. */
function tone(state: StateType) {
  switch (state) {
    case 'completed':
      return 'success' as const;
    case 'failed':
    case 'crashed':
    case 'cancelled':
      return 'danger' as const;
    case 'running':
      return 'running' as const;
    case 'paused':
    case 'awaiting_input':
      return 'warning' as const;
    default:
      return 'neutral' as const;
  }
}

/**
 * The run, its steps, and where to watch it properly.
 *
 * Deliberately not a waterfall: that view exists, it is fed by the same events,
 * and drawing a second one here would be two pictures of one run that can
 * disagree while one of them is a frame behind.
 */
export function ManagedRunCard({ run, pending }: { run?: RunProjection; pending: boolean }) {
  if (!run) {
    return (
      <Card className="p-3 text-sm">
        {pending ? (
          <span className="flex items-center gap-2 text-muted-foreground">
            <Spinner /> Starting a run on the server…
          </span>
        ) : null}
      </Card>
    );
  }

  return (
    <Card className="flex flex-col gap-2 p-3 text-sm">
      <div className="flex flex-wrap items-center gap-2">
        <Badge tone={tone(run.state.state_type)}>{run.state.name || run.state.state_type}</Badge>
        <span className="text-muted-foreground">running on the server as</span>
        <IdChip label="execution" value={run.execution_id.slice(0, 12)} full={run.execution_id} />
        <Link
          to="/workflows"
          search={{ workflow: run.definition_name, execution: run.execution_id }}
          className="ml-auto text-xs text-primary hover:underline"
        >
          Watch the waterfall →
        </Link>
      </div>

      <ul className="flex flex-col gap-1">
        {run.steps.map((step) => (
          <li key={step.step_id} className="flex items-center gap-2 text-xs">
            <Badge tone={tone(step.state.state_type)}>{step.state.state_type}</Badge>
            <span className="font-medium">{step.step_id}</span>
            <span className="text-muted-foreground">{step.runtime}</span>
            {step.current_attempt > 1 ? (
              <span className="text-muted-foreground">attempt {step.current_attempt}</span>
            ) : null}
            {step.state.name ? (
              <span className="min-w-0 truncate text-muted-foreground">{step.state.name}</span>
            ) : null}
          </li>
        ))}
      </ul>

      <p className="text-xs text-muted-foreground">
        This run belongs to the server. Closing the tab does not stop it, and reopening this page
        picks it back up.
      </p>
    </Card>
  );
}
