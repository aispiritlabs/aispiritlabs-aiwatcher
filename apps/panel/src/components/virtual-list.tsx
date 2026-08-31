import * as React from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';

import { Spinner } from '@/components/ui/primitives';
import { cn } from '@/lib/utils';

/**
 * A scrolling list that renders only what is on screen, and asks for more when
 * the bottom comes into view.
 *
 * Both halves of the explorer need this and neither can afford the naive
 * version: a run with forty thousand events would mount forty thousand rows,
 * and a tree over every span in the retention window is the same problem with a
 * different noun.
 *
 * Rows are measured rather than assumed — a message row grows when it is
 * expanded, so a fixed row height would leave the list scrolled to the wrong
 * place the moment anyone clicked something.
 */
export function VirtualList<T>({
  items,
  estimateSize = 34,
  overscan = 12,
  className,
  renderRow,
  keyOf,
  onReachEnd,
  isFetchingMore,
  footer,
  followEnd = false,
}: {
  items: T[];
  estimateSize?: number;
  overscan?: number;
  className?: string;
  renderRow: (item: T, index: number) => React.ReactNode;
  keyOf: (item: T, index: number) => string;
  /** Called when the last row is rendered. Guarded by the caller. */
  onReachEnd?: () => void;
  isFetchingMore?: boolean;
  footer?: React.ReactNode;
  /** Keep the newest row visible as items are appended. */
  followEnd?: boolean;
}) {
  const scrollRef = React.useRef<HTMLDivElement>(null);

  const virtualizer = useVirtualizer({
    count: items.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => estimateSize,
    overscan,
    getItemKey: (index) => keyOf(items[index]!, index),
  });

  const virtualRows = virtualizer.getVirtualItems();
  const lastRendered = virtualRows.at(-1)?.index;

  // Fire on render rather than on a scroll handler: the virtualizer already
  // knows what is visible, and a scroll listener would have to re-derive it.
  React.useEffect(() => {
    if (onReachEnd && lastRendered !== undefined && lastRendered >= items.length - 1) {
      onReachEnd();
    }
  }, [onReachEnd, lastRendered, items.length]);

  React.useEffect(() => {
    if (followEnd && items.length > 0) {
      virtualizer.scrollToIndex(items.length - 1, { align: 'end' });
    }
  }, [followEnd, items.length, virtualizer]);

  return (
    <div ref={scrollRef} className={cn('overflow-y-auto', className)}>
      <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
        {virtualRows.map((row) => (
          <div
            key={row.key}
            data-index={row.index}
            ref={virtualizer.measureElement}
            className="absolute left-0 top-0 w-full"
            style={{ transform: `translateY(${row.start}px)` }}
          >
            {renderRow(items[row.index]!, row.index)}
          </div>
        ))}
      </div>
      {isFetchingMore ? (
        <p className="flex items-center justify-center gap-2 py-2 text-xs text-muted-foreground">
          <Spinner />
          loading more
        </p>
      ) : null}
      {footer}
    </div>
  );
}
