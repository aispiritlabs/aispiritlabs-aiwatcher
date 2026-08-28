import { Link, Outlet, createFileRoute } from '@tanstack/react-router';

/**
 * The observability area: everything about runs that already happened or are
 * happening now.
 *
 * Three views of one dataset rather than three features. The explorer is the
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
  return (
    <div className="flex flex-col gap-4">
      <nav className="flex items-center gap-1 border-b border-border">
        {VIEWS.map(({ to, label }) => (
          <Link
            key={to}
            to={to}
            className="-mb-px border-b-2 border-transparent px-3 py-2 text-sm text-muted-foreground transition-colors hover:text-foreground [&.active]:border-primary [&.active]:text-foreground"
          >
            {label}
          </Link>
        ))}
      </nav>
      <Outlet />
    </div>
  );
}
