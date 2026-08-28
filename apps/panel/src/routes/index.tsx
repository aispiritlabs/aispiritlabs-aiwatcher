import { createFileRoute, redirect } from '@tanstack/react-router';

/**
 * The old root. Kept as a redirect rather than deleted: `/` is what is
 * bookmarked, what a proxy health check hits, and what someone types.
 */
export const Route = createFileRoute('/')({
  beforeLoad: () => {
    throw redirect({ to: '/observability/explore' });
  },
});
