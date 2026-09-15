import { Link, createRootRouteWithContext } from '@tanstack/react-router';
import type { QueryClient } from '@tanstack/react-query';
import { RootLayout } from '@/app/shell';

export const Route = createRootRouteWithContext<{ queryClient: QueryClient }>()({
  component: RootLayout,
  errorComponent: ({ reset }) => (
    <div role="alert" className="space-y-3 p-10 text-sm">
      <p>This page could not be opened. Check the link and its filters, or try again.</p>
      <button onClick={reset} className="rounded border border-border px-3 py-2">Try again</button>{' '}
      <Link to="/" search={{ start: 'workspace' }} className="text-primary underline">Your work</Link>
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
