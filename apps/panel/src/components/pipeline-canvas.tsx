import * as React from 'react';
import {
  Background,
  BackgroundVariant,
  Controls,
  Handle,
  Panel,
  Position,
  ReactFlow,
  useReactFlow,
  type Connection,
  type Edge,
  type Node,
  type NodeChange,
  type NodeProps,
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import {
  AlertCircle,
  Code2,
  Database,
  Layers,
  Maximize2,
  NotebookPen,
  ShieldCheck,
  Table2,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';

import type { BlockSpec, PipelineBlock, PipelineEdge } from '@/api/generated/types.gen';
import {
  FlowCard,
  FlowLegend,
  ReachControl,
  flowEdgeClass,
  type FlowRole,
  type FlowState,
} from '@/components/flow-visuals';
import { describeOutcome, type BlockOutcome, type PipelineOutcomes } from '@/lib/pipeline';
import { foldPhases, phaseOf, type CanvasItem, type Phase, type PhaseId } from '@/lib/phases';
import { edgeInReach, nodeInReach, reachFrom, type ReachMode } from '@/lib/reach';
import { formatCount } from '@/lib/utils';
import { layoutGraph } from '@/lib/workflow-layout';

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
 *
 * What it draws with is `flow-visuals`, shared with the workflow graph: a
 * block's colour is its **kind**, which never changes, and its state is drawn
 * as motion on top. Those are two different questions and a reader normally
 * arrives already knowing the first.
 */

type BlockKind = BlockSpec['kind'];

const KIND: Record<BlockKind, { label: string; icon: LucideIcon; role: FlowRole }> = {
  source: { label: 'Source', icon: Database, role: 'data' },
  transform: { label: 'Data transformation', icon: Code2, role: 'compute' },
  notebook: { label: 'Python', icon: NotebookPen, role: 'runtime' },
  approval: { label: 'Approval', icon: ShieldCheck, role: 'gate' },
  view: { label: 'Publish dataset', icon: Table2, role: 'publish' },
};

/**
 * The four words a block's state is drawn with. The outcome already speaks
 * them; this exists so the canvas and the workflow graph agree on the spelling
 * rather than each mapping its own status vocabulary onto the same animations.
 */
function stateOf(outcome: BlockOutcome): FlowState {
  switch (outcome.status) {
    case 'running':
      return 'live';
    case 'failed':
      return 'failed';
    case 'done':
      return 'done';
    case 'idle':
      return 'idle';
  }
}

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
    case 'approval':
      // The question, because that is what somebody has to read to know what
      // this box is holding up. The answers are in the inspector and, while a
      // managed run waits on it, on the run's own card.
      return spec.prompt || 'no question yet';
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
  away: boolean;
};

function BlockNode({ data }: NodeProps<Node<BlockData, 'block'>>) {
  const { block, outcome, active, away } = data;
  const { icon, label, role } = KIND[block.spec.kind];
  return (
    <FlowCard
      role={role}
      state={stateOf(outcome)}
      selected={active}
      away={away}
      mark={icon}
      title={block.title || label}
      sublabel={
        <span title={describeBlock(block.spec)}>
          {label} · {describeBlock(block.spec)}
        </span>
      }
      footer={
        <>
          {outcome.status === 'failed' ? (
            <AlertCircle className="h-3 w-3 shrink-0 text-danger" />
          ) : null}
          <span className="truncate tabular-nums">{describeOutcome(outcome)}</span>
        </>
      }
      className="cursor-pointer"
    >
      {block.spec.kind !== 'source' ? (
        <Handle
          type="target"
          position={Position.Left}
          className="!h-2.5 !w-2.5 !border-border !bg-muted"
        />
      ) : null}
      {block.spec.kind !== 'view' ? (
        <Handle
          type="source"
          position={Position.Right}
          className="!h-2.5 !w-2.5 !border-border !bg-muted"
        />
      ) : null}
    </FlowCard>
  );
}

type PhaseData = {
  phase: Phase;
  blocks: PipelineBlock[];
  outcome: BlockOutcome;
  away: boolean;
};

/**
 * A phase, folded shut.
 *
 * It reports the worst thing inside it rather than a total, because that is
 * the question somebody folding six steps away still needs answered: a box
 * that says "6 blocks" over a failed step would be hiding the one thing worth
 * seeing.
 */
function PhaseNode({ data }: NodeProps<Node<PhaseData, 'phase'>>) {
  const { phase, blocks, outcome, away } = data;
  return (
    <FlowCard
      role={PHASE_ROLE[phase.id]}
      state={stateOf(outcome)}
      away={away}
      mark={Layers}
      title={phase.label}
      sublabel={`${formatCount(blocks.length)} blocks · folded`}
      footer={
        <>
          <span className="truncate">{describeOutcome(outcome)}</span>
          <Maximize2 className="ml-auto h-3 w-3 shrink-0" />
        </>
      }
      className="cursor-pointer border-dashed"
    >
      <Handle
        type="target"
        position={Position.Left}
        className="!h-2.5 !w-2.5 !border-border !bg-muted"
      />
      <Handle
        type="source"
        position={Position.Right}
        className="!h-2.5 !w-2.5 !border-border !bg-muted"
      />
    </FlowCard>
  );
}

const PHASE_ROLE: Record<PhaseId, FlowRole> = {
  ingest: 'data',
  engineering: 'compute',
  ml: 'runtime',
  gate: 'gate',
  output: 'publish',
};

/**
 * What a folded phase reports: the worst state inside it.
 *
 * Failure outranks running outranks done, and idle is what is left. A box that
 * averaged them, or counted them, would answer a question nobody asked while
 * hiding the one they did.
 */
function worstOf(blocks: PipelineBlock[], outcomes: PipelineOutcomes): BlockOutcome {
  const inside = blocks.map((block) => outcomes[block.id] ?? { status: 'idle' as const });
  const failed = inside.find((outcome) => outcome.status === 'failed');
  if (failed) return failed;
  if (inside.some((outcome) => outcome.status === 'running')) {
    return { status: 'running', note: 'a step is running' };
  }
  if (inside.length > 0 && inside.every((outcome) => outcome.status === 'done')) {
    return { status: 'done', note: 'all done' };
  }
  return { status: 'idle', note: 'not run' };
}

/*
 * `phase`, not `group`: React Flow ships a built-in node type called `group`
 * and styles `.react-flow__node-group` with its own 150px width and padding,
 * which painted a grey box behind and beside every folded phase. A custom type
 * whose name collides with a built-in silently inherits that built-in's CSS.
 */
const NODE_TYPES = { block: BlockNode, phase: PhaseNode };

/**
 * Re-fit the view when something moved every block at once.
 *
 * `fitView` on `<ReactFlow>` only runs at mount, so a tidy that lays a
 * ten-block chain along one line left most of it off the right-hand edge —
 * the layout was right and the canvas looked broken. It has to be a child of
 * `<ReactFlow>` because that is where the store lives, and it renders nothing.
 */
function FitOnSignal({ signal }: { signal: number }) {
  const flow = useReactFlow();
  React.useEffect(() => {
    if (signal === 0) return;
    // A frame later, or it fits the positions of the render that is being
    // replaced: the layout was right and the view was framed on the old one.
    const at = requestAnimationFrame(() => flow.fitView({ padding: 0.14, duration: 320 }));
    return () => cancelAnimationFrame(at);
  }, [signal, flow]);
  return null;
}

function bothInPhase(blocks: PipelineBlock[], edge: PipelineEdge, phase: PhaseId): boolean {
  const at = (id: string) => blocks.find((block) => block.id === id);
  const from = at(edge.from);
  const to = at(edge.to);
  return (
    from !== undefined &&
    to !== undefined &&
    phaseOf(from.spec) === phase &&
    phaseOf(to.spec) === phase
  );
}

export function PipelineCanvas({
  blocks,
  edges,
  outcomes,
  selected,
  reach,
  onReach,
  phase,
  collapsed,
  onExpand,
  fitSignal = 0,
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
  reach?: ReachMode;
  onReach: (mode: ReachMode | undefined) => void;
  /** The stretch of the chain somebody is isolating from the phase strip. */
  phase?: PhaseId | undefined;
  /** Phases drawn as one box instead of their blocks. A view, never an edit. */
  collapsed?: ReadonlySet<PhaseId>;
  onExpand?: (phase: PhaseId) => void;
  /** Bumped when every block has moved at once, so the view re-fits. */
  fitSignal?: number;
  onSelect: (id: string) => void;
  onMove: (id: string, position: { x: number; y: number }) => void;
  onConnect: (edge: PipelineEdge) => void;
  onDisconnect: (edge: PipelineEdge) => void;
  onDelete: (id: string) => void;
}) {
  /*
   * The traced reach, or nothing. A trace needs a subject, so it is derived
   * from the selection rather than held beside it — which is also what stops
   * the canvas keeping a filter after the block it was about has gone.
   */
  const traced = React.useMemo(
    () => (selected && reach ? reachFrom(edges, selected) : undefined),
    [edges, selected, reach],
  );

  /*
   * Two controls recede the same blocks, so they answer through one predicate.
   * A phase is a *slice by kind* and a trace is a *walk along the edges*; with
   * both on, a block has to survive both, which is the honest reading of
   * having asked two questions at once.
   */
  const isAway = React.useCallback(
    (block: PipelineBlock) => {
      if (phase !== undefined && phaseOf(block.spec) !== phase) return true;
      if (traced !== undefined && reach !== undefined && !nodeInReach(traced, reach, block.id)) {
        return true;
      }
      return false;
    },
    [phase, traced, reach],
  );

  const shut = collapsed ?? new Set<PhaseId>();
  const folded = React.useMemo(() => foldPhases(blocks, edges, shut), [blocks, edges, shut]);

  /*
   * Where the boxes sit while a phase is folded.
   *
   * Stored positions describe the *blocks*, and a folded phase is not one — so
   * a box standing in for six steps would sit at the first of them and leave
   * the gap the other five used to fill. While anything is folded this is a
   * derived picture, so it is laid out like one, through the same `layoutGraph`
   * the tidy button uses. Nothing is written: expand it again and the
   * arrangement somebody made comes back untouched.
   */
  const drawn = React.useMemo(() => {
    if (shut.size === 0) return undefined;
    return new Map(
      layoutGraph(
        folded.items.map((item) => item.id),
        folded.edges,
      ).map((node) => [node.id, node.position]),
    );
  }, [shut, folded]);

  const nodes: Node[] = React.useMemo(
    () =>
      folded.items.map((item: CanvasItem) => {
        if (item.kind === 'group') {
          // The box takes the position of the first block in the run, so a
          // fold changes what is drawn and never where anything sits.
          const head = item.blocks[0];
          return {
            id: item.id,
            type: 'phase' as const,
            position: drawn?.get(item.id) ?? {
              x: head?.position?.x ?? 0,
              y: head?.position?.y ?? 0,
            },
            data: {
              phase: item.phase,
              blocks: item.blocks,
              outcome: worstOf(item.blocks, outcomes),
              away: phase !== undefined && item.phase.id !== phase,
            } satisfies PhaseData,
            // A box is a rendering of several blocks, not a block. Dragging it
            // would have to mean moving all of them, and deleting it would
            // have to mean deleting all of them — neither is what somebody
            // folding a phase away is asking for.
            draggable: false,
            deletable: false,
          };
        }
        return {
          id: item.id,
          type: 'block' as const,
          // `x` and `y` are optional in the contract because they default in
          // the registry; a block that has never been dragged sits at the origin.
          position: drawn?.get(item.id) ?? {
            x: item.block.position?.x ?? 0,
            y: item.block.position?.y ?? 0,
          },
          // Folded is a reading view: a drag would write a position the
          // derived layout immediately overrides, and the block would snap
          // back. Expand to arrange.
          draggable: drawn === undefined,
          data: {
            block: item.block,
            outcome: outcomes[item.id] ?? { status: 'idle' },
            active: item.id === selected,
            away: isAway(item.block),
          } satisfies BlockData,
        };
      }),
    [folded, outcomes, selected, isAway, phase, drawn],
  );

  const selectedBlock = blocks.find((block) => block.id === selected);

  const flowEdges: Edge[] = React.useMemo(
    () =>
      folded.edges.map((edge) => ({
        id: `${edge.from}-${edge.to}`,
        source: edge.from,
        target: edge.to,
        className: flowEdgeClass(
          stateOf(outcomes[edge.to] ?? { status: 'idle' }),
          // An edge belongs to a phase only when both of its ends do — the
          // arrow *into* a phase is the boundary, not part of the slice.
          (phase !== undefined && !bothInPhase(blocks, edge, phase)) ||
            (traced !== undefined &&
              reach !== undefined &&
              !edgeInReach(traced, reach, edge.from, edge.to)),
        ),
      })),
    [blocks, folded, outcomes, traced, reach, phase],
  );

  /* Only the kinds actually on the canvas: a legend for boxes nobody drew is
     a key to a picture that is not there. */
  const legend = React.useMemo(() => {
    const seen = new Map<FlowRole, string>();
    for (const block of blocks) {
      const { role, label } = KIND[block.spec.kind];
      if (!seen.has(role)) seen.set(role, label);
    }
    return [...seen].map(([role, label]) => ({ role, label }));
  }, [blocks]);

  return (
    <div className="space-y-2">
      <div className="flow-surface h-[26rem] w-full overflow-hidden rounded-lg border border-border">
        <ReactFlow
          nodes={nodes}
          edges={flowEdges}
          nodeTypes={NODE_TYPES}
          fitView
          proOptions={{ hideAttribution: true }}
          onNodeClick={(_, node) => {
            if (node.type === 'phase') {
              const phaseId = node.id.slice('phase:'.length) as PhaseId;
              onExpand?.(phaseId);
              return;
            }
            onSelect(node.id);
          }}
          onNodesChange={(changes: NodeChange[]) => {
            for (const change of changes) {
              // A folded phase is not in the draft, so nothing it reports
              // belongs there either.
              if ('id' in change && change.id.startsWith('phase:')) continue;
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
          <FitOnSignal signal={fitSignal} />
          <Background
            variant={BackgroundVariant.Dots}
            gap={16}
            size={1}
            color="var(--color-gridline)"
          />
          <Controls
            showInteractive={false}
            className="!border !border-border !bg-card [&_button]:!border-border [&_button]:!bg-card [&_button]:!fill-muted-foreground hover:[&_button]:!bg-accent"
          />
          {selectedBlock ? (
            <Panel position="top-right">
              <ReachControl
                mode={reach}
                onChange={onReach}
                subject={selectedBlock.title || blockLabel(selectedBlock.spec.kind)}
              />
            </Panel>
          ) : null}
        </ReactFlow>
      </div>
      <FlowLegend entries={legend} className="px-1" />
    </div>
  );
}
