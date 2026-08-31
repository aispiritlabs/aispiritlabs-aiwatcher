import * as React from 'react';
import { Link, Outlet, createFileRoute } from '@tanstack/react-router';
import { useQueryClient } from '@tanstack/react-query';

import { StreamBadge } from '@/components/status-badge';
import { openSystemStream, type StreamPhase } from '@/lib/live';
import { publishObservabilityRevision } from '@/lib/observability-revision';

/**
 * The observability area: everything about runs that already happened or are
 * happening now.
 *
 * Four views of one dataset rather than four features. The explorer is the
 * default because it is the one you arrive at with a question; metrics is the
 * one you arrive at without one; the runs table is the flat fallback when you
 * already know the run id.
 */

export const Route = createFileRoute('/observability')({
  component: ObservabilityLayout,
});

const VIEWS = [
  { to: '/observability/explore', label: 'Explore' },
  { to: '/observability/metrics', label: 'Metrics' },
  { to: '/observability/runs', label: 'Runs' },
  { to: '/observability/query', label: 'Query' },
] as const;

function ObservabilityLayout() {
  const queryClient = useQueryClient();
  const [phase, setPhase] = React.useState<StreamPhase>('catching-up');

  React.useEffect(() => {
    const changedRuns = new Set<string>();
    let refreshTimer: ReturnType<typeof setTimeout> | undefined;

    const flush = () => {
      refreshTimer = undefined;
      const runIds = [...changedRuns];
      changedRuns.clear();

      // Prefix invalidation updates whichever Observability tab is active.
      // The short delay folds a burst of token chunks into one read-model
      // refresh while the event feed itself remains a true live tail.
      const refreshes = [
        queryClient.invalidateQueries({ queryKey: ['dimensions'] }),
        queryClient.invalidateQueries({ queryKey: ['spans'] }),
        queryClient.invalidateQueries({ queryKey: ['runs'] }),
        queryClient.invalidateQueries({ queryKey: ['metrics'] }),
        ...runIds.flatMap((runId) => [
          queryClient.invalidateQueries({ queryKey: ['events', runId] }),
          queryClient.invalidateQueries({ queryKey: ['run', runId] }),
        ]),
      ];
      void Promise.all(refreshes);
      publishObservabilityRevision();
    };

    const close = openSystemStream({
      onEvent: (event) => {
        changedRuns.add(event.run_id);
        if (!refreshTimer) refreshTimer = setTimeout(flush, 500);
      },
      onPhase: setPhase,
    });

    return () => {
      if (refreshTimer) clearTimeout(refreshTimer);
      close();
    };
  }, [queryClient]);

  return (
    <div className="flex flex-col gap-4">
      <nav className="flex items-center justify-between gap-3 border-b border-border">
        <div className="flex items-center gap-1">
          {VIEWS.map(({ to, label }) => (
            <Link
              key={to}
              to={to}
              // The period carries across the sub-navigation; nothing else does.
              // Having narrowed to the last fifteen minutes, "now show me the
              // metrics for it" is the next question, and a tab switch that
              // silently reset the window would answer a different one. The rest
              // of the search — the pivot, the open run, a query — belongs to the
              // view that owns it.
              search={(previous: { window?: number }) =>
                previous.window === undefined ? {} : { window: previous.window }
              }
              className="-mb-px border-b-2 border-transparent px-3 py-2 text-sm text-muted-foreground transition-colors hover:text-foreground [&.active]:border-primary [&.active]:text-foreground"
            >
              {label}
            </Link>
          ))}
        </div>
        <StreamBadge phase={phase} />
      </nav>
      <Outlet />
    </div>
  );
}
