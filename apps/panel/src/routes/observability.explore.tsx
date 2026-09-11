import { createFileRoute } from '@tanstack/react-router';
import { ExplorePage } from '@/features/observability/screens/explore/page';
import { searchSchema } from '@/features/observability/screens/explore/search';

export const Route = createFileRoute('/observability/explore')({
  validateSearch: searchSchema,
  component: ExplorePage,
});
