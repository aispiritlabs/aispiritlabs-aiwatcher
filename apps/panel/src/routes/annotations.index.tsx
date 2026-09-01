import { createFileRoute, redirect } from '@tanstack/react-router';

/** The area's landing view. Labelling is the default; it is where the work is. */
export const Route = createFileRoute('/annotations/')({
  beforeLoad: () => {
    throw redirect({ to: '/annotations/label' });
  },
});
