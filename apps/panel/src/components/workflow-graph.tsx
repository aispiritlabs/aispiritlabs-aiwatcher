import * as React from 'react';
import {
  Background,
  BackgroundVariant,
  Controls,
  Handle,
  Position,
  ReactFlow,
  type Edge,
  type Node,
  type NodeProps,
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import { AlertCircle, Bot, CircleDashed, FileBox, Repeat } from 'lucide-react';

import type { AgentMessage, NodeState, WorkflowEdge } from '@/api/generated/types.gen';
import { graphWidth, layoutAgents, layoutGraph } from '@/lib/workflow-layout';
import { cn, formatDuration } from '@/lib/utils';

/**
 * A workflow execution, as a graph.
 *
 * Two kinds of thing are drawn and they are kept visually apart, because
 * conflating them would make the picture claim something nothing recorded:
 *
 * * **Stages and their declared edges** — the shape the orchestrator said it
 *   would run. Solid, muted, always in the pipeline row.
 * * **Agents and the messages between them** — what was actually said. Dashed,
 *   coloured, on their own row above. An agent is not a stage: it can appear in
 *   several and in none, so giving it a rank in the pipeline would be a lie
 *   about the order things happen in.
 *
 * A stage nothing has started is drawn dim rather than omitted. That is the
 * whole reason the topology rides the log — see ADR_0012.
 */

const STATUS_RING: Record<string, string> = {
  pending: 'border-border/60 bg-card/40 text-muted-foreground',
  running: 'border-running bg-running/10 text-foreground',
  succeeded: 'border-success/60 bg-card text-foreground',
  failed: 'border-danger bg-danger/10 text-foreground',
};

const STATUS_DOT: Record<string, string> = {
  pending: 'bg-muted-foreground/40',
  running: 'bg-running animate-pulse',
  succeeded: 'bg-success',
  failed: 'bg-danger',
};

type StageData = {
  node: NodeState;
  selected: boolean;
};

type AgentData = {
  agent: string;
  active: boolean;
};

function StageNode({ data }: NodeProps<Node<StageData, 'stage'>>) {
  const { node, selected } = data;
  return (
    <div
      className={cn(
        'flex w-[13rem] flex-col gap-1.5 rounded-lg border-2 px-3 py-2 shadow-sm transition-colors',
        STATUS_RING[node.status] ?? STATUS_RING.pending,
        selected && 'ring-2 ring-primary ring-offset-2 ring-offset-background',
        // A node nothing declared is drawn dashed: the graph has drifted from
        // the code running it, and that is worth seeing rather than hiding.
        !node.declared && 'border-dashed',
      )}
    >
      <Handle
        type="target"
        position={Position.Left}
        className="!h-2 !w-2 !border-border !bg-muted"
      />
      <div className="flex items-center gap-1.5">
        <span className={cn('h-2 w-2 shrink-0 rounded-full', STATUS_DOT[node.status])} />
        <span className="truncate text-sm font-medium">{node.name}</span>
      </div>
      <div className="flex items-center gap-2 text-[0.7rem] text-muted-foreground">
        {node.kind ? <span className="truncate">{node.kind}</span> : null}
        {node.status === 'pending' ? (
          <span className="flex items-center gap-1">
            <CircleDashed className="h-3 w-3" /> not run
          </span>
        ) : (
          <span className="tabular-nums">{formatDuration(node.duration_ms)}</span>
        )}
      </div>
      <div className="flex items-center gap-2.5 text-[0.7rem] text-muted-foreground">
        {node.artifacts.length > 0 ? (
          <span className="flex items-center gap-1" title="artifacts produced">
            <FileBox className="h-3 w-3" />
            {node.artifacts.length}
          </span>
        ) : null}
        {node.attempts > 1 ? (
          <span className="flex items-center gap-1 text-warning" title="attempts">
            <Repeat className="h-3 w-3" />
            {node.attempts}
          </span>
        ) : null}
        {node.agents.length > 0 ? (
          <span className="flex items-center gap-1 truncate" title={node.agents.join(', ')}>
            <Bot className="h-3 w-3" />
            {node.agents[0]}
            {node.agents.length > 1 ? ` +${node.agents.length - 1}` : ''}
          </span>
        ) : null}
        {node.error ? <AlertCircle className="h-3 w-3 shrink-0 text-danger" /> : null}
      </div>
      <Handle
        type="source"
        position={Position.Right}
        className="!h-2 !w-2 !border-border !bg-muted"
      />
    </div>
  );
}

function AgentNode({ data }: NodeProps<Node<AgentData, 'agent'>>) {
  return (
    <div
      className={cn(
        'flex w-[13rem] items-center gap-2 rounded-full border px-3 py-1.5 text-sm',
        data.active
          ? 'border-primary/60 bg-primary/10 text-foreground'
          : 'border-border bg-card text-muted-foreground',
      )}
    >
      {/* Two pairs of handles, because agents reply. A conversation drawn
          left-to-right in both directions is one line, and one line cannot
          say that both of them spoke — which is the entire question this view
          is open to answer. The return leg routes over the top instead. */}
      <Handle
        id="in"
        type="target"
        position={Position.Left}
        className="!h-2 !w-2 !border-border !bg-muted"
      />
      <Handle
        id="in-back"
        type="target"
        position={Position.Top}
        className="!h-2 !w-2 !border-border !bg-muted"
      />
      <Bot className="h-3.5 w-3.5 shrink-0" />
      <span className="truncate">{data.agent}</span>
      <Handle
        id="out"
        type="source"
        position={Position.Right}
        className="!h-2 !w-2 !border-border !bg-muted"
      />
      <Handle
        id="out-back"
        type="source"
        position={Position.Top}
        className="!h-2 !w-2 !border-border !bg-muted"
      />
    </div>
  );
}

const NODE_TYPES = { stage: StageNode, agent: AgentNode };

export function WorkflowGraph({
  nodes,
  edges,
  messages,
  agents,
  selectedNode,
  onSelectNode,
}: {
  nodes: NodeState[];
  edges: WorkflowEdge[];
  messages: AgentMessage[];
  agents: string[];
  selectedNode?: string | undefined;
  onSelectNode: (nodeId: string | undefined) => void;
}) {
  // Keyed on the graph's *shape*, not on its contents. A status change must
  // not move a box; only a node appearing or disappearing may.
  const nodesRef = React.useRef(nodes);
  const edgesRef = React.useRef(edges);
  nodesRef.current = nodes;
  edgesRef.current = edges;
  const shape = React.useMemo(() => nodes.map((node) => node.node_id).join('|'), [nodes]);
  const edgeShape = React.useMemo(
    () => edges.map((edge) => `${edge.from}>${edge.to}`).join('|'),
    [edges],
  );
  // `nodes` and `edges` are intentionally absent from the dependency list:
  // depending on them would recompute — and therefore re-place — the graph on
  // every status change, which is the reshuffle this keying exists to prevent.
  const positions = React.useMemo(
    () => new Map(layoutGraph(nodesRef.current, edgesRef.current).map((n) => [n.id, n.position])),
    [shape, edgeShape],
  );

  const messagingAgents = React.useMemo(() => {
    const seen: string[] = [];
    for (const message of messages) {
      for (const end of [message.from, message.to]) {
        if (!seen.includes(end)) seen.push(end);
      }
    }
    return seen;
  }, [messages]);

  const flowNodes = React.useMemo<Node[]>(() => {
    const width = graphWidth([...positions].map(([id, position]) => ({ id, position })));
    const stages: Node[] = nodes.map((node) => ({
      id: node.node_id,
      type: 'stage',
      position: positions.get(node.node_id) ?? { x: 0, y: 0 },
      data: { node, selected: node.node_id === selectedNode } satisfies StageData,
      draggable: false,
    }));
    const agentRow: Node[] = layoutAgents(messagingAgents, width).map((placed) => {
      const agent = placed.id.slice('agent:'.length);
      return {
        id: placed.id,
        type: 'agent',
        position: placed.position,
        data: { agent, active: agents.includes(agent) } satisfies AgentData,
        draggable: false,
      };
    });
    return [...agentRow, ...stages];
  }, [nodes, positions, selectedNode, messagingAgents, agents]);

  const flowEdges = React.useMemo<Edge[]>(() => {
    const known = new Set(nodes.map((node) => node.node_id));
    const declared: Edge[] = edges
      .filter((edge) => known.has(edge.from) && known.has(edge.to))
      .map((edge) => ({
        id: `declared:${edge.from}:${edge.to}`,
        source: edge.from,
        target: edge.to,
        label: edge.label ?? undefined,
        style: { stroke: 'var(--color-border)', strokeWidth: 1.5 },
        // Animated only while the downstream stage is actually running, so
        // motion on this canvas always means something is happening.
        animated: nodes.some((node) => node.node_id === edge.to && node.status === 'running'),
      }));

    // One edge per distinct pair, however many messages went over it: a
    // hundred parallel lines between two agents is not a hundred facts.
    const pairs = new Map<string, { from: string; to: string; count: number }>();
    for (const message of messages) {
      // A separator no agent name can contain, so a pair is never confused
      // with an agent whose own name happens to look like one.
      const key = `${message.from}\u0000${message.to}`;
      const seen = pairs.get(key);
      pairs.set(key, { from: message.from, to: message.to, count: (seen?.count ?? 0) + 1 });
    }
    const observed: Edge[] = [...pairs.values()].map(({ from, to, count }) => {
      // The agent row runs left to right in first-seen order, so a message
      // travelling right-to-left is a reply. Routing it over the top is what
      // keeps a two-way conversation from collapsing into one line.
      const replying = messagingAgents.indexOf(from) > messagingAgents.indexOf(to);
      return {
        id: `message:${from}:${to}`,
        source: `agent:${from}`,
        target: `agent:${to}`,
        sourceHandle: replying ? 'out-back' : 'out',
        targetHandle: replying ? 'in-back' : 'in',
        label: count > 1 ? `${count} messages` : 'message',
        labelStyle: { fill: 'var(--color-muted-foreground)', fontSize: 10 },
        labelBgStyle: { fill: 'var(--color-card)' },
        // Dashed and coloured, never solid: a declared edge is a shape somebody
        // promised, an observed one is something that was said. The picture
        // must not blur the two.
        style: { stroke: 'var(--color-primary)', strokeWidth: 1.5, strokeDasharray: '4 3' },
        animated: true,
      };
    });

    return [...declared, ...observed];
  }, [nodes, edges, messages, messagingAgents]);

  if (nodes.length === 0) {
    return (
      <div className="flex h-full min-h-[24rem] items-center justify-center px-8 text-center">
        <p className="max-w-md text-sm text-muted-foreground">
          This execution has no nodes yet. A producer draws the graph by publishing{' '}
          <code className="id">workflow.declared</code>; without one, the shape appears a stage at a
          time as <code className="id">step.*</code> events arrive.
        </p>
      </div>
    );
  }

  return (
    <div className="h-[32rem] w-full">
      <ReactFlow
        nodes={flowNodes}
        edges={flowEdges}
        nodeTypes={NODE_TYPES}
        onNodeClick={(_, node) => {
          if (node.type !== 'stage') return;
          onSelectNode(node.id === selectedNode ? undefined : node.id);
        }}
        onPaneClick={() => onSelectNode(undefined)}
        fitView
        fitViewOptions={{ padding: 0.15, minZoom: 0.35, maxZoom: 1.1 }}
        nodesDraggable={false}
        nodesConnectable={false}
        // React Flow ships light defaults; this console is dark-only, so the
        // furniture is mapped onto the panel's own tokens rather than themed
        // with a stylesheet override that would drift from `styles.css`.
        colorMode="dark"
        style={{ background: 'transparent' }}
      >
        <Background
          variant={BackgroundVariant.Dots}
          gap={18}
          size={1}
          color="var(--color-gridline)"
        />
        <Controls
          showInteractive={false}
          className="!border !border-border !bg-card [&_button]:!border-border [&_button]:!bg-card [&_button]:!fill-muted-foreground hover:[&_button]:!bg-accent"
        />
      </ReactFlow>
    </div>
  );
}
