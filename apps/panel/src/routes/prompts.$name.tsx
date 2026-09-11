import { createFileRoute } from '@tanstack/react-router';
import { PromptPage } from '@/features/prompts/screens/detail/page';
import { searchSchema } from '@/features/prompts/screens/detail/search';

export const Route = createFileRoute('/prompts/$name')({
  validateSearch: searchSchema,
  component: PromptPage,
});
