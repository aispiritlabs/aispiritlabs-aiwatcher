import { createFileRoute } from '@tanstack/react-router';
import { WorkflowsPage } from '@/features/workflows/screens/overview/page';
import { searchSchema } from '@/features/workflows/screens/overview/search';

export const Route = createFileRoute('/workflows')({
  validateSearch: searchSchema,
  component: WorkflowsPage,
});
