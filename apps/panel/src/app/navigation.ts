import {
  Bot,
  Boxes,
  BookOpen,
  Database,
  FlaskConical,
  type LucideIcon,
  MessagesSquare,
  Radio,
  ScrollText,
  Settings,
  Shapes,
  Sigma,
  Sparkles,
  Telescope,
  WandSparkles,
  Workflow,
} from 'lucide-react';

/**
 * The navigation, as one description rather than as markup in eleven files.
 *
 * Areas are peers grouped by the work people do; CLAUDE.md's Panel section says where the line falls and why.
 * The root layout supports all-area and classic section navigation over this
 * same route catalog; switching layouts never changes the selected object.
 */

export interface NavView {
  to: string;
  label: string;
}

export interface NavArea {
  /** Where the label itself goes. A single-view area links straight at its page. */
  to: string;
  label: string;
  icon: LucideIcon;
  /** One line, shown on the section's landing card. Never a second heading. */
  blurb: string;
  /** The pages inside it. Empty for an area that is one page. */
  views: NavView[];
  /**
   * The one search parameter that survives a move between this area's views.
   *
   * Which parameter it is differs by area and the reason is the same in each:
   * it is the half of the question the reader has already answered. In
   * Observability that is the period — having narrowed to fifteen minutes,
   * "now the metrics for it" is the next question. In Annotations it is the
   * project — "label this, now export it" is one thought, and a tab switch
   * that dropped it would make it two. Everything else belongs to the view
   * that owns it and is dropped.
   */
  carries?: 'window' | 'project';
}

export interface NavSection {
  id: SectionId;
  label: string;
  icon: LucideIcon;
  blurb: string;
  /** Where clicking the section itself lands. The area somebody opens first. */
  home: string;
  areas: NavArea[];
}

export type SectionId =
  | 'feature'
  | 'training'
  | 'inference'
  | 'workflows'
  | 'learning'
  | 'system';

export const SECTIONS: NavSection[] = [
  {
    id: 'feature',
    label: 'Data',
    icon: Boxes,
    blurb: 'What a model learns from: curated, drawn, reviewed, and frozen.',
    home: '/datasets',
    areas: [
      {
        to: '/datasets',
        label: 'Datasets',
        icon: Database,
        blurb: 'Browse datasets, versions and their sources.',
        views: [],
      },
      {
        to: '/data-curation/pipeline',
        label: 'Data Curation',
        icon: WandSparkles,
        blurb: 'Prepare data, inspect results and publish dataset versions.',
        carries: 'window',
        views: [
          { to: '/data-curation/pipeline', label: 'Pipeline' },
          { to: '/data-curation/recipe', label: 'Recipe' },
        ],
      },
      {
        to: '/annotations/label',
        label: 'Annotations',
        icon: Shapes,
        blurb: 'Label images, review annotations and export training data.',
        carries: 'project',
        views: [
          { to: '/annotations/label', label: 'Label' },
          { to: '/annotations/sources', label: 'Sources' },
          { to: '/annotations/imports', label: 'Imports' },
          { to: '/annotations/exports', label: 'Exports' },
        ],
      },
      {
        to: '/conversations/review',
        label: 'Conversations',
        icon: MessagesSquare,
        blurb: 'Review conversations and curate approved examples.',
        views: [
          { to: '/conversations/review', label: 'Review' },
          { to: '/conversations/corpora', label: 'Corpora' },
        ],
      },
    ],
  },
  {
    id: 'training',
    label: 'Models & quality',
    icon: Sigma,
    blurb: 'Fitting a model or a prompt, and the evidence that it got better.',
    home: '/training/models',
    areas: [
      {
        to: '/training/models', label: 'Models', icon: Boxes,
        blurb: 'Model versions, evaluation evidence and the data used to train them.', views: [],
      },
      {
        to: '/training/runs',
        label: 'Training',
        icon: Sigma,
        blurb: 'Training runs, parameters and learning curves.',
        views: [],
      },
      {
        to: '/experiments',
        label: 'Experiments',
        icon: Sparkles,
        blurb: 'Compare variants on quality, latency and cost.',
        views: [],
      },
      {
        to: '/evaluation',
        label: 'Evaluation',
        icon: FlaskConical,
        blurb: 'Reports against a suite and a dataset, compared to a baseline.',
        views: [],
      },
    ],
  },
  {
    id: 'inference',
    label: 'Applications',
    icon: Radio,
    blurb: 'What is running right now, and what it did when it ran.',
    home: '/observability/explore',
    areas: [
      {
        to: '/agents',
        label: 'Agents',
        icon: Bot,
        blurb: 'What each agent runs on, what it calls, what it costs, and its runs.',
        views: [],
      },
      {
        to: '/prompts',
        label: 'Prompts',
        icon: ScrollText,
        blurb: 'Inspect prompt versions, production labels and evaluation results.',
        views: [],
      },
      {
        to: '/observability/explore',
        label: 'Observability',
        icon: Telescope,
        blurb: 'Explore run history, live events, metrics and queries.',
        carries: 'window',
        views: [
          { to: '/observability/explore', label: 'Explore' },
          { to: '/observability/live', label: 'Live' },
          { to: '/observability/query', label: 'Query' },
          { to: '/observability/metrics', label: 'Metrics' },
          { to: '/observability/runs', label: 'Runs' },
        ],
      },
    ],
  },
  {
    id: 'workflows',
    label: 'Workflows',
    icon: Workflow,
    blurb: 'Workflow definitions and executions.',
    home: '/workflows',
    areas: [
      {
        to: '/workflows',
        label: 'Workflows',
        icon: Workflow,
        blurb: 'Inspect workflow graphs and follow their executions.',
        views: [],
      },
    ],
  },
  {
    id: 'learning', label: 'Learning', icon: BookOpen,
    blurb: 'Workshops and practical labs.', home: '/learning',
    areas: [{ to: '/learning', label: 'Workshops & labs', icon: BookOpen,
      // Timed access *is* available and is the grant window; what has no
      // contract is a lab's brief, its tests and its mark, and the area says
      // which of the two each part is rather than one sentence covering both.
      blurb: 'Workshops, who is on them and until when; the labs are still empty slots.', views: [] }],
  },
  {
    id: 'system',
    label: 'System',
    icon: Settings,
    blurb: 'What this deployment has wired, and the variable that decides each.',
    home: '/system',
    areas: [
      {
        to: '/system',
        label: 'System',
        icon: Settings,
        // The one area that describes the deployment rather than anything it
        // holds — and the one whose read the server answers only for an
        // admin, which the page renders as a sentence rather than hiding the
        // link. A link that vanished by role would make "is there a System
        // area" a question nobody can answer from the panel.
        blurb: 'Runtimes, integrations and instance configuration, read only.',
        views: [],
      },
    ],
  },
];

/**
 * Which section a path belongs to.
 *
 * Longest-prefix, so `/training/models` picks Training rather than matching
 * something shorter first. A path in no section — a run detail, a prompt's own
 * page — resolves through [`SECTION_OF`] below instead of falling back to the
 * first section, because highlighting Feature while somebody reads a run is
 * worse than highlighting nothing.
 */
const SECTION_OF: Array<[prefix: string, id: SectionId]> = [
  ['/observability', 'inference'],
  ['/agents', 'inference'],
  ['/workflows', 'workflows'],
  ['/learning', 'learning'],
  ['/system', 'system'],
  ['/runs', 'inference'],
  ['/training', 'training'],
  ['/experiments', 'training'],
  ['/evaluation', 'training'],
  ['/prompts', 'inference'],
  ['/datasets', 'feature'],
  ['/data-curation', 'feature'],
  ['/annotations', 'feature'],
  ['/conversations', 'feature'],
];

export function sectionOf(pathname: string): NavSection | undefined {
  const id = SECTION_OF.find(
    ([prefix]) => pathname === prefix || pathname.startsWith(`${prefix}/`),
  )?.[1];
  return SECTIONS.find((section) => section.id === id);
}

/** The area within a section that a path is inside, for the sidebar's highlight. */
export function areaOf(section: NavSection, pathname: string): NavArea | undefined {
  const exact = section.areas.find((area) => pathname === area.to || pathname.startsWith(`${area.to}/`));
  if (exact) return exact;
  return section.areas.find((area) => {
    const root = `/${area.to.split('/')[1]}`;
    return pathname === root || pathname.startsWith(`${root}/`);
  });
}
