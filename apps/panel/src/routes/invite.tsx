import { createFileRoute } from '@tanstack/react-router';
import { InvitePage } from '@/features/account/screens/invite/page';

export const Route = createFileRoute('/invite')({
  component: InvitePage,
});
