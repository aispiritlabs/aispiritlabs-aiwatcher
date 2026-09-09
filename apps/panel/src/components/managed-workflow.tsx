import * as React from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  cancelExecution,
  executionHistory,
  getExecution,
  listWorkflowDefinitions,
  pauseExecution,
  resumeExecution,
  retryStep,
  startExecution,
  stepContext,
} from '@/api/generated/sdk.gen';
import type { RecordedMessage } from '@/api/generated/types.gen';
import { useCan } from '@/lib/auth';
import { AnswerGate } from '@/components/answer-gate';
import { Button, Card, CardContent } from '@/components/ui/primitives';

/** Registration is authored state; execution lists and graphs remain log projections. */
export function WorkflowLauncher({
  onStarted,
}: {
  onStarted: (workflow: string, execution: string) => void;
}) {
  const canEdit = useCan('editor');
  const queryClient = useQueryClient();
  const [name, setName] = React.useState('');
  const [parameters, setParameters] = React.useState('{}');
  const definitions = useQuery({
    queryKey: ['workflow-definitions'],
    queryFn: async () => {
      const response = await listWorkflowDefinitions();
      if (!response.data) throw new Error('Could not load registered workflows');
      return response.data;
    },
  });
  const selected = definitions.data?.find((entry) => entry.definition.name === name);
  const launch = useMutation({
    mutationFn: async (key: string) => {
      if (!selected) throw new Error('Select a registered workflow');
      const inputs: unknown = JSON.parse(parameters);
      if (!inputs || typeof inputs !== 'object' || Array.isArray(inputs)) {
        throw new Error('Parameters must be a JSON object');
      }
      const response = await startExecution({
        headers: { 'Idempotency-Key': key },
        body: {
          target: { kind: 'workflow', name: selected.definition.name, revision: selected.revision },
          parameters: inputs as Record<string, unknown>,
        },
      });
      if (!response.data) throw new Error(response.error?.message ?? 'The run was refused');
      return response.data.execution;
    },
    onSuccess: (run) => {
      void queryClient.invalidateQueries({ queryKey: ['workflow-executions'] });
      void queryClient.invalidateQueries({ queryKey: ['workflows'] });
      onStarted(run.definition_name, run.execution_id);
    },
  });
  if (definitions.isError)
    return <p className="text-sm text-muted-foreground">Workflow registration is unavailable.</p>;
  return (
    <Card>
      <CardContent className="flex flex-col gap-3 p-4">
        <div className="font-medium">Start a workflow</div>
        <div className="flex flex-wrap items-end gap-3">
          <label className="flex flex-col gap-1 text-sm">
            Registered workflow
            <select
              className="rounded border border-border bg-background p-2"
              value={name}
              onChange={(e) => setName(e.target.value)}
            >
              <option value="">Select a workflow…</option>
              {definitions.data?.map((entry) => (
                <option key={entry.definition.name} value={entry.definition.name}>
                  {entry.definition.name} · {entry.definition.version}
                </option>
              ))}
            </select>
          </label>
          <label className="flex min-w-64 flex-1 flex-col gap-1 text-sm">
            Parameters (JSON)
            <textarea
              aria-label="Workflow parameters"
              className="rounded border border-border bg-background p-2 font-mono text-xs"
              rows={2}
              value={parameters}
              onChange={(e) => setParameters(e.target.value)}
            />
          </label>
          <Button
            disabled={!canEdit || !selected || launch.isPending}
            onClick={() => launch.mutate(crypto.randomUUID())}
          >
            {launch.isPending ? 'Starting…' : 'Start run'}
          </Button>
        </div>
        {definitions.data?.length === 0 && (
          <p className="text-sm text-muted-foreground">
            Register a workflow from its Runtime to make it available here.
          </p>
        )}
        {launch.error && (
          <p role="alert" className="text-sm text-danger">
            {launch.error.message}
          </p>
        )}
      </CardContent>
    </Card>
  );
}

export function useManagedExecution(executionId: string) {
  return useQuery({
    queryKey: ['managed-execution', executionId],
    queryFn: async () => {
      const response = await getExecution({ path: { execution_id: executionId } });
      if (response.data) return response.data;
      if (response.response?.status === 404 || response.response?.status === 501) return null;
      throw new Error(response.error?.message ?? 'Could not load execution controls');
    },
    refetchInterval: (query) => {
      const value = query.state.data;
      if (value === null) return false;
      const state = value?.execution.state.state_type;
      return state && ['completed', 'failed', 'cancelled', 'crashed'].includes(state)
        ? false
        : 2000;
    },
  });
}

/// What one stream row is called, whichever of the three kinds it is.
///
/// The third is a hosted run's: a worker's own message, whose vocabulary is its
/// own rather than one this build enumerates — so the name comes off the row
/// instead of out of a union of literals.
function messageName(message: RecordedMessage['message']): string {
  switch (message.kind) {
    case 'event':
      return message.event;
    case 'command':
      return message.command;
    case 'hosted':
      return message.message_type;
  }
}

export function ManagedExecutionControls({
  executionId,
  node,
}: {
  executionId: string;
  node?: string;
}) {
  const canEdit = useCan('editor');
  const queryClient = useQueryClient();
  const run = useManagedExecution(executionId);
  const context = useQuery({
    queryKey: ['managed-step', executionId, node],
    enabled: !!run.data && !!node,
    queryFn: async () =>
      (await stepContext({ path: { execution_id: executionId, step_id: node! } })).data ?? null,
    refetchInterval: run.data?.execution.state.state_type === 'running' ? 2000 : false,
  });
  const [showHistory, setShowHistory] = React.useState(false);
  const [after, setAfter] = React.useState(0);
  React.useEffect(() => setAfter(0), [executionId]);
  const history = useQuery({
    queryKey: ['execution-history', executionId, after],
    enabled: !!run.data && showHistory,
    queryFn: async () => {
      const response = await executionHistory({
        path: { execution_id: executionId },
        query: { after, limit: 30 },
      });
      if (!response.data) throw new Error('Could not load execution history');
      return response.data;
    },
  });
  const command = useMutation({
    mutationFn: async (action: 'pause' | 'resume' | 'cancel' | 'retry') => {
      const path = { execution_id: executionId };
      const response =
        action === 'pause'
          ? await pauseExecution({ path })
          : action === 'resume'
            ? await resumeExecution({ path })
            : action === 'cancel'
              ? await cancelExecution({ path, body: { reason: 'Cancelled from Workflows' } })
              : await retryStep({ path: { ...path, step_id: node! } });
      if (!response.data) throw new Error(response.error?.message ?? 'The command was refused');
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['managed-execution', executionId] });
      void queryClient.invalidateQueries({ queryKey: ['managed-step', executionId] });
      void queryClient.invalidateQueries({ queryKey: ['execution-history', executionId] });
    },
  });
  if (run.error) return <p role="alert">{run.error.message}</p>;
  if (!run.data) return null; // An observed external workflow has no managed commands.
  return (
    <Card>
      <CardContent className="flex flex-col gap-3 p-4">
        <div className="flex flex-wrap items-center gap-2">
          <span className="mr-auto text-sm font-medium">
            {run.data.execution.state.name || run.data.execution.state.state_type}
          </span>
          {run.data.allowed.map((action) => (
            <Button
              key={action}
              disabled={!canEdit || command.isPending}
              onClick={() => command.mutate(action)}
            >
              {action}
            </Button>
          ))}
          {context.data?.allowed.includes('retry') && (
            <Button
              disabled={!canEdit || command.isPending}
              onClick={() => command.mutate('retry')}
            >
              Retry {node}
            </Button>
          )}
          <Button onClick={() => setShowHistory(!showHistory)}>
            {showHistory ? 'Hide history' : 'Decision history'}
          </Button>
        </div>
        {command.error && (
          <p role="alert" className="text-sm text-danger">
            {command.error.message}
          </p>
        )}
        {/* A gate parks the graph and the answer is the only thing that moves
            it on, so the view that watches a workflow run has to be able to
            give one — a run stopped here with no control is a run somebody has
            to go and find another screen for. The same component the pipeline's
            run card uses, because it is the same question and the same route. */}
        {node && context.data?.allowed.includes('answer') && context.data.state?.awaiting && (
          <AnswerGate
            executionId={executionId}
            stepId={node}
            attempt={context.data.state.current_attempt}
            question={context.data.state.awaiting}
            onAnswered={() => {
              void queryClient.invalidateQueries({ queryKey: ['managed-execution', executionId] });
              void queryClient.invalidateQueries({ queryKey: ['managed-step', executionId] });
            }}
          />
        )}
        {showHistory && (
          <div className="flex flex-col gap-2 text-xs">
            <p className="text-muted-foreground">
              Recorded commands and decisions, in stream order. Retries preserve earlier attempts.
            </p>
            {history.error && <p role="alert">{history.error.message}</p>}
            {history.data?.messages.map((entry) => (
              <details key={entry.stream_version} className="rounded border border-border p-2">
                <summary className="cursor-pointer">
                  {entry.stream_version} · {entry.direction} · {messageName(entry.message)} ·{' '}
                  {entry.recorded_at}
                </summary>
                <pre className="mt-2 max-h-64 overflow-auto whitespace-pre-wrap">
                  {JSON.stringify(entry.message, null, 2)}
                </pre>
              </details>
            ))}
            <div className="flex gap-2">
              <Button disabled={after === 0} onClick={() => setAfter(0)}>
                First page
              </Button>
              <Button
                disabled={!history.data?.next_after}
                onClick={() => setAfter(history.data?.next_after ?? 0)}
              >
                Next page
              </Button>
            </div>
          </div>
        )}
      </CardContent>
    </Card>
  );
}
