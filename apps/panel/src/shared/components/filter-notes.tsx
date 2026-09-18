import type { Unapplied } from '@/shared/lib/object-filter';

/**
 * What the filter on screen did, and what this view could not do with it.
 *
 * Two kinds of sentence, and neither is decoration. The first is the only
 * honest answer to a filter that selects runs while some of the numbers beside
 * it count something smaller than a run — a model narrows the LLM half and
 * leaves the tool half alone, and a reader who is not told that is reading two
 * populations as one. The second is the Live view's rule generalised: a view
 * that cannot apply an axis names it, because a filter that is on the screen
 * and not on the request is the one failure a filter must not have.
 *
 * Both come from `queryFor`, beside the request they describe, rather than
 * being written per page — a page repeating a rule from memory is a page free
 * to keep saying it after the route stops doing it.
 */
export function FilterNotes({ notes, unapplied }: { notes: string[]; unapplied: Unapplied[] }) {
  if (notes.length === 0 && unapplied.length === 0) return null;

  return (
    <div className="flex flex-col gap-1">
      {notes.map((note) => (
        <p key={note} className="text-xs text-muted-foreground">
          {note}
        </p>
      ))}
      {unapplied.length > 0 ? (
        <p className="text-xs text-warning">
          Not applied here:{' '}
          {unapplied.map((entry, index) => (
            <span key={entry.axis}>
              {index > 0 ? '; ' : ''}
              <span className="font-medium">{entry.axis}</span> — {entry.why}
            </span>
          ))}
          .
        </p>
      ) : null}
    </div>
  );
}
