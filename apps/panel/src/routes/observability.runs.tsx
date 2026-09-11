import { createFileRoute } from '@tanstack/react-router';
import { RunsPage } from '@/features/observability/screens/runs/page';
import { searchSchema } from '@/features/observability/screens/runs/search';

export const Route = createFileRoute('/observability/runs')({
  validateSearch: searchSchema,
  component: RunsPage,
});
