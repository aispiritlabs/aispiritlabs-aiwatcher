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
