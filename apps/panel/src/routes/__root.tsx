import { Link, createRootRouteWithContext } from '@tanstack/react-router';
import type { QueryClient } from '@tanstack/react-query';
import { RootLayout } from '@/app/shell';

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
