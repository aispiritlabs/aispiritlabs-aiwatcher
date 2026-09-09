import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { RouterProvider, createRouter } from '@tanstack/react-router';

import '@/lib/api';
import '@/styles.css';
import { AuthGate } from '@/components/auth-gate';
import { routeTree } from './routeTree.gen';

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      // Live data arrives over SSE, so polling would only duplicate it. Query
      // is for the initial history load and cache invalidation as new events
      // arrive.
      refetchOnWindowFocus: false,
      staleTime: 5_000,
      retry: 1,
    },
  },
});

const router = createRouter({
  routeTree,
  context: { queryClient },
  defaultPreload: 'intent',
  defaultPreloadStaleTime: 0,
  /*
   * Keep the reader where they were when the URL changes but the page does not.
   *
   * This panel puts filters, selections and view state in the URL on purpose —
   * a link to a filtered view has to land somebody on that view. The cost was
   * that every one of those is a *navigation*, and the router scrolls to the
   * top of a navigation: picking a phase, tracing a node, opening a block or
   * changing a time window all threw the reader back to the page header, on
   * every route in the application.
   *
   * Restoration is keyed by `pathname` rather than the default `href`, which
   * is the whole fix: every search-param variant of one page then shares a
   * scroll position, so changing what you are looking at keeps where you are
   * looking. Moving to a different page still starts at the top, or at
   * wherever that page was left.
   */
  scrollRestoration: true,
  getScrollRestorationKey: (location) => location.pathname,
});

declare module '@tanstack/react-router' {
  interface Register {
    router: typeof router;
  }
}

const container = document.getElementById('root');
if (!container) throw new Error('#root is missing from index.html');

createRoot(container).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      {/* Above the router, not inside it: the sign-in screen is not a page —
          there is no navigation to it and no URL for it. It is what the whole
          application looks like when nobody is signed in. */}
      <AuthGate>
        <RouterProvider router={router} />
      </AuthGate>
    </QueryClientProvider>
  </StrictMode>,
);
