import { Outlet } from '@tanstack/react-router';

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
 *
 * Its views are listed in `app/navigation.ts` and drawn by the sidebar, so
 * this layout is a pass-through: a second tab row here would be the same
 * four links, one level in, disagreeing with the first one the day somebody
 * adds a page to only one of them.
 */

export function DataCurationLayout() {
  return <Outlet />;
}
