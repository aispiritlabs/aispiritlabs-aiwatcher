import { Outlet } from '@tanstack/react-router';

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
 *
 * Its views are listed in `app/navigation.ts` and drawn by the sidebar, so
 * this layout is a pass-through: a second tab row here would be the same
 * four links, one level in, disagreeing with the first one the day somebody
 * adds a page to only one of them.
 */

export function AnnotationsLayout() {
  return <Outlet />;
}
