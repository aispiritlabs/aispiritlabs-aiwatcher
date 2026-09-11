import { createFileRoute } from '@tanstack/react-router';
import { RunPage } from '@/features/observability/screens/run-detail/page';

export const Route = createFileRoute('/runs/$runId')({
  component: RunPage,
});
