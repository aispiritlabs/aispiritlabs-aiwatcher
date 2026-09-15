import { createFileRoute } from '@tanstack/react-router';
import { ProfilePage } from '@/features/account/screens/profile/page';

export const Route = createFileRoute('/account')({ component: ProfilePage });
