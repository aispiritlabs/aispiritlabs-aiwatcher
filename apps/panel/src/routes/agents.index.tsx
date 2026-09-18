import { createFileRoute } from '@tanstack/react-router';
import { AgentsPage } from '@/features/agents/screens/overview/page';
import { searchSchema } from '@/features/agents/screens/overview/search';

export const Route = createFileRoute('/agents/')({
  validateSearch: searchSchema,
  component: AgentsPage,
});
