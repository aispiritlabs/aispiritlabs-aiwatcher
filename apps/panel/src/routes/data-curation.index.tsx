import { createFileRoute, redirect } from '@tanstack/react-router';

/**
 * The area's landing view.
 *
 * The canvas, not the script editor: a curation that reads a corpus, transforms
 * it and runs a notebook over it is four blocks and one script's worth of Flow,
 * and the blocks are where that is assembled. The script editor is still there
 * for a curation that is only a query — see ADR_0024.
 */
export const Route = createFileRoute('/data-curation/')({
  beforeLoad: () => {
    throw redirect({ to: '/data-curation/pipeline' });
  },
});
