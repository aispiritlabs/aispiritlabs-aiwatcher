import { createFileRoute } from '@tanstack/react-router';
import { TrainingLayout } from '@/features/training/screens/overview/page';

export const Route = createFileRoute('/training')({
  component: TrainingLayout,
});
