import { createFileRoute } from '@tanstack/react-router';
import { ReviewPage } from '@/features/conversations/screens/review/page';
import { searchSchema } from '@/features/conversations/screens/review/search';

export const Route = createFileRoute('/conversations/review')({
  validateSearch: searchSchema,
  component: ReviewPage,
});
