import { createFileRoute, redirect } from '@tanstack/react-router';

/** The area's landing view. Review is the default; it is where the work is. */
export const Route = createFileRoute('/conversations/')({
  beforeLoad: () => {
    throw redirect({ to: '/conversations/review' });
  },
});
