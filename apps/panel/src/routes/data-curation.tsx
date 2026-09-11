import { createFileRoute } from '@tanstack/react-router';
import { DataCurationLayout } from '@/features/data-curation/screens/overview/page';

export const Route = createFileRoute('/data-curation')({
  component: DataCurationLayout,
});
