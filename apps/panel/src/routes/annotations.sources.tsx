import { createFileRoute } from '@tanstack/react-router';
import { SourcesPage } from '@/features/annotations/screens/sources/page';
import { searchSchema } from '@/features/annotations/screens/sources/search';

export const Route = createFileRoute('/annotations/sources')({
  validateSearch: searchSchema,
  component: SourcesPage,
});
