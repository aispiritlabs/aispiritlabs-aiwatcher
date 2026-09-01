import { Link, Outlet, createFileRoute } from '@tanstack/react-router';

/**
 * Training: the one area here that reads nothing folded from the event log.
 *
 * A training run is a record that grows in place, not a trace — see ADR_0018.
 * Two views, and the second is the reason the first is worth keeping: **Runs**
 * is the curve, **Models** is what a run produced and which version a service
 * loads next. The join from a bad agent run back to the labelled images behind
 * its model passes through both.
 *
 * Launching a training job is still Experiments' business. Starting work and
 * watching it are different jobs done at different times, which is the same
 * split Workflows and Data Curation already make.
 */

export const Route = createFileRoute('/training')({
  component: TrainingLayout,
});

const VIEWS = [
  { to: '/training/runs', label: 'Runs' },
  { to: '/training/models', label: 'Models' },
] as const;

function TrainingLayout() {
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
