import { createFileRoute } from '@tanstack/react-router';
import { PromptsPage } from '@/features/prompts/screens/list/page';
import { searchSchema } from '@/features/prompts/screens/list/search';

export const Route = createFileRoute('/prompts/')({
  validateSearch: searchSchema,
  component: PromptsPage,
});
