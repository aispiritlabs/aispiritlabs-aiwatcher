import { createFileRoute } from '@tanstack/react-router';
import { SystemPage } from '@/features/system/screens/overview/page';
import { searchSchema } from '@/features/system/screens/overview/search';

export const Route = createFileRoute('/system')({
  validateSearch: searchSchema,
  component: SystemPage,
});
