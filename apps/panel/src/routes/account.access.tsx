import { createFileRoute } from '@tanstack/react-router';
import { AccessPage } from '@/features/account/screens/access/page';
import { searchSchema } from '@/features/account/screens/access/search';

export const Route = createFileRoute('/account/access')({
  validateSearch: searchSchema,
  component: AccessPage,
});
