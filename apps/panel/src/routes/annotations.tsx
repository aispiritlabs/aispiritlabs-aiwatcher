import { createFileRoute } from '@tanstack/react-router';
import { AnnotationsLayout } from '@/features/annotations/screens/overview/page';

export const Route = createFileRoute('/annotations')({
  component: AnnotationsLayout,
});
