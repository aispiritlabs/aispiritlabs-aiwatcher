import { useBlocker, type ShouldBlockFn } from '@tanstack/react-router';

type Options = {
  dirty: boolean;
  pending?: boolean;
  message: string;
  losesDraft?: (navigation: Parameters<ShouldBlockFn>[0]) => boolean;
};

/** Keep local replacements and router/history navigation behind the same decision.
 * The browser owns focus, Escape and the separate reload/close confirmation.
 * Confirming a replacement never marks the draft saved: a failed import must
 * still leave the original draft protected.
 */
export function useUnsavedChanges({
  dirty,
  pending = false,
  message,
  losesDraft = ({ current, next }) => current.pathname !== next.pathname,
}: Options) {
  const confirmDiscard = () => {
    if (!dirty && !pending) return true;
    const progress = pending
      ? 'Work is still in progress. Leaving does not cancel requests already sent.\n\n'
      : '';
    return window.confirm(
      `${message}\n\n${progress}Discard local changes and continue? Choose Cancel to keep editing.`,
    );
  };

  useBlocker({
    shouldBlockFn: (navigation) =>
      (dirty || pending) && losesDraft(navigation) && !confirmDiscard(),
    enableBeforeUnload: dirty || pending,
  });

  return confirmDiscard;
}
