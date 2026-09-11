import { createFileRoute } from '@tanstack/react-router';
import { ConversationsLayout } from '@/features/conversations/screens/overview/page';

export const Route = createFileRoute('/conversations')({
  component: ConversationsLayout,
});
