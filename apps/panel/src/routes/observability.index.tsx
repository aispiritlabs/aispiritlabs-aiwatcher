import { createFileRoute, redirect } from '@tanstack/react-router';

/** The area's landing view. Nothing renders here; the explorer is the default. */
export const Route = createFileRoute('/observability/')({
  beforeLoad: () => {
    throw redirect({ to: '/observability/explore' });
  },
});
