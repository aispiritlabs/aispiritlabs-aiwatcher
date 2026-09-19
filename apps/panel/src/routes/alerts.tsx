import { createFileRoute } from '@tanstack/react-router';
import { AlertsPage } from '@/features/alerts/screens/overview/page';
import { searchSchema } from '@/features/alerts/screens/overview/search';

export const Route = createFileRoute('/alerts')({
  validateSearch: searchSchema,
  component: AlertsPage,
});
