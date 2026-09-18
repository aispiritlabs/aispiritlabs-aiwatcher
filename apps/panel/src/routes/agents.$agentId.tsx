import { createFileRoute } from '@tanstack/react-router';
import { AgentPage } from '@/features/agents/screens/detail/page';
import { searchSchema } from '@/features/agents/screens/detail/search';

export const Route = createFileRoute('/agents/$agentId')({
  validateSearch: searchSchema,
  component: AgentPage,
});
