import { createFileRoute } from '@tanstack/react-router';
import { ExperimentsPage } from '@/features/experiments/screens/overview/page';
import { searchSchema } from '@/features/experiments/screens/overview/search';

export const Route = createFileRoute('/experiments')({
  validateSearch: searchSchema,
  component: ExperimentsPage,
});
