import { useQueryClient } from '@tanstack/react-query';
import { Outlet } from '@tanstack/react-router';
import * as React from 'react';

import { publishObservabilityRevision } from '@/features/observability/lib/observability-revision';
import { StreamBadge } from '@/shared/components/status-badge';
import { openSystemStream, type StreamPhase } from '@/shared/lib/live';

/**
 * The observability area: everything about runs that already happened or are
 * happening now.
 *
 * Its views are listed in `app/navigation.ts` and drawn by the sidebar; what
 * this layout owns is the thing they share, which is the stream. One
 * subscription for the whole area rather than one per view: every tab under it
 * reads the same read model, and a connection opened and dropped on each tab
 * switch would replay history every time.
 *
 * The Live view opens a *second*, filtered stream of its own. That is not a
 * duplicate of this one — this one exists to invalidate queries and carries
 * every event in the system for that purpose, and narrowing it to whatever the
 * Live view is watching would stop the other four tabs refreshing.
 */

export function ObservabilityLayout() {
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
      {/* The badge alone, right-aligned: it is the one thing the sidebar
          cannot say, because whether the connection is live is not a property
          of which page you are on. */}
      <div className="flex justify-end">
        <StreamBadge phase={phase} />
      </div>
      <Outlet />
    </div>
  );
}
