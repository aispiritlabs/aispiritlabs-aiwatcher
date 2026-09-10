import { Outlet, createFileRoute } from '@tanstack/react-router';

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
 *
 * Its views are listed in `lib/navigation.ts` and drawn by the sidebar, so
 * this layout is a pass-through: a second tab row here would be the same
 * four links, one level in, disagreeing with the first one the day somebody
 * adds a page to only one of them.
 */

export const Route = createFileRoute('/training')({
  component: TrainingLayout,
});

function TrainingLayout() {
  return <Outlet />;
}
