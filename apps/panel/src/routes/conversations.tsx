import { Outlet, createFileRoute } from '@tanstack/react-router';

/**
 * The conversation archive: the only area here that shows content, and the
 * only one where a role decides whether it is shown at all.
 *
 * Two views, in the order the work happens. **Review** is the gate — nothing
 * reaches a corpus until somebody has read it — and **Corpora** is what comes
 * out: an asynchronous export job, and the immutable `name@sha256` a training
 * run records.
 *
 * It sits beside Annotations rather than inside Datasets because it is the same
 * job on the other kind of input. An annotation export and a conversation
 * export are both authored, both outside retention, both content-addressed and
 * both reviewed before they are frozen; what differs is that this one holds
 * somebody's words, which is why it is encrypted and why it expires. See
 * ADR_0021.
 *
 * Its views are listed in `lib/navigation.ts` and drawn by the sidebar, so
 * this layout is a pass-through: a second tab row here would be the same
 * four links, one level in, disagreeing with the first one the day somebody
 * adds a page to only one of them.
 */

export const Route = createFileRoute('/conversations')({
  component: ConversationsLayout,
});

function ConversationsLayout() {
  return <Outlet />;
}
