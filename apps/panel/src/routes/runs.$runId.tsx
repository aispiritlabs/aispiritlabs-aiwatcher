import { createFileRoute } from '@tanstack/react-router';
import { RunPage } from '@/features/observability/screens/run-detail/page';
import { searchSchema } from '@/features/observability/screens/run-detail/search';

export const Route = createFileRoute('/runs/$runId')({
  validateSearch: searchSchema,
  component: RunPage,
});
