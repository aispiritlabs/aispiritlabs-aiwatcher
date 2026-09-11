import { createFileRoute } from '@tanstack/react-router';
import { ObservabilityLayout } from '@/features/observability/screens/overview/page';

export const Route = createFileRoute('/observability')({
  component: ObservabilityLayout,
});
