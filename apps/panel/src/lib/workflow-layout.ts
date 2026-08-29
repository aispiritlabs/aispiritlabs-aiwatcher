import type { NodeState, WorkflowEdge } from '@/api/generated/types.gen';

/**
 * Where each node of a graph sits on the canvas.
 *
 * Hand-rolled rather than dagre, for the reason `waterfall.tsx` is hand-rolled:
 * a layout library would be a second dependency to hold an opinion about, and
 * the opinion that matters here is not "optimal" but **stable**. This canvas
 * re-renders on every live frame. A layout that reshuffled as a stage finished
 * would make the graph unreadable exactly while somebody is watching it.
 *
 * So the algorithm is longest-path layering — the standard first phase of a
 * Sugiyama layout — with two deliberate simplifications:
 *
 * * **No crossing minimisation.** Within a rank, nodes keep the order the
 *   producer declared them in. A declaration order is a human's idea of the
 *   sequence, and honouring it beats a heuristic that produces a slightly
 *   tidier picture nobody recognises.
 * * **Back edges do not affect ranking.** A graph with a cycle — two agents
 *   that call each other — has no topological order at all, so ranking ignores
 *   any edge that would push a node behind a node it already sits after. The
 *   edge is still drawn; it just does not get a say in where things go.
 */

/** Node box size, in canvas units. Must agree with `StageNode`'s CSS. */
export const NODE_WIDTH = 208;
export const NODE_HEIGHT = 84;
const RANK_GAP = 96;
const ROW_GAP = 28;

export interface PositionedNode {
  id: string;
  position: { x: number; y: number };
}

/**
 * Rank every node, then place it.
 *
 * A node with no incoming edge starts at rank 0. Everything else sits one past
 * the deepest thing that feeds it, which is what puts a fan-in stage *after*
 * both of its inputs rather than beside the earlier one.
 */
export function layoutGraph(nodes: NodeState[], edges: WorkflowEdge[]): PositionedNode[] {
  const order = new Map(nodes.map((node, index) => [node.node_id, index]));
  const known = (id: string) => order.has(id);
  const real = edges.filter((edge) => known(edge.from) && known(edge.to) && edge.from !== edge.to);

  const rank = new Map<string, number>(nodes.map((node) => [node.node_id, 0]));
  const incoming = new Map<string, string[]>();
  for (const edge of real) {
    incoming.set(edge.to, [...(incoming.get(edge.to) ?? []), edge.from]);
  }

  // Relax ranks until they stop moving. Bounded by the node count, which is
  // what makes a cycle terminate instead of spinning: on the pass where a back
  // edge would push a node past that bound, it is simply not applied.
  for (let pass = 0; pass < nodes.length; pass += 1) {
    let moved = false;
    for (const node of nodes) {
      const parents = incoming.get(node.node_id) ?? [];
      if (parents.length === 0) continue;
      const deepest = Math.max(...parents.map((parent) => rank.get(parent) ?? 0));
      const next = deepest + 1;
      if (next > (rank.get(node.node_id) ?? 0) && next < nodes.length) {
        rank.set(node.node_id, next);
        moved = true;
      }
    }
    if (!moved) break;
  }

  const rows = new Map<number, string[]>();
  for (const node of nodes) {
    const at = rank.get(node.node_id) ?? 0;
    rows.set(at, [...(rows.get(at) ?? []), node.node_id]);
  }

  const tallest = Math.max(...[...rows.values()].map((row) => row.length), 1);
  const positioned: PositionedNode[] = [];
  for (const [at, row] of rows) {
    // Declaration order within a rank, then centred against the widest rank so
    // a four-stage chain reads as a line rather than as a staircase.
    const sorted = [...row].sort((left, right) => (order.get(left) ?? 0) - (order.get(right) ?? 0));
    const offset = ((tallest - sorted.length) * (NODE_HEIGHT + ROW_GAP)) / 2;
    sorted.forEach((id, index) => {
      positioned.push({
        id,
        position: {
          x: at * (NODE_WIDTH + RANK_GAP),
          y: offset + index * (NODE_HEIGHT + ROW_GAP),
        },
      });
    });
  }
  return positioned;
}

/**
 * Where a message edge should attach when its ends are agents rather than
 * nodes.
 *
 * Agents are laid out on their own row above the graph, in first-seen order,
 * because an agent is not a stage: it can appear in several and in none. Giving
 * them a rank would put them in the pipeline, which is the one thing the
 * picture must not say.
 */
export function layoutAgents(agents: string[], graphWidth: number): PositionedNode[] {
  const total = Math.max(agents.length, 1);
  const span = Math.max(graphWidth, total * (NODE_WIDTH + 24));
  return agents.map((agent, index) => ({
    id: `agent:${agent}`,
    position: {
      x: (index * (span - NODE_WIDTH)) / Math.max(total - 1, 1),
      y: -(NODE_HEIGHT + 56),
    },
  }));
}

/** The canvas extent the nodes occupy, for centring and for the agent row. */
export function graphWidth(positioned: PositionedNode[]): number {
  return Math.max(...positioned.map((node) => node.position.x), 0) + NODE_WIDTH;
}
