import { createFileRoute } from '@tanstack/react-router';
import { AccountLayout } from '@/features/account/screens/layout';

export const Route = createFileRoute('/account')({ component: AccountLayout });
