import { createFileRoute } from '@tanstack/react-router';
import { QueryPage } from '@/features/observability/screens/query/page';
import { searchSchema } from '@/features/observability/screens/query/search';

export const Route = createFileRoute('/observability/query')({
  validateSearch: searchSchema,
  component: QueryPage,
});
