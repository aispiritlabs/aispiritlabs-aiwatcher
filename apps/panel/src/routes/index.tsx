import { createFileRoute } from '@tanstack/react-router';
import { WorkspacePage } from '@/app/workspace';

export const Route = createFileRoute('/')({
  validateSearch: (search: Record<string, unknown>): { start?: 'workspace' } =>
    search.start === 'workspace' ? { start: 'workspace' } : {},
  component: WorkspacePage,
});
