import { Link, Outlet, createFileRoute } from '@tanstack/react-router';

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
 */

export const Route = createFileRoute('/conversations')({
  component: ConversationsLayout,
});

const VIEWS = [
  { to: '/conversations/review', label: 'Review' },
  { to: '/conversations/corpora', label: 'Corpora' },
] as const;

function ConversationsLayout() {
  return (
    <div className="flex flex-col gap-4">
      <nav className="flex items-center gap-1 border-b border-border">
        {VIEWS.map(({ to, label }) => (
          <Link
            key={to}
            to={to}
            className="-mb-px border-b-2 border-transparent px-3 py-2 text-sm text-muted-foreground transition-colors hover:text-foreground [&.active]:border-primary [&.active]:text-foreground"
          >
            {label}
          </Link>
        ))}
      </nav>
      <Outlet />
    </div>
  );
}
