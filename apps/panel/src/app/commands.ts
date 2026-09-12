import type { LucideIcon } from 'lucide-react';

import { SECTIONS, type NavSection } from '@/app/navigation';

/**
 * What can be typed, and where each thing lands.
 *
 * The panel's whole view state lives in the URL — that is the rule every
 * filter here already follows — so "go somewhere with a filter applied" is
 * expressible as a route and a search object, and nothing else. That is what
 * makes a command palette possible at all: a command is a link, and running
 * one is a navigation rather than a script that drives the page.
 *
 * **Matched, never parsed.** Typing selects from a list this file authors, the
 * way a shell completes a command rather than interpreting a sentence. The
 * alternative — sending the text to a model and letting it choose the route —
 * would make navigation a network call that can be wrong, and "it went
 * somewhere else this time" is a bad property for the thing you press to get
 * unlost.
 *
 * **Every command corresponds to an affordance that exists.** A command that
 * navigated somewhere and left the reader to find the feature is the empty
 * state with extra steps, and one that promised an action this panel cannot
 * perform is the plausible fake CLAUDE.md's Panel section warns about. So
 * "Create a new dataset" lands on the Recipe view, where a curation is
 * actually authored, and says so in its own hint rather than pretending there
 * is a dialog.
 */

/** Where a command goes: a path, and the search params that make it specific. */
export interface CommandTarget {
  to: string;
  search?: Record<string, unknown>;
}

export interface Command extends CommandTarget {
  /** Stable across renames of the label; what a test and a recent list hold. */
  id: string;
  label: string;
  /** The area this belongs to, drawn beside the label so two "Runs" differ. */
  group: string;
  icon?: LucideIcon;
  /**
   * One line saying what the reader will be looking at. Carries the honesty
   * for the commands whose name promises more than the page does.
   */
  hint?: string;
  /**
   * Words somebody might type that are not in the label.
   *
   * The point of the palette is that "live", "tail" and "what is running now"
   * all reach the Live view. Without these a palette is a list you have to
   * already know the wording of, which is the navigation you already had.
   */
  keywords?: string[];
}

/**
 * Going to a page, derived from the navigation rather than listed again.
 *
 * `app/navigation.ts` is already the one description of what pages exist; a
 * second list here would be free to disagree with the sidebar the day an area
 * is added, and the palette is exactly where that would go unnoticed.
 */
function navigationCommands(): Command[] {
  return SECTIONS.flatMap((section: NavSection) =>
    section.areas.flatMap((area) => {
      // An area that is one page is one command; an area with views is one per
      // view, because "Observability" alone is not somewhere you can be.
      const views = area.views.length > 0 ? area.views : [{ to: area.to, label: area.label }];
      return views.map((view) => ({
        id: `go:${view.to}`,
        label:
          area.views.length > 0 ? `Go to ${area.label} › ${view.label}` : `Go to ${area.label}`,
        group: section.label,
        icon: area.icon,
        hint: area.blurb,
        to: view.to,
        keywords: [area.label, view.label, section.label],
      }));
    }),
  );
}

/**
 * The commands that carry a filter, which are the ones worth typing.
 *
 * Each is a question somebody actually arrives with — "what is running right
 * now", "what is waiting on me", "what failed" — expressed as the search
 * params that view already understands. They are authored rather than
 * generated because the *interesting* combinations are a matter of judgement:
 * the cross product of every param and every value is thousands of rows and
 * almost none of them is a question.
 */
const FILTERED: Command[] = [
  // ── Inference ──────────────────────────────────────────────────────────────
  {
    id: 'traces:live',
    label: 'Show traces for live agents',
    group: 'Observability',
    // No filter, and that is the honest answer rather than a missing one: this
    // view *is* the log as it arrives, so everything in it is live by
    // definition. A `status: running` beside it read like a narrowing and was
    // none — the view says so itself, because status is assembled from several
    // events and is not something one carries (ADR_0003).
    hint: 'The durable log as it arrives. Narrow it by agent, runtime or session.',
    to: '/observability/live',
    keywords: ['tail', 'follow', 'streaming', 'now', 'watch', 'sse', 'running', 'live'],
  },
  {
    id: 'runs:failed',
    label: 'Show runs that failed',
    group: 'Observability',
    hint: 'The runs list, narrowed to the ones that ended badly.',
    to: '/observability/runs',
    search: { status: 'failed' },
    keywords: ['errors', 'broken', 'exceptions', 'failures'],
  },
  {
    id: 'runs:running',
    label: 'Show runs still going',
    group: 'Observability',
    // A run with no end event stays `Running`; the list says when it was last
    // heard from rather than guessing that it died.
    hint: 'Runs with no end event yet, and when each was last heard from.',
    to: '/observability/runs',
    search: { status: 'running' },
    keywords: ['in flight', 'unfinished', 'open', 'stalled', 'hanging'],
  },
  {
    id: 'traces:by-model',
    label: 'Compare runs by model',
    group: 'Observability',
    hint: 'The run tree pivoted on the model each span named.',
    to: '/observability/explore',
    search: { by: 'model' },
    keywords: ['models', 'pivot', 'group by', 'llm'],
  },
  {
    id: 'traces:by-agent',
    label: 'Compare runs by agent',
    group: 'Observability',
    hint: 'The run tree pivoted on the agent that produced it.',
    to: '/observability/explore',
    search: { by: 'agent' },
    keywords: ['agents', 'pivot', 'group by'],
  },
  {
    id: 'traces:by-tool',
    label: 'Compare runs by tool',
    group: 'Observability',
    hint: 'The run tree pivoted on the tools that were called.',
    to: '/observability/explore',
    search: { by: 'tool' },
    keywords: ['tools', 'pivot', 'group by', 'function calls'],
  },
  {
    id: 'traces:query',
    label: 'Ask a question of the runs',
    group: 'Observability',
    hint: 'The Query tab, in whichever engine this deployment runs.',
    to: '/observability/query',
    keywords: ['sql', 'flow', 'duckdb', 'datafusion', 'aggregate', 'analyse'],
  },
  {
    id: 'traces:metrics',
    label: 'Show throughput, latency and cost',
    group: 'Observability',
    hint: 'Metrics over the selected period.',
    to: '/observability/metrics',
    keywords: ['tokens', 'spend', 'p95', 'charts', 'graphs'],
  },

  // ── Feature ────────────────────────────────────────────────────────────────
  {
    id: 'dataset:new',
    label: 'Create a new dataset',
    group: 'Data Curation',
    // The honest hint. There is no create-dataset dialog: a version is
    // published by a curation's view block, so this lands where one is written.
    hint: 'Opens the Recipe view — a dataset version is published by a curation.',
    to: '/data-curation/recipe',
    keywords: ['new', 'add', 'author', 'curation', 'recipe', 'publish'],
  },
  {
    id: 'dataset:pipeline',
    label: 'Build a curation pipeline',
    group: 'Data Curation',
    hint: 'The block canvas: source, transform, notebook, approval, view.',
    to: '/data-curation/pipeline',
    search: { view: 'canvas' },
    keywords: ['blocks', 'chain', 'canvas', 'notebook', 'flow'],
  },
  {
    id: 'dataset:discover',
    label: 'Search Kaggle and Hugging Face',
    group: 'Datasets',
    hint: 'What exists on the hubs. What is permitted is the licence at the link.',
    to: '/datasets',
    search: { view: 'discover' },
    keywords: ['hub', 'hubs', 'kaggle', 'huggingface', 'corpus', 'find data'],
  },
  {
    id: 'annotations:review',
    label: 'Show annotations waiting for review',
    group: 'Annotations',
    hint: 'Drawn and not yet accepted.',
    to: '/annotations/label',
    search: { review: 'in_review' },
    keywords: ['label', 'pending', 'queue', 'approve', 'images'],
  },
  {
    id: 'annotations:imports',
    label: 'Show what an import refused',
    group: 'Annotations',
    hint: 'Counts by reason, and the rows behind them.',
    to: '/annotations/imports',
    keywords: ['rejected', 'failed rows', 'batch', 'staged'],
  },
  {
    id: 'conversations:pending',
    label: 'Show conversations waiting on a person',
    group: 'Conversations',
    hint: 'The review gate. Reading the words themselves needs the admin role.',
    to: '/conversations/review',
    search: { review: 'pending' },
    keywords: ['turns', 'gate', 'approve', 'queue', 'unreviewed'],
  },
  {
    id: 'conversations:pii',
    label: 'Show conversations with personal data found',
    group: 'Conversations',
    hint: 'Findings from the shape scanner, from the plaintext head.',
    to: '/conversations/review',
    search: { finding: 'pii' },
    keywords: ['pii', 'personal', 'secrets', 'redaction', 'gdpr'],
  },

  // ── Training ───────────────────────────────────────────────────────────────
  {
    id: 'training:running',
    label: 'Show training runs in flight',
    group: 'Training',
    hint: 'The curve while it runs. There is no progress bar; nothing knows the epoch count.',
    to: '/training/runs',
    search: { status: 'running' },
    keywords: ['fitting', 'epochs', 'loss', 'curve', 'now'],
  },
  {
    id: 'training:failed',
    label: 'Show training runs that failed',
    group: 'Training',
    to: '/training/runs',
    search: { status: 'failed' },
    keywords: ['broken', 'crashed', 'errors'],
  },
  {
    id: 'training:models',
    label: 'Show the model registry',
    group: 'Training',
    hint: 'Versions, their labels, and the run each learned from.',
    to: '/training/models',
    keywords: ['models', 'promote', 'production', 'weights', 'onnx', 'registry'],
  },
  {
    id: 'prompts:all',
    label: 'Show prompts and their versions',
    group: 'Prompts',
    hint: 'Versions by content, and whether an optimisation was an improvement.',
    to: '/prompts',
    keywords: ['prompt', 'optimisation', 'optimization', 'labels', 'production'],
  },
  {
    id: 'evaluation:all',
    label: 'Show evaluation reports',
    group: 'Evaluation',
    hint: 'Against a suite and a dataset, compared with a baseline.',
    to: '/evaluation',
    keywords: ['evals', 'scores', 'suite', 'baseline', 'benchmark'],
  },
];

/** Everything the palette can run. Navigation first so a bare "go" is obvious. */
export function allCommands(): Command[] {
  return [...FILTERED, ...navigationCommands()];
}

// ── Matching ─────────────────────────────────────────────────────────────────

/**
 * Whether `query`'s characters appear in `text` in order, and how tightly.
 *
 * A subsequence match rather than a substring one, because that is what makes
 * a palette feel like a CLI: `sdlv` reaches "Show traces for live agents" and
 * nobody has to remember the wording. The score rewards matches that start a
 * word and runs of adjacent characters, so an initialism beats a scatter of
 * letters spread across the sentence.
 *
 * `null` for no match, so a caller cannot mistake a score of zero for one.
 */
export function fuzzyScore(text: string, query: string): number | null {
  const haystack = text.toLowerCase();
  const needle = query.toLowerCase();
  if (needle.length === 0) return 0;

  let score = 0;
  let from = 0;
  let previous = -2;
  for (const character of needle) {
    // A space in the query matches nothing in particular: it is how somebody
    // separates words they remember, not a character they expect to find.
    if (character === ' ') continue;
    const at = haystack.indexOf(character, from);
    if (at === -1) return null;
    if (at === previous + 1) score += 6; // a run
    if (at === 0 || ' -/›'.includes(haystack[at - 1] ?? '')) score += 10; // a word start
    score -= Math.min(at - from, 6); // distance travelled, bounded
    previous = at;
    from = at + 1;
  }
  return score;
}

/**
 * The commands that match, best first.
 *
 * The label is what a score is taken from; keywords can only *admit* a
 * command, at a discount. Scoring them equally let a command with a long
 * keyword list beat the one whose name was being typed — "live" reached four
 * things before it reached the Live view.
 */
export function search(commands: Command[], query: string): Command[] {
  const trimmed = query.trim();
  if (trimmed.length === 0) return commands;

  return commands
    .map((command) => {
      const direct = fuzzyScore(command.label, trimmed);
      const byGroup = fuzzyScore(`${command.group} ${command.label}`, trimmed);
      const byKeyword = (command.keywords ?? [])
        .map((keyword) => fuzzyScore(keyword, trimmed))
        .filter((value): value is number => value !== null);
      const best = Math.max(
        direct ?? -Infinity,
        (byGroup ?? -Infinity) - 4,
        ...byKeyword.map((value) => value - 8),
      );
      return { command, score: best };
    })
    .filter((row) => Number.isFinite(row.score))
    .sort((a, b) => b.score - a.score)
    .map((row) => row.command);
}
