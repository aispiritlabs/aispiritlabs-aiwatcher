import * as React from 'react';
import { useNavigate } from '@tanstack/react-router';
import {
  FloatingFocusManager,
  FloatingOverlay,
  FloatingPortal,
  useDismiss,
  useFloating,
  useInteractions,
  useRole,
} from '@floating-ui/react';
import { CornerDownLeft, Search } from 'lucide-react';

import { type Command, allCommands, search } from '@/app/commands';
import { cn } from '@/shared/lib/utils';

/**
 * The command panel: type what you want, land on the page with it applied.
 *
 * It lives in `app/` rather than in `shared/` because it has to know both the
 * navigation and where each feature's filters live, and `shared` may depend on
 * neither — `scripts/check-architecture.mjs` enforces that, and it is right to.
 *
 * **It navigates and does nothing else.** Every command is a route and a
 * search object ([`commands.ts`] says why that is enough), so running one goes
 * through the router like a link and the back button undoes it. Nothing here
 * mutates, and nothing here holds state a reload would lose.
 *
 * **`@floating-ui/react` for the modal, not for positioning.** A centred
 * palette needs no anchor; what it needs is the part of that library which is
 * not about coordinates — a portal out of the header's stacking context, an
 * overlay that locks the background scroll, a focus trap that returns focus to
 * whatever opened it, and dismissal on Escape and outside press with the
 * listeners removed again on close. Writing those four by hand is where a
 * dialog goes subtly wrong, and this is the panel's first one: CLAUDE.md's
 * Panel section said Radix would arrive with it, and this is that decision
 * settled the other way.
 */
export function CommandPanel({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const navigate = useNavigate();
  const [query, setQuery] = React.useState('');
  const [active, setActive] = React.useState(0);
  const listRef = React.useRef<HTMLUListElement>(null);

  const commands = React.useMemo(() => allCommands(), []);
  const matches = React.useMemo(() => search(commands, query), [commands, query]);

  // A fresh panel every time it opens. Reopening onto the last query would
  // make the first keystroke a correction rather than a search.
  React.useEffect(() => {
    if (open) {
      setQuery('');
      setActive(0);
    }
  }, [open]);

  // The highlight follows the list, never the other way round: a query that
  // narrows to two rows must not leave the selection pointing at the seventh.
  React.useEffect(() => setActive(0), [query]);

  const { refs, context } = useFloating({ open, onOpenChange });
  const { getFloatingProps } = useInteractions([
    useDismiss(context, { outsidePress: true, escapeKey: true }),
    useRole(context, { role: 'dialog' }),
  ]);

  const run = React.useCallback(
    (command: Command) => {
      onOpenChange(false);
      // `search` replaces rather than merges: a command is a whole question,
      // and carrying the previous page's filters into it would answer a
      // different one. The router's own types are wider than this registry, so
      // the cast is where that is admitted rather than hidden.
      void navigate({ to: command.to, search: (command.search ?? {}) as never } as never);
    },
    [navigate, onOpenChange],
  );

  function onKeyDown(event: React.KeyboardEvent) {
    if (event.key === 'ArrowDown' || (event.key === 'n' && event.ctrlKey)) {
      event.preventDefault();
      setActive((at) => (matches.length === 0 ? 0 : (at + 1) % matches.length));
    } else if (event.key === 'ArrowUp' || (event.key === 'p' && event.ctrlKey)) {
      event.preventDefault();
      setActive((at) => (matches.length === 0 ? 0 : (at - 1 + matches.length) % matches.length));
    } else if (event.key === 'Enter') {
      event.preventDefault();
      const chosen = matches[active];
      if (chosen) run(chosen);
    }
  }

  // Keep the highlighted row visible without scrolling the page: the list is
  // long and arrowing past the fold is the ordinary way to reach a command.
  React.useEffect(() => {
    listRef.current?.querySelector('[data-active="true"]')?.scrollIntoView({ block: 'nearest' });
  }, [active, matches]);

  if (!open) return null;

  return (
    <FloatingPortal>
      <FloatingOverlay lockScroll className="z-50 bg-black/40 backdrop-blur-sm">
        <FloatingFocusManager context={context} initialFocus={0} returnFocus>
          <div
            ref={refs.setFloating}
            aria-label="Command panel"
            className="mx-auto mt-[12vh] w-[min(40rem,calc(100vw-2rem))] overflow-hidden rounded-lg border border-border bg-background shadow-2xl"
            {...getFloatingProps({ onKeyDown })}
          >
            <div className="flex items-center gap-2 border-b border-border px-3">
              <Search className="h-4 w-4 shrink-0 text-muted-foreground" />
              <input
                autoFocus
                aria-label="Type a command"
                aria-controls="command-panel-list"
                aria-activedescendant={
                  matches[active] ? `command-${matches[active].id}` : undefined
                }
                placeholder="Show traces for live agents…"
                className="w-full bg-transparent py-3 text-sm outline-none placeholder:text-muted-foreground"
                value={query}
                onChange={(event) => setQuery(event.target.value)}
              />
            </div>

            {matches.length === 0 ? (
              // Named rather than an empty box: what somebody typed is the one
              // piece of information that makes "nothing" readable.
              <p className="px-4 py-6 text-sm text-muted-foreground">
                Nothing here matches “{query}”.
              </p>
            ) : (
              <ul
                ref={listRef}
                id="command-panel-list"
                role="listbox"
                aria-label="Commands"
                className="max-h-[50vh] overflow-y-auto py-1"
              >
                {matches.map((command, index) => (
                  <li key={command.id}>
                    <button
                      type="button"
                      id={`command-${command.id}`}
                      role="option"
                      aria-selected={index === active}
                      data-active={index === active}
                      // Pointer and keyboard agree on one highlight, so
                      // Enter runs what the mouse is over.
                      onPointerMove={() => setActive(index)}
                      onClick={() => run(command)}
                      className={cn(
                        'flex w-full items-start gap-3 px-4 py-2 text-left',
                        index === active && 'bg-muted',
                      )}
                    >
                      {command.icon ? (
                        <command.icon className="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground" />
                      ) : (
                        <span className="mt-0.5 h-4 w-4 shrink-0" />
                      )}
                      <span className="min-w-0 flex-1">
                        <span className="flex items-baseline gap-2">
                          <span className="truncate text-sm">{command.label}</span>
                          <span className="shrink-0 text-xs text-muted-foreground">
                            {command.group}
                          </span>
                        </span>
                        {command.hint ? (
                          <span className="block truncate text-xs text-muted-foreground">
                            {command.hint}
                          </span>
                        ) : null}
                      </span>
                      {index === active ? (
                        <CornerDownLeft className="mt-0.5 h-3 w-3 shrink-0 text-muted-foreground" />
                      ) : null}
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </FloatingFocusManager>
      </FloatingOverlay>
    </FloatingPortal>
  );
}

/**
 * ⌘K, or Ctrl-K where there is no ⌘.
 *
 * Bound on the document rather than on a container, because the point of the
 * shortcut is that it works wherever the reader's focus happens to be. It
 * stays out of the way of a reader who is typing: a field with focus keeps the
 * keystroke, except for the shortcut itself, which is what somebody presses to
 * leave the field they are in.
 */
export function useCommandPanel(): [boolean, (open: boolean) => void] {
  const [open, setOpen] = React.useState(false);

  React.useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key.toLowerCase() === 'k' && (event.metaKey || event.ctrlKey)) {
        event.preventDefault();
        setOpen((value) => !value);
      }
    }
    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, []);

  return [open, setOpen];
}
