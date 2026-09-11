import { createFileRoute } from '@tanstack/react-router';
import { DatasetsPage } from '@/features/datasets/screens/overview/page';
import { searchSchema } from '@/features/datasets/screens/overview/search';

export const Route = createFileRoute('/datasets')({
  validateSearch: searchSchema,
  component: DatasetsPage,
});
