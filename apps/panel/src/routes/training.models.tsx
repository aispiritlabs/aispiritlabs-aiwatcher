import { createFileRoute } from '@tanstack/react-router';
import { ModelsPage } from '@/features/training/screens/models/page';
import { searchSchema } from '@/features/training/screens/models/search';

export const Route = createFileRoute('/training/models')({
  validateSearch: searchSchema,
  component: ModelsPage,
});
