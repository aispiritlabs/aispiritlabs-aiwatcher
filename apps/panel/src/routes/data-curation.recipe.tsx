import { createFileRoute } from '@tanstack/react-router';
import { DataCurationPage } from '@/features/data-curation/screens/recipe/page';
import { searchSchema } from '@/features/data-curation/screens/recipe/search';

export const Route = createFileRoute('/data-curation/recipe')({
  validateSearch: searchSchema,
  component: DataCurationPage,
});
