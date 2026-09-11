import { createFileRoute } from '@tanstack/react-router';
import { ExportsPage } from '@/features/annotations/screens/exports/page';
import { searchSchema } from '@/features/annotations/screens/exports/search';

export const Route = createFileRoute('/annotations/exports')({
  validateSearch: searchSchema,
  component: ExportsPage,
});
