import { createFileRoute } from '@tanstack/react-router';
import { LivePage } from '@/features/observability/screens/live/page';
import { searchSchema } from '@/features/observability/screens/live/search';

export const Route = createFileRoute('/observability/live')({
  validateSearch: searchSchema,
  component: LivePage,
});
