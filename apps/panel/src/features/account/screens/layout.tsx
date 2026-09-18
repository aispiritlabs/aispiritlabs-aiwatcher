import { Link, Outlet, useRouterState } from '@tanstack/react-router';

import { cn } from '@/shared/lib/utils';

/**
 * Two views about the same subject: you, and what you may reach.
 *
 * The tab row is local rather than in `app/navigation.ts` because this is not
 * one of the work areas — it hangs off the header's account menu, and putting
 * it in the sidebar would make administering grants look like a place people
 * work rather than somewhere they go on purpose.
 */
const VIEWS = [
  { to: '/account', label: 'Profile' },
  { to: '/account/access', label: 'Organizations & projects' },
] as const;

export function AccountLayout() {
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  return (
    <div className="flex flex-col gap-4">
      <nav aria-label="Account" className="flex gap-1 border-b border-border">
        {VIEWS.map((view) => (
          <Link
            key={view.to}
            to={view.to}
            className={cn(
              'rounded-t-md px-3 py-2 text-sm text-muted-foreground transition-colors hover:text-foreground',
              pathname === view.to && 'border-b-2 border-primary text-foreground',
            )}
          >
            {view.label}
          </Link>
        ))}
      </nav>
      <Outlet />
    </div>
  );
}
