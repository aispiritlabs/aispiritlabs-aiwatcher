import { createFileRoute } from '@tanstack/react-router';
import { MetricsPage } from '@/features/observability/screens/metrics/page';
import { searchSchema } from '@/features/observability/screens/metrics/search';

export const Route = createFileRoute('/observability/metrics')({
  validateSearch: searchSchema,
  component: MetricsPage,
});
