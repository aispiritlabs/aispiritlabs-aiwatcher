/**
 * What a node reaches, and what reaches it.
 *
 * Archify's most useful reader feature is not a colour: it is being able to
 * click one box and see the rest of the picture recede to just the part that
 * feeds it and the part it feeds. On a five-stage graph that is a convenience;
 * on a ten-block curation it is the difference between reading the chain and
 * tracing it with a finger.
 *
 * This is a *traversal*, in the same sense `pipeline.ts::orderOf` is one. It
 * answers "what is connected to this, and which way round", never "is this
 * shape allowed" — the registry owns that, reports every reason at once, and a
 * second rule set here would drift from it.
 */

/** An edge, in the one shape both canvases already have. */
export type ReachEdge = { readonly from: string; readonly to: string };

export type Reach = {
  readonly focus: string;
  /** Everything that can reach the focus, transitively. Excludes the focus. */
  readonly upstream: ReadonlySet<string>;
  /** Everything the focus reaches, transitively. Excludes the focus. */
  readonly downstream: ReadonlySet<string>;
};

export type ReachMode = 'upstream' | 'downstream' | 'both';

function walk(adjacency: Map<string, string[]>, start: string): Set<string> {
  const seen = new Set<string>();
  const queue = [start];
  while (queue.length > 0) {
    // `shift` keeps this breadth-first, which costs nothing at these sizes and
    // makes the traversal order stable for the tests to state.
    const at = queue.shift();
    if (at === undefined) break;
    for (const next of adjacency.get(at) ?? []) {
      // A cycle is not this function's problem to report — it is the
      // registry's — but it is this function's problem not to hang on.
      if (next === start || seen.has(next)) continue;
      seen.add(next);
      queue.push(next);
    }
  }
  return seen;
}

export function reachFrom(edges: readonly ReachEdge[], focus: string): Reach {
  const forward = new Map<string, string[]>();
  const backward = new Map<string, string[]>();
  for (const edge of edges) {
    forward.set(edge.from, [...(forward.get(edge.from) ?? []), edge.to]);
    backward.set(edge.to, [...(backward.get(edge.to) ?? []), edge.from]);
  }
  return {
    focus,
    upstream: walk(backward, focus),
    downstream: walk(forward, focus),
  };
}

/** Is this node part of what the reader asked to see? */
export function nodeInReach(reach: Reach, mode: ReachMode, id: string): boolean {
  if (id === reach.focus) return true;
  if (mode !== 'downstream' && reach.upstream.has(id)) return true;
  if (mode !== 'upstream' && reach.downstream.has(id)) return true;
  return false;
}

/**
 * Is this edge on a path *through* the focus?
 *
 * Both ends being in reach is not enough. An edge from something upstream
 * straight to something downstream is a bypass — it goes round the focused
 * node rather than through it — and drawing it as part of the reach would
 * claim a route that does not exist. So an edge counts only when both of its
 * ends sit on the same side, with the focus itself belonging to both sides.
 */
export function edgeInReach(reach: Reach, mode: ReachMode, from: string, to: string): boolean {
  const up = (id: string) => id === reach.focus || reach.upstream.has(id);
  const down = (id: string) => id === reach.focus || reach.downstream.has(id);
  if (mode !== 'downstream' && up(from) && up(to)) return true;
  if (mode !== 'upstream' && down(from) && down(to)) return true;
  return false;
}
