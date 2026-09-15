import { createFileRoute } from '@tanstack/react-router';
import { LearningPage } from '@/features/learning/screens/overview/page';
import { searchSchema } from '@/features/learning/screens/overview/search';

export const Route = createFileRoute('/learning')({ component: LearningPage, validateSearch: searchSchema });
