import { createFileRoute } from '@tanstack/react-router';
import { EvaluationPage } from '@/features/evaluation/screens/overview/page';
import { searchSchema } from '@/features/evaluation/screens/overview/search';

export const Route = createFileRoute('/evaluation')({
  validateSearch: searchSchema,
  component: EvaluationPage,
});
