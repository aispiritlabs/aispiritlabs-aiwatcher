import { createFileRoute } from '@tanstack/react-router';
import { CorporaPage } from '@/features/conversations/screens/corpora/page';
import { searchSchema } from '@/features/conversations/screens/corpora/search';

export const Route = createFileRoute('/conversations/corpora')({
  validateSearch: searchSchema,
  component: CorporaPage,
});
