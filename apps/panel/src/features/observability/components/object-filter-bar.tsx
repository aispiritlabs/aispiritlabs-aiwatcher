import { Filter, X } from 'lucide-react';
import * as React from 'react';

import {
  AttributePicker,
  type AttributeSelection,
} from '@/features/observability/components/attribute-picker';
import { ATTRIBUTES, attribute, type AttributeId } from '@/features/observability/lib/query-builder';
import { FilterNotes } from '@/shared/components/filter-notes';
import {
  OBJECT_AXES,
  type ObjectAxis,
  type ObjectFilter,
  type Unapplied,
} from '@/shared/lib/object-filter';
import { Badge, Button, Card } from '@/shared/components/ui/primitives';
import { cn, shortId } from '@/shared/lib/utils';

/**
 * The shared object filter, as a control.
 *
 * The same picker the Live view and the Query builder use, over the same
 * values the read model has actually seen — reading it twice for one page's
 * two halves would be two lists free to disagree about what has run. What is
 * different here is the shape: those two views give a filter a sidebar,
 * because filtering *is* what somebody is doing there. A table and a chart are
 * read with the filter mostly settled, so it collapses to a row of chips and
 * opens on request.
 *
 * `variant` is a chip and never a picker row: the query builder has no column
 * for it (`query-builder.ts`), so the one place it can be chosen is a link
 * from the explorer's variant pivot. A filter that arrived on a link is still
 * shown, still removable, and still sent — the picker offering no way back to
 * it is a gap in the picker, not permission to drop it.
 */
export function ObjectFilterBar({
  filter,
  onChange,
  windowSeconds,
  unapplied,
  notes,
  /** One clause saying what this page's numbers are, for the picker's header. */
  reading,
}: {
  filter: ObjectFilter;
  onChange: (next: ObjectFilter) => void;
  windowSeconds: number | undefined;
  unapplied: Unapplied[];
  notes: string[];
  reading: string;
}) {
  const [open, setOpen] = React.useState(false);
  const chosen = React.useMemo(
    () =>
      OBJECT_AXES.flatMap((axis) =>
        (filter[axis] ?? []).map((value) => ({ axis, value }) as const),
      ),
    [filter],
  );
  const blocked = React.useMemo(
    () => new Map(unapplied.map((entry) => [entry.axis, entry.why] as const)),
    [unapplied],
  );

  const remove = (axis: ObjectAxis, value: string) =>
    onChange({ ...filter, [axis]: (filter[axis] ?? []).filter((held) => held !== value) });

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <Button
          size="sm"
          variant={open ? 'default' : 'outline'}
          aria-expanded={open}
          onClick={() => setOpen((value) => !value)}
        >
          <Filter className="h-3 w-3" />
          Filter
          {chosen.length > 0 ? (
            <Badge tone="primary" className="px-1.5 py-0 text-[10px]">
              {chosen.length}
            </Badge>
          ) : null}
        </Button>

        {chosen.map(({ axis, value }) => (
          <button
            key={`${axis}:${value}`}
            type="button"
            onClick={() => remove(axis, value)}
            aria-label={`Remove the ${axis} filter ${value}`}
            title={`${axis}: ${value}`}
            className={cn(
              'inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-xs transition-colors',
              blocked.has(axis)
                ? 'border-warning/50 text-warning hover:bg-warning/10'
                : 'border-primary/50 text-foreground hover:bg-accent',
            )}
          >
            <span className="text-muted-foreground">{axis}</span>
            <span className="max-w-[12rem] truncate">{shortId(value, 24)}</span>
            <X className="h-3 w-3 shrink-0" />
          </button>
        ))}

        {chosen.length > 0 ? (
          <Button size="sm" variant="ghost" onClick={() => onChange({})}>
            Clear
          </Button>
        ) : null}
      </div>

      {open ? (
        <Card className="max-h-[22rem] overflow-y-auto">
          <p className="border-b border-border/40 px-2 py-1.5 text-[11px] leading-relaxed text-muted-foreground">
            Values are what has run in the period. Choosing one selects the runs that have it;{' '}
            {reading}
          </p>
          <AttributePicker
            attributes={ATTRIBUTES}
            value={filter as AttributeSelection}
            onChange={(next) => onChange({ ...filter, ...(next as ObjectFilter) })}
            windowSeconds={windowSeconds}
            unavailable={(candidate) => blocked.get(candidate.id as ObjectAxis)}
          />
        </Card>
      ) : null}

      <FilterNotes notes={notes} unapplied={unapplied} />
    </div>
  );
}

/** An axis's label as the picker spells it, for prose outside the picker. */
export function axisLabel(axis: ObjectAxis): string {
  return attribute(axis as AttributeId)?.label ?? axis;
}
