import * as React from 'react';
import { Link, createFileRoute } from '@tanstack/react-router';
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  AlertCircle,
  ExternalLink,
  FileBox,
  MessagesSquare,
  RefreshCw,
  Search,
} from 'lucide-react';
import { z } from 'zod';

import {
  getWorkflowExecution,
  listWorkflowExecutions,
  listWorkflows,
} from '@/api/generated/sdk.gen';
import type { ExecutionSummary, NodeState, WorkflowDefinition } from '@/api/generated/types.gen';
import { rerunWorkflow } from '@/api/generated/sdk.gen';
import { StatusBadge, StreamBadge } from '@/components/status-badge';
import { VirtualList } from '@/components/virtual-list';
import { WorkflowGraph } from '@/components/workflow-graph';
import {
  Badge,
  Button,
  Card,
  CardContent,
  EmptyState,
  IdChip,
  Spinner,
  Stat,
} from '@/components/ui/primitives';
import { openWorkflowStream, type LiveEventFrame, type StreamPhase } from '@/lib/live';
import { cn, formatCount, formatDuration, formatTime, pinchId, shortId } from '@/lib/utils';

/**
 * An artifact's size, in the units a person reading a path thinks in.
 *
 * `formatCount` abbreviates for cardinalities — "184.3k" reads as a count of
 * things, not as a size, which is the wrong thing to say about a file.
 */
function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['kB', 'MB', 'GB'];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`;
}

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

const searchSchema = z.object({
  workflow: z.string().optional(),
  execution: z.string().optional(),
  node: z.string().optional(),
  find: z.string().optional(),
});

export const Route = createFileRoute('/workflows')({
  validateSearch: searchSchema,
  component: WorkflowsPage,
});

type Selection = z.infer<typeof searchSchema>;

const PAGE = 40;

function WorkflowsPage() {
  const search = Route.useSearch();
  const navigate = Route.useNavigate();

  const merge = React.useCallback(
    (next: Partial<Selection>) => {
      void navigate({ search: (previous) => ({ ...previous, ...next }) });
    },
    [navigate],
  );

  const find = search.find ?? '';
  const [draft, setDraft] = React.useState(find);

  // The URL is the state; the input is a draft of it. Committing on a debounce
  // rather than on every keystroke keeps the history from filling with
  // half-typed words and the server from answering seven queries for one.
  React.useEffect(() => setDraft(find), [find]);
  React.useEffect(() => {
    if (draft === find) return;
    const timer = setTimeout(() => merge({ find: draft || undefined }), 250);
    return () => clearTimeout(timer);
  }, [draft, find, merge]);

  const workflows = useInfiniteQuery({
    queryKey: ['workflows', find],
    initialPageParam: undefined as string | undefined,
    queryFn: async ({ pageParam }) => {
      const response = await listWorkflows({
        query: { search: find || undefined, after: pageParam, limit: PAGE },
      });
      if (!response.data) throw new Error('failed to list workflows');
      return response.data;
    },
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    refetchInterval: 15_000,
  });

  const rows = React.useMemo(
    () => (workflows.data?.pages ?? []).flatMap((page) => page.workflows),
    [workflows.data],
  );

  // Nothing selected means the newest workflow, so the page is useful on
  // arrival rather than showing three empty panes.
  const selectedWorkflow = search.workflow ?? rows[0]?.workflow_id;

  const executions = useInfiniteQuery({
    queryKey: ['workflow-executions', selectedWorkflow],
    initialPageParam: undefined as string | undefined,
    queryFn: async ({ pageParam }) => {
      const response = await listWorkflowExecutions({
        query: { workflow_id: selectedWorkflow, after: pageParam, limit: PAGE },
      });
      if (!response.data) throw new Error('failed to list executions');
      return response.data;
    },
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    enabled: Boolean(selectedWorkflow),
    refetchInterval: 5_000,
  });

  const executionRows = React.useMemo(
    () => (executions.data?.pages ?? []).flatMap((page) => page.executions),
    [executions.data],
  );
  const selectedExecution = search.execution ?? executionRows[0]?.workflow_run_id;

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold">Workflows</h1>
          <p className="text-sm text-muted-foreground">
            The graph an orchestration declared, and one traversal of it as it happens. Folded from
            the same log as everything else — aiwatcher never calls the orchestrator to draw this.
          </p>
        </div>
      </div>

      <div className="grid gap-4 xl:grid-cols-[minmax(16rem,20rem)_1fr]">
        <div className="flex flex-col gap-4">
          <Card className="overflow-hidden">
            <div className="flex items-center gap-2 border-b border-border p-2">
              <Search className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
              <input
                value={draft}
                onChange={(event) => setDraft(event.target.value)}
                placeholder="Filter workflows…"
                className="w-full bg-transparent text-sm outline-none placeholder:text-muted-foreground"
              />
              {workflows.isFetching ? <Spinner className="shrink-0 text-muted-foreground" /> : null}
            </div>
            {workflows.isError ? (
              <p className="p-4 text-sm text-muted-foreground">
                Could not reach the API. Is the aiwatcher server running?
              </p>
            ) : rows.length === 0 && !workflows.isLoading ? (
              <p className="p-4 text-sm text-muted-foreground">
                No workflows yet. A run joins one by carrying{' '}
                <code className="id">workflow_id</code>; publishing{' '}
                <code className="id">workflow.declared</code> gives it a shape.
              </p>
            ) : (
              <VirtualList
                items={rows}
                className="max-h-[20rem]"
                estimateSize={58}
                keyOf={(workflow) => workflow.workflow_id}
                onReachEnd={() => {
                  if (workflows.hasNextPage && !workflows.isFetchingNextPage) {
                    void workflows.fetchNextPage();
                  }
                }}
                isFetchingMore={workflows.isFetchingNextPage}
                renderRow={(workflow) => (
                  <WorkflowRow
                    workflow={workflow}
                    selected={workflow.workflow_id === selectedWorkflow}
                    onSelect={() =>
                      // An explicit target, not a partial merge: picking a
                      // different workflow must drop the execution and node
                      // selected inside the previous one.
                      void navigate({
                        search: { workflow: workflow.workflow_id, find: find || undefined },
                      })
                    }
                  />
                )}
              />
            )}
          </Card>

          <Card className="overflow-hidden">
            <div className="border-b border-border px-3 py-2 text-xs uppercase tracking-wide text-muted-foreground">
              Executions
            </div>
            {!selectedWorkflow ? (
              <p className="p-4 text-sm text-muted-foreground">Pick a workflow.</p>
            ) : executionRows.length === 0 && !executions.isLoading ? (
              <p className="p-4 text-sm text-muted-foreground">
                Nothing has run this workflow inside the retention window.
              </p>
            ) : (
              <VirtualList
                items={executionRows}
                className="max-h-[24rem]"
                estimateSize={62}
                keyOf={(execution) => execution.workflow_run_id}
                onReachEnd={() => {
                  if (executions.hasNextPage && !executions.isFetchingNextPage) {
                    void executions.fetchNextPage();
                  }
                }}
                isFetchingMore={executions.isFetchingNextPage}
                renderRow={(execution) => (
                  <ExecutionRow
                    execution={execution}
                    selected={execution.workflow_run_id === selectedExecution}
                    onSelect={() =>
                      merge({ execution: execution.workflow_run_id, node: undefined })
                    }
                  />
                )}
              />
            )}
          </Card>
        </div>

        {selectedExecution ? (
          <ExecutionPane
            workflowId={selectedWorkflow}
            executionId={selectedExecution}
            selectedNode={search.node}
            onSelectNode={(node) => merge({ node })}
          />
        ) : (
          <EmptyState
            title="No execution selected"
            hint="Pick a workflow on the left. Its most recent execution opens here."
          />
        )}
      </div>
    </div>
  );
}

function WorkflowRow({
  workflow,
  selected,
  onSelect,
}: {
  workflow: WorkflowDefinition;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onSelect}
      className={cn(
        'flex w-full flex-col gap-1 border-b border-border/50 px-3 py-2 text-left transition-colors last:border-b-0 hover:bg-accent/40',
        selected && 'bg-accent/60',
      )}
    >
      <span className="truncate text-sm font-medium">{workflow.name}</span>
      <span className="flex items-center gap-2 text-xs text-muted-foreground">
        <span>{formatCount(workflow.executions)} runs</span>
        {workflow.nodes.length > 0 ? <span>{workflow.nodes.length} stages</span> : null}
        {workflow.running > 0 ? <Badge tone="running">{workflow.running} live</Badge> : null}
        {workflow.failed > 0 ? <Badge tone="danger">{workflow.failed} failed</Badge> : null}
      </span>
    </button>
  );
}

function ExecutionRow({
  execution,
  selected,
  onSelect,
}: {
  execution: ExecutionSummary;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onSelect}
      className={cn(
        'flex w-full flex-col gap-1 border-b border-border/50 px-3 py-2 text-left transition-colors last:border-b-0 hover:bg-accent/40',
        selected && 'bg-accent/60',
      )}
    >
      <span className="flex items-center justify-between gap-2">
        <span className="id truncate">{shortId(execution.workflow_run_id, 18)}</span>
        <StatusBadge status={execution.status} />
      </span>
      <span className="flex items-center gap-2 text-xs text-muted-foreground">
        <span>{formatTime(execution.started_at)}</span>
        <span className="tabular-nums">{formatDuration(execution.duration_ms)}</span>
        <span>
          {execution.nodes_succeeded}/{execution.nodes_total} stages
        </span>
      </span>
    </button>
  );
}

function ExecutionPane({
  workflowId,
  executionId,
  selectedNode,
  onSelectNode,
}: {
  workflowId: string | undefined;
  executionId: string;
  selectedNode: string | undefined;
  onSelectNode: (node: string | undefined) => void;
}) {
  const queryClient = useQueryClient();
  const [phase, setPhase] = React.useState<StreamPhase>('catching-up');
  const [resyncedFrom, setResyncedFrom] = React.useState<string | null>(null);

  const query = useQuery({
    queryKey: ['workflow-execution', executionId],
    queryFn: async () => {
      const response = await getWorkflowExecution({ path: { workflow_run_id: executionId } });
      if (!response.data) throw response.error ?? new Error(`no execution ${executionId}`);
      return response.data;
    },
  });

  const startCheckpoint = query.data?.summary.last_checkpoint;
  const isRunning = query.data?.summary.status === 'running';

  // Fetch, then open the stream at the checkpoint that fetch returned. That
  // handoff is what makes the live view seamless rather than duplicating or
  // skipping what happened in between — see `openWorkflowStream`.
  React.useEffect(() => {
    if (startCheckpoint === undefined) return undefined;

    const close = openWorkflowStream(executionId, startCheckpoint, {
      onEvent: (frame: LiveEventFrame) => {
        // The graph is a projection, not a fold this component maintains. A
        // frame is a signal to refetch it, not data to merge: reconstructing
        // node status in the browser would be a second implementation of
        // `workflows.rs` that could disagree with the first.
        if (frame.event_type.startsWith('llm.chunk')) return;
        void queryClient.invalidateQueries({ queryKey: ['workflow-execution', executionId] });
      },
      onPhase: setPhase,
      onResync: setResyncedFrom,
    });
    return close;
  }, [executionId, startCheckpoint, queryClient]);

  if (query.isError) {
    return (
      <EmptyState
        title={`No execution ${shortId(executionId, 18)}`}
        hint="It may have been evicted from the read model. Retention bounds this view."
      />
    );
  }
  if (!query.data) {
    return (
      <div className="flex items-center gap-2 p-10 text-sm text-muted-foreground">
        <Spinner /> loading the graph…
      </div>
    );
  }

  const { summary, nodes, edges, messages, messages_truncated } = query.data;
  const node = nodes.find((candidate) => candidate.node_id === selectedNode);

  return (
    <div className="flex flex-col gap-4">
      <Card>
        <CardContent className="flex flex-wrap items-center justify-between gap-3 p-4">
          <div className="flex flex-wrap items-center gap-3">
            <StatusBadge status={summary.status} />
            {isRunning ? <StreamBadge phase={phase} /> : null}
            <IdChip
              label="execution"
              value={shortId(summary.workflow_run_id, 20)}
              full={summary.workflow_run_id}
            />
            {summary.version ? (
              <IdChip label="version" value={shortId(summary.version, 12)} full={summary.version} />
            ) : null}
          </div>
          <RerunButton
            workflowId={workflowId ?? summary.workflow_id}
            executionId={summary.workflow_run_id}
            fromNode={node?.node_id}
          />
        </CardContent>
      </Card>

      {resyncedFrom ? (
        <Card className="border-warning/40 bg-warning/5">
          <CardContent className="p-3 text-xs text-warning">
            The connection was away long enough that the live buffer had scrolled past; the missed
            events were read from the durable log.
          </CardContent>
        </Card>
      ) : null}

      {summary.error ? (
        <Card className="border-danger/40 bg-danger/5">
          <CardContent className="p-3 text-sm text-danger">{summary.error}</CardContent>
        </Card>
      ) : null}

      <Card>
        <CardContent className="grid grid-cols-2 gap-6 p-4 sm:grid-cols-3 lg:grid-cols-6">
          <Stat label="Duration" value={formatDuration(summary.duration_ms)} />
          <Stat
            label="Stages"
            value={`${summary.nodes_succeeded}/${summary.nodes_total}`}
            hint={summary.nodes_pending > 0 ? `${summary.nodes_pending} never ran` : undefined}
          />
          <Stat label="Running" value={formatCount(summary.nodes_running)} />
          <Stat label="Failed" value={formatCount(summary.nodes_failed)} />
          <Stat label="Artifacts" value={formatCount(summary.artifacts)} />
          <Stat label="Runs" value={formatCount(summary.runs.length)} hint="one per stage pod" />
        </CardContent>
      </Card>

      <Card className="overflow-hidden">
        <WorkflowGraph
          nodes={nodes}
          edges={edges}
          messages={messages}
          agents={summary.agents}
          selectedNode={selectedNode}
          onSelectNode={onSelectNode}
        />
      </Card>

      {node ? <NodeInspector node={node} /> : null}

      {messages.length > 0 ? (
        <Card className="overflow-hidden">
          <div className="flex items-center gap-2 border-b border-border px-3 py-2 text-xs uppercase tracking-wide text-muted-foreground">
            <MessagesSquare className="h-3.5 w-3.5" />
            Agent messages
            {messages_truncated ? (
              <span className="normal-case">
                — older ones were shed; {formatCount(summary.messages)} were sent
              </span>
            ) : null}
          </div>
          <table className="w-full text-sm">
            <tbody>
              {messages.map((message, index) => (
                <tr
                  key={`${message.at}-${message.from}-${message.to}-${index}`}
                  className="border-b border-border/50 last:border-b-0"
                >
                  <td className="px-3 py-1.5 text-xs text-muted-foreground tabular-nums">
                    {formatTime(message.at)}
                  </td>
                  <td className="px-3 py-1.5">
                    {message.from} <span className="text-muted-foreground">to</span> {message.to}
                  </td>
                  <td className="px-3 py-1.5 text-xs text-muted-foreground">
                    {message.kind ?? '—'}
                    {message.channel ? ` · ${message.channel}` : ''}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </Card>
      ) : null}
    </div>
  );
}

function NodeInspector({ node }: { node: NodeState }) {
  return (
    <Card>
      <CardContent className="flex flex-col gap-3 p-4">
        <div className="flex flex-wrap items-center gap-3">
          <span className="text-sm font-semibold">{node.name}</span>
          <Badge
            tone={
              node.status === 'failed'
                ? 'danger'
                : node.status === 'running'
                  ? 'running'
                  : node.status === 'succeeded'
                    ? 'success'
                    : 'neutral'
            }
          >
            {node.status}
          </Badge>
          {!node.declared ? <Badge tone="warning">not in the declared graph</Badge> : null}
          {node.attempts > 1 ? <Badge tone="warning">{node.attempts} attempts</Badge> : null}
        </div>

        <div className="grid grid-cols-2 gap-6 sm:grid-cols-4">
          <Stat label="Duration" value={formatDuration(node.duration_ms)} />
          <Stat label="Kind" value={node.kind ?? '—'} />
          <Stat label="Agents" value={node.agents.length > 0 ? node.agents.join(', ') : '—'} />
          <Stat
            label="Run"
            value={
              node.run_id ? (
                <Link
                  to="/runs/$runId"
                  params={{ runId: node.run_id }}
                  className="text-sm text-primary underline"
                  title={node.run_id}
                >
                  {pinchId(node.run_id, 10, 10)}
                </Link>
              ) : (
                '—'
              )
            }
          />
        </div>

        {node.error ? (
          <p className="flex items-start gap-2 rounded-md border border-danger/40 bg-danger/5 p-2 text-sm text-danger">
            <AlertCircle className="mt-0.5 h-4 w-4 shrink-0" />
            {node.error}
          </p>
        ) : null}

        {node.artifacts.length > 0 ? (
          <div className="flex flex-col gap-1">
            <span className="flex items-center gap-1.5 text-xs uppercase tracking-wide text-muted-foreground">
              <FileBox className="h-3.5 w-3.5" /> Artifacts
            </span>
            {/* The uri, not the bytes. aiwatcher stores the pointer — see the
                artifact guardrail — so this is a reference to open elsewhere. */}
            {node.artifacts.map((artifact) => (
              <div
                key={artifact.uri}
                className="flex flex-wrap items-baseline gap-2 rounded-md border border-border px-2 py-1.5"
              >
                <span className="text-sm">{artifact.name}</span>
                <span className="id truncate text-muted-foreground">{artifact.uri}</span>
                {artifact.size_bytes ? (
                  <span className="text-xs text-muted-foreground tabular-nums">
                    {formatBytes(artifact.size_bytes)}
                  </span>
                ) : null}
                {artifact.media_type ? (
                  <span className="text-xs text-muted-foreground">{artifact.media_type}</span>
                ) : null}
              </div>
            ))}
          </div>
        ) : null}
      </CardContent>
    </Card>
  );
}

function RerunButton({
  workflowId,
  executionId,
  fromNode,
}: {
  workflowId: string;
  executionId: string;
  fromNode: string | undefined;
}) {
  const rerun = useMutation({
    mutationFn: async () => {
      const response = await rerunWorkflow({
        path: { workflow_id: workflowId },
        body: { workflow_run_id: executionId, from_node: fromNode ?? null },
      });
      if (!response.data) throw response.error ?? new Error('the rerun was refused');
      return response.data;
    },
  });

  if (isRunnerDisabled(rerun.error)) {
    return (
      <p className="max-w-sm rounded-md border border-dashed border-border px-3 py-2 text-xs text-muted-foreground">
        This instance has no workflow runner. Set{' '}
        <code className="id">AIWATCHER_WORKFLOW_RUNNER=http</code> and{' '}
        <code className="id">AIWATCHER_WORKFLOW_RUNNER_URL</code> to let aiwatcher ask your
        orchestrator to run this again.
      </p>
    );
  }

  return (
    <div className="flex items-center gap-2">
      {rerun.data ? (
        <span className="flex items-center gap-1.5 text-xs text-muted-foreground">
          queued
          {rerun.data.reference ? <code className="id">{rerun.data.reference}</code> : null}
          {rerun.data.url ? (
            <a
              href={rerun.data.url}
              target="_blank"
              rel="noreferrer"
              className="flex items-center gap-1 text-primary underline"
            >
              watch <ExternalLink className="h-3 w-3" />
            </a>
          ) : null}
        </span>
      ) : null}
      {rerun.isError && !isRunnerDisabled(rerun.error) ? (
        <span className="text-xs text-danger">
          {(rerun.error as { message?: string } | null)?.message ?? 'the rerun was refused'}
        </span>
      ) : null}
      <Button
        variant="outline"
        size="sm"
        disabled={rerun.isPending}
        onClick={() => rerun.mutate()}
        className="gap-1.5"
        title={
          fromNode
            ? `Ask the orchestrator to resume from ${fromNode}`
            : 'Ask the orchestrator to run this workflow again'
        }
      >
        {rerun.isPending ? <Spinner /> : <RefreshCw className="h-3.5 w-3.5" />}
        {fromNode ? `Rerun from ${fromNode}` : 'Rerun'}
      </Button>
    </div>
  );
}

/**
 * Whether a failure is "this deployment wired no orchestrator".
 *
 * Reads the response *body*, like `isRegistryDisabled`: the server answers 501
 * with a machine-readable `code`, so the button can become an explanation
 * rather than an error nobody can act on.
 */
export function isRunnerDisabled(error: unknown): boolean {
  const body = error as { code?: string } | null | undefined;
  return body?.code === 'runner_disabled';
}
