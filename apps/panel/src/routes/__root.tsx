import { Link, createRootRouteWithContext, retainSearchParams } from '@tanstack/react-router';
import type { QueryClient } from '@tanstack/react-query';
import { z } from 'zod';

import { RootLayout } from '@/app/shell';
import { activeScope, formatScope, parseScope, setActiveScope } from '@/shared/lib/scope';

/**
 * The one search parameter that belongs to no view.
 *
 * Every other parameter in this panel is a view's own — a filter, a window, a
 * selected object — and is dropped when the reader moves elsewhere. A scope is
 * not a filter: it decides which rows exist at all, so it belongs above every
 * area and survives every move between them. `NavArea.carries` is the same
 * idea one level down, and this is that idea made global.
 */
const rootSearch = z.object({
  /** `<organization>/<project>`, or absent for the unassigned side. */
  scope: z.string().optional(),
});

export const Route = createRootRouteWithContext<{ queryClient: QueryClient }>()({
  component: RootLayout,
  validateSearch: rootSearch,
  search: { middlewares: [retainSearchParams(['scope'])] },
  /**
   * Point the transport at the URL's scope before anything reads.
   *
   * `beforeLoad` on the root runs ahead of every loader and every render on
   * every navigation, which is the ordering this needs: a deep link carrying a
   * scope must not get one instance-wide answer in first. And when the scope
   * changes, the cache goes with it — react-query keys are a view's own and
   * name no project, so keeping them would hand the next project the previous
   * one's rows.
   */
  beforeLoad: ({ search, context }) => {
    const next = parseScope(search.scope);
    const current = activeScope();
    const changed = (current && formatScope(current)) !== (next && formatScope(next));
    if (!changed) return;
    setActiveScope(next);
    context.queryClient.clear();
  },
  errorComponent: ({ reset }) => (
    <div role="alert" className="space-y-3 p-10 text-sm">
      <p>This page could not be opened. Check the link and its filters, or try again.</p>
      <button onClick={reset} className="rounded border border-border px-3 py-2">
        Try again
      </button>{' '}
      <Link to="/" search={{ start: 'workspace' }} className="text-primary underline">
        Your work
      </Link>
    </div>
  ),
  notFoundComponent: () => (
    <div className="p-10 text-center text-sm text-muted-foreground">
      No such page.{' '}
      <Link to="/" search={{ start: 'workspace' }} className="text-primary underline">
        Back to your work
      </Link>
      .
    </div>
  ),
});
