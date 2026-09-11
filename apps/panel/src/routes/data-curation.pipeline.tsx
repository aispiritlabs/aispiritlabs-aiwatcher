import { createFileRoute } from '@tanstack/react-router';
import { PipelinePage } from '@/features/data-curation/screens/pipeline/page';
import { searchSchema } from '@/features/data-curation/screens/pipeline/search';

export const Route = createFileRoute('/data-curation/pipeline')({
  validateSearch: searchSchema,
  component: PipelinePage,
});
