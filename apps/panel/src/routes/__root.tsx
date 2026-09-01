import { Link, Outlet, createRootRouteWithContext } from '@tanstack/react-router';
import type { QueryClient } from '@tanstack/react-query';
import {
  Activity,
  Database,
  FlaskConical,
  LineChart,
  ScrollText,
  Shapes,
  Sigma,
  Sparkles,
  WandSparkles,
  Workflow,
} from 'lucide-react';

import { UserMenu } from '@/components/user-menu';

/**
 * Nine areas, not a flat list of every page.
 *
 * The header used to list every route side by side, which worked while there
 * was one product area. There are six now — watching runs, watching the
 * pipelines those runs are stages of, judging them, keeping the prompts they
 * run on, curating what they are judged against, and changing the thing being
 * run — and they are different jobs done at different times. So the top level is the area, and the pages inside one area are its
 * own concern (see `observability.tsx`).
 */

export const Route = createRootRouteWithContext<{ queryClient: QueryClient }>()({
  component: RootLayout,
  notFoundComponent: () => (
    <div className="p-10 text-center text-sm text-muted-foreground">
      No such page.{' '}
      <Link to="/observability/explore" className="text-primary underline">
        Back to the explorer
      </Link>
      .
    </div>
  ),
});

const AREAS = [
  { to: '/observability', label: 'Observability', icon: LineChart },
  // Between watching and judging: a workflow is the level above a run, and the
  // question it answers — where is this pipeline, what has it not reached — is
  // still one about what happened rather than about whether it was any good.
  { to: '/workflows', label: 'Workflows', icon: Workflow },
  { to: '/evaluation', label: 'Evaluation', icon: FlaskConical },
  // Between judging and curating: a prompt is the thing an evaluation is
  // evidence about and the thing a dataset is used to change.
  { to: '/prompts', label: 'Prompts', icon: ScrollText },
  { to: '/datasets', label: 'Datasets', icon: Database },
  { to: '/data-curation', label: 'Data Curation', icon: WandSparkles },
  // Between curating and experimenting: an annotation export is the other kind
  // of training input — authored rather than folded, and outside retention.
  { to: '/annotations', label: 'Annotations', icon: Shapes },
  // And the thing those annotations are for. Its own area rather than a tab
  // inside Experiments, because a training run reads nothing folded from the
  // log and shares no machinery with anything above it — see ADR_0018.
  { to: '/training', label: 'Training', icon: Sigma },
  { to: '/experiments', label: 'Experiments', icon: Sparkles },
] as const;

function RootLayout() {
  return (
    <div className="min-h-screen">
      <header className="sticky top-0 z-10 border-b border-border bg-background/80 backdrop-blur">
        <div className="mx-auto flex max-w-[100rem] items-center gap-6 px-6">
          <Link
            to="/observability/explore"
            className="flex shrink-0 items-center gap-2 py-3 font-semibold"
          >
            <Activity className="h-4 w-4 text-primary" />
            aiwatcher
          </Link>
          {/* `min-w-0` and its own overflow: the areas plus the caller's chip
              are wider than a laptop at some widths, and the nav is the part
              that should scroll rather than the page. */}
          <nav className="flex min-w-0 items-center gap-1 overflow-x-auto">
            {AREAS.map(({ to, label, icon: Icon }) => (
              <Link
                key={to}
                to={to}
                // The tab is active for anything below it, so a run detail
                // three levels deep still shows which area it belongs to.
                activeOptions={{ exact: false }}
                className="flex items-center gap-1.5 border-b-2 border-transparent px-3 py-3 text-sm text-muted-foreground transition-colors hover:text-foreground [&.active]:border-primary [&.active]:text-foreground"
              >
                <Icon className="h-3.5 w-3.5" />
                {label}
              </Link>
            ))}
          </nav>
          <UserMenu />
        </div>
      </header>
      <main className="mx-auto max-w-[100rem] px-6 py-6">
        <Outlet />
      </main>
    </div>
  );
}
