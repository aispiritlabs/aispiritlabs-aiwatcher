import { createFileRoute, redirect } from '@tanstack/react-router';

/** The area's landing view. The curve is what somebody came for. */
export const Route = createFileRoute('/training/')({
  beforeLoad: () => {
    throw redirect({ to: '/training/runs' });
  },
});
