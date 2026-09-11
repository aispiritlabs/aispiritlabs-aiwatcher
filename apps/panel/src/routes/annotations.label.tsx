import { createFileRoute } from '@tanstack/react-router';
import { LabelPage } from '@/features/annotations/screens/label/page';
import { searchSchema } from '@/features/annotations/screens/label/search';

export const Route = createFileRoute('/annotations/label')({
  validateSearch: searchSchema,
  component: LabelPage,
});
