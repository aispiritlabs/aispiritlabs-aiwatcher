import { createFileRoute } from '@tanstack/react-router';
import { ImportsPage } from '@/features/annotations/screens/imports/page';
import { searchSchema } from '@/features/annotations/screens/imports/search';

export const Route = createFileRoute('/annotations/imports')({
  validateSearch: searchSchema,
  component: ImportsPage,
});
