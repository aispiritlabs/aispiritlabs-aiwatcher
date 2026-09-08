import * as React from 'react';
import {
  Background,
  BackgroundVariant,
  Controls,
  Handle,
  Position,
  ReactFlow,
  type Connection,
  type Edge,
  type Node,
  type NodeChange,
  type NodeProps,
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import { AlertCircle, Code2, Database, NotebookPen, Table2 } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';

import type { BlockSpec, PipelineBlock, PipelineEdge } from '@/api/generated/types.gen';
import type { BlockOutcome, PipelineOutcomes } from '@/lib/pipeline';
import { cn, formatCount } from '@/lib/utils';

/**
 * A curation as boxes and arrows: the thing that gets edited.
 *
 * Drawn with the same library as the workflow graph, and the difference
 * between them is worth naming. That one **shows** an execution somebody
 * else's orchestrator declared, and nothing on it can be moved. This one is
 * where a curation is *made*: a block is added, connected, dragged, opened and
 * removed here, and the shape it ends up in is what gets saved.
 *
 * What it does not do is decide whether that shape is valid. The registry
 * refuses a chain that cannot run and returns every reason at once; this canvas
 * renders those lines and implements no rules of its own — the same split the
 * annotation canvas keeps with the shape validator, and for the same reason: a
 * second rule set in TypeScript drifts from the first, and the day it does
 * somebody trusts the wrong one.
 */

type BlockKind = BlockSpec['kind'];

const KIND: Record<BlockKind, { label: string; icon: LucideIcon; tone: string }> = {
  source: { label: 'Source', icon: Database, tone: 'text-primary' },
  transform: { label: 'Flow PHP', icon: Code2, tone: 'text-warning' },
  notebook: { label: 'marimo', icon: NotebookPen, tone: 'text-success' },
  view: { label: 'View', icon: Table2, tone: 'text-muted-foreground' },
};

/** What a block is showing, under its name. */
function describeBlock(spec: BlockSpec): string {
  switch (spec.kind) {
    case 'source':
      return spec.dataset === 'hub_rows'
        ? (spec.arguments?.dataset ?? 'a hub corpus')
        : spec.dataset;
    case 'transform': {
      const steps = (spec.steps ?? '').split('\n').filter((line) => line.trim());
      return steps.length === 1 ? '1 step' : `${steps.length} steps`;
    }
    case 'notebook':
      return spec.notebook;
    case 'view':
      return spec.dataset ?? 'not published';
  }
}

export function blockLabel(kind: BlockKind): string {
  return KIND[kind].label;
}

type BlockData = {
  block: PipelineBlock;
  outcome: BlockOutcome;
  active: boolean;
};

function BlockNode({ data }: NodeProps<Node<BlockData, 'block'>>) {
  const { block, outcome, active } = data;
  const { icon: Icon, label, tone } = KIND[block.spec.kind];
  return (
    <div
      className={cn(
        'w-[13rem] cursor-pointer rounded-lg border-2 bg-card px-3 py-2 shadow-sm transition-colors',
        active ? 'border-primary' : 'border-border',
        outcome.status === 'failed' && 'border-danger bg-danger/5',
        outcome.status === 'running' && 'border-running',
      )}
    >
      {block.spec.kind !== 'source' ? (
        <Handle
          type="target"
          position={Position.Left}
          className="!h-2.5 !w-2.5 !border-border !bg-muted"
        />
      ) : null}
      <div className="flex items-center gap-1.5">
        <Icon className={cn('h-3.5 w-3.5 shrink-0', tone)} />
        <span className="truncate text-sm font-medium">{block.title || label}</span>
        {outcome.status === 'running' ? (
          <span className="ml-auto h-2 w-2 shrink-0 animate-pulse rounded-full bg-running" />
        ) : null}
        {outcome.status === 'failed' ? (
          <AlertCircle className="ml-auto h-3.5 w-3.5 shrink-0 text-danger" />
        ) : null}
      </div>
      <p
        className="mt-0.5 truncate text-[0.7rem] text-muted-foreground"
        title={describeBlock(block.spec)}
      >
        {label} · {describeBlock(block.spec)}
      </p>
      <p className="mt-1 truncate text-[0.7rem] tabular-nums text-muted-foreground">
        {describeOutcome(outcome)}
      </p>
      {block.spec.kind !== 'view' ? (
        <Handle
          type="source"
          position={Position.Right}
          className="!h-2.5 !w-2.5 !border-border !bg-muted"
        />
      ) : null}
    </div>
  );
}

/**
 * The line under a block, from whichever of the two ran it.
 *
 * The ad-hoc path counted rows and milliseconds; a managed run reports the word
 * the server used and nothing else, because a step's timings are the log's
 * answer rather than the workflow store's. So the counts are printed when they
 * exist and the note carries the rest — never `0 rows · 0 ms`, which would be a
 * measurement nobody took.
 */
function describeOutcome(outcome: BlockOutcome): string {
  if (outcome.status === 'failed') return outcome.message;
  if (outcome.status === 'running') return outcome.note ?? 'running…';
  if (outcome.status === 'idle') return outcome.note ?? 'not run';
  const measured =
    outcome.rows === undefined
      ? undefined
      : `${formatCount(outcome.rows)} rows · ${outcome.tookMs ?? 0} ms`;
  return [measured, outcome.note].filter(Boolean).join(' · ') || 'done';
}

const NODE_TYPES = { block: BlockNode };

export function PipelineCanvas({
  blocks,
  edges,
  outcomes,
  selected,
  onSelect,
  onMove,
  onConnect,
  onDisconnect,
  onDelete,
}: {
  blocks: PipelineBlock[];
  edges: PipelineEdge[];
  outcomes: PipelineOutcomes;
  selected?: string;
  onSelect: (id: string) => void;
  onMove: (id: string, position: { x: number; y: number }) => void;
  onConnect: (edge: PipelineEdge) => void;
  onDisconnect: (edge: PipelineEdge) => void;
  onDelete: (id: string) => void;
}) {
  const nodes: Node<BlockData, 'block'>[] = React.useMemo(
    () =>
      blocks.map((block) => ({
        id: block.id,
        type: 'block' as const,
        // `x` and `y` are optional in the contract because they default in
        // the registry; a block that has never been dragged sits at the origin.
        position: { x: block.position?.x ?? 0, y: block.position?.y ?? 0 },
        data: {
          block,
          outcome: outcomes[block.id] ?? { status: 'idle' },
          active: block.id === selected,
        },
      })),
    [blocks, outcomes, selected],
  );

  const flowEdges: Edge[] = React.useMemo(
    () =>
      edges.map((edge) => ({
        id: `${edge.from}-${edge.to}`,
        source: edge.from,
        target: edge.to,
        animated: outcomes[edge.to]?.status === 'running',
      })),
    [edges, outcomes],
  );

  return (
    <div className="h-[26rem] w-full overflow-hidden rounded-lg border border-border bg-muted/10">
      <ReactFlow
        nodes={nodes}
        edges={flowEdges}
        nodeTypes={NODE_TYPES}
        fitView
        proOptions={{ hideAttribution: true }}
        onNodeClick={(_, node) => onSelect(node.id)}
        onNodesChange={(changes: NodeChange<Node<BlockData, 'block'>>[]) => {
          for (const change of changes) {
            // Every position event, not only the last one: a block's position
            // is held in the page's state because it is part of what gets
            // saved, and dropping the intermediate ones makes a drag jump.
            if (change.type === 'position' && change.position) {
              onMove(change.id, change.position);
            }
            if (change.type === 'remove') onDelete(change.id);
          }
        }}
        onEdgesChange={(changes) => {
          for (const change of changes) {
            if (change.type !== 'remove') continue;
            const edge = edges.find(
              (candidate) => `${candidate.from}-${candidate.to}` === change.id,
            );
            if (edge) onDisconnect(edge);
          }
        }}
        onConnect={(connection: Connection) => {
          if (connection.source && connection.target) {
            onConnect({ from: connection.source, to: connection.target });
          }
        }}
      >
        <Background variant={BackgroundVariant.Dots} gap={16} size={1} />
        <Controls showInteractive={false} />
      </ReactFlow>
    </div>
  );
}
