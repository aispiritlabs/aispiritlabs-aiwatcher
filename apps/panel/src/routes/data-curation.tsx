import { Link, Outlet, createFileRoute } from '@tanstack/react-router';

/**
 * Turning retained data into a dataset something can be trained or judged on.
 *
 * Two views, and the difference between them is how much of the job is one
 * question. **Pipeline** is the canvas: where the rows come from, what shapes
 * them, what a notebook does to them and what is published — four engines'
 * worth of work, assembled as blocks and run one at a time (ADR_0024).
 * **Recipe** is the older and smaller thing, and still the right one when the
 * whole curation is a single Flow PHP query.
 *
 * The period carries across the two and nothing else does: "this transformation
 * over the last day" is one thought, and a tab switch that dropped the window
 * would make it two.
 */

export const Route = createFileRoute('/data-curation')({
  component: DataCurationLayout,
});

const VIEWS = [
  { to: '/data-curation/pipeline', label: 'Pipeline' },
  { to: '/data-curation/recipe', label: 'Recipe' },
] as const;

function DataCurationLayout() {
  return (
    <div className="flex flex-col gap-4">
      <nav className="flex items-center gap-1 border-b border-border">
        {VIEWS.map(({ to, label }) => (
          <Link
            key={to}
            to={to}
            search={(previous: { window?: number }) =>
              previous.window === undefined ? {} : { window: previous.window }
            }
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
