import { Link, Outlet, createFileRoute } from '@tanstack/react-router';

/**
 * The annotation area: where the data a vision model is trained on is made.
 *
 * Four views, in the order the work happens. **Label** is where a plan is
 * drawn on; **Sources** is where the images come from, including the public
 * corpora and what their licences permit; **Imports** is a corpus arriving —
 * staged in pages, read by a job, with every refused row named; **Exports** is
 * the immutable manifest a training run names.
 *
 * It sits between Datasets and Experiments in the navigation for a reason: a
 * curated dataset is rows folded out of the log, and an annotation export is
 * the other kind of training input — authored, outside retention, and the
 * thing an experiment is actually run against. See ADR_0017.
 */

export const Route = createFileRoute('/annotations')({
  component: AnnotationsLayout,
});

const VIEWS = [
  { to: '/annotations/label', label: 'Label' },
  { to: '/annotations/sources', label: 'Sources' },
  { to: '/annotations/imports', label: 'Imports' },
  { to: '/annotations/exports', label: 'Exports' },
] as const;

function AnnotationsLayout() {
  return (
    <div className="flex flex-col gap-4">
      <nav className="flex items-center gap-1 border-b border-border">
        {VIEWS.map(({ to, label }) => (
          <Link
            key={to}
            to={to}
            // The project carries across the sub-navigation and nothing else
            // does: "label this, now export it" is one thought, and a tab
            // switch that dropped the project would make it two.
            search={(previous: { project?: string }) =>
              previous.project === undefined ? {} : { project: previous.project }
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
