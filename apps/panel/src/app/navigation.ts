import {
  Boxes,
  Database,
  FlaskConical,
  type LucideIcon,
  MessagesSquare,
  Radio,
  ScrollText,
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
 * Eleven areas side by side stopped being legible, and nothing said which were
 * about the same job, so they sit in three sections — the lifecycle every area
 * already belongs to:
 *
 * - **Feature** — what goes *in*. Datasets, curation, annotations,
 *   conversations. All authored, all content-addressed, all outside retention
 *   (ADR_0011, ADR_0017, ADR_0021).
 * - **Training** — making the thing, and judging it. Runs and models, the
 *   experiments that launch them, the evaluations that measure them, and the
 *   prompts that are the other thing an evaluation is evidence about.
 * - **Inference** — what is running now. The whole observability area, and the
 *   workflows those runs are stages of.
 *
 * The split is the crate table's, not a cosmetic one: everything under Feature
 * and Training reads an object store, everything under Inference folds the log.
 *
 * A section's areas and their views are drawn as a sidebar rather than a third
 * row of tabs: it shows both levels at once, so the third is visible without
 * being clicked into, and stacked tab rows would cost the top of every page.
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

export type SectionId = 'feature' | 'training' | 'inference';

export const SECTIONS: NavSection[] = [
  {
    id: 'feature',
    label: 'Feature',
    icon: Boxes,
    blurb: 'What a model learns from: curated, drawn, reviewed, and frozen.',
    home: '/datasets',
    areas: [
      {
        to: '/datasets',
        label: 'Datasets',
        icon: Database,
        blurb: 'Recipes, versions, and what the hubs say exists.',
        views: [],
      },
      {
        to: '/data-curation/pipeline',
        label: 'Data Curation',
        icon: WandSparkles,
        blurb: 'Blocks across three engines, ad-hoc or managed by the server.',
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
        blurb: 'Vector shapes on images, reviewed, split by subject, exported.',
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
        blurb: 'Encrypted turns, a human gate, and the corpora that come out.',
        views: [
          { to: '/conversations/review', label: 'Review' },
          { to: '/conversations/corpora', label: 'Corpora' },
        ],
      },
    ],
  },
  {
    id: 'training',
    label: 'Training',
    icon: Sigma,
    blurb: 'Fitting a model or a prompt, and the evidence that it got better.',
    home: '/training/runs',
    areas: [
      {
        to: '/training/runs',
        label: 'Training',
        icon: Sigma,
        blurb: 'The curve while it runs, and the model registry it feeds.',
        views: [
          { to: '/training/runs', label: 'Runs' },
          { to: '/training/models', label: 'Models' },
        ],
      },
      {
        to: '/experiments',
        label: 'Experiments',
        icon: Sparkles,
        blurb: 'Comparing variants on quality, latency and cost — not built yet.',
        views: [],
      },
      {
        to: '/evaluation',
        label: 'Evaluation',
        icon: FlaskConical,
        blurb: 'Reports against a suite and a dataset, compared to a baseline.',
        views: [],
      },
      {
        to: '/prompts',
        label: 'Prompts',
        icon: ScrollText,
        blurb: 'Versions by content, and whether an optimisation was one.',
        views: [],
      },
    ],
  },
  {
    id: 'inference',
    label: 'Inference',
    icon: Radio,
    blurb: 'What is running right now, and what it did when it ran.',
    home: '/observability/explore',
    areas: [
      {
        to: '/observability/explore',
        label: 'Observability',
        icon: Telescope,
        blurb: 'Every level of a run, its metrics, and questions asked of both.',
        carries: 'window',
        views: [
          { to: '/observability/explore', label: 'Explore' },
          { to: '/observability/live', label: 'Live' },
          { to: '/observability/query', label: 'Query' },
          { to: '/observability/metrics', label: 'Metrics' },
          { to: '/observability/runs', label: 'Runs' },
        ],
      },
      {
        to: '/workflows',
        label: 'Workflows',
        icon: Workflow,
        blurb: 'The graph a run is a stage of, and what it has not reached.',
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
  ['/workflows', 'inference'],
  ['/runs', 'inference'],
  ['/training', 'training'],
  ['/experiments', 'training'],
  ['/evaluation', 'training'],
  ['/prompts', 'training'],
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
  return section.areas.find((area) => {
    const root = `/${area.to.split('/')[1]}`;
    return pathname === root || pathname.startsWith(`${root}/`);
  });
}
