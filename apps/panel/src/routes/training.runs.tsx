import { createFileRoute } from '@tanstack/react-router';
import { RunsPage } from '@/features/training/screens/runs/page';
import { searchSchema } from '@/features/training/screens/runs/search';

export const Route = createFileRoute('/training/runs')({
  validateSearch: searchSchema,
  component: RunsPage,
});
