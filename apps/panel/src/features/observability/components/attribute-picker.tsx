import * as React from 'react';
import { useQuery } from '@tanstack/react-query';
import { Check, ChevronDown, ChevronRight, Search, X } from 'lucide-react';

import { listDimension } from '@/api/generated/sdk.gen';
import { Badge, Spinner } from '@/shared/components/ui/primitives';
import { windowParam } from '@/shared/components/time-range';
import { ATTRIBUTES, type Attribute, type AttributeId } from '@/features/observability/lib/query-builder';
import { cn, formatCount, isStalled, shortId } from '@/shared/lib/utils';

/**
 * The attributes a question can be asked about, and the values actually seen.
 *
 * Clicking spares somebody the columns' trivia: a run carries `agents` as a
 * list, a span calls the same thing `agent_id`, and every column is nullable so
 * the comparison has to be `same`. The values come from the read model, so each
 * row has actually run, with its run count and whether any of those runs is
 * working now — which also says whether the thing about to be filtered for is
 * there at all.
 *
 * One attribute's values are fetched when it is opened and searched on the
 * server (`/api/v1/dimensions/{kind}`, a cursor page): ADR_0007's rule for every
 * list.
 */

export type AttributeSelection = Partial<Record<AttributeId, string[]>>;

export function AttributePicker({
  attributes,
  value,
  onChange,
  windowSeconds,
  /** An attribute this grain cannot express is shown greyed with the reason. */
  unavailable,
}: {
  attributes: Attribute[];
  value: AttributeSelection;
  onChange: (next: AttributeSelection) => void;
  windowSeconds: number | undefined;
  unavailable?: (attribute: Attribute) => string | undefined;
}) {
  const toggle = React.useCallback(
    (id: AttributeId, item: string) => {
      const current = value[id] ?? [];
      const next = current.includes(item)
        ? current.filter((candidate) => candidate !== item)
        : [...current, item];
      onChange({ ...value, [id]: next });
    },
    [onChange, value],
  );

  return (
    <div className="flex flex-col">
      {attributes.map((attribute) => (
        <AttributeGroup
          key={attribute.id}
          attribute={attribute}
          chosen={value[attribute.id] ?? []}
          onToggle={(item) => toggle(attribute.id, item)}
          onClear={() => onChange({ ...value, [attribute.id]: [] })}
          windowSeconds={windowSeconds}
          blocked={unavailable?.(attribute)}
        />
      ))}
    </div>
  );
}

/** The attributes offered by default: every one the builder knows. */
export const ALL_ATTRIBUTES = ATTRIBUTES;

function AttributeGroup({
  attribute,
  chosen,
  onToggle,
  onClear,
  windowSeconds,
  blocked,
}: {
  attribute: Attribute;
  chosen: string[];
  onToggle: (value: string) => void;
  onClear: () => void;
  windowSeconds: number | undefined;
  blocked: string | undefined;
}) {
  // Open when something in it is chosen, so a filter arriving from a link is
  // visible rather than hidden one click deep.
  const [open, setOpen] = React.useState(chosen.length > 0);
  const [draft, setDraft] = React.useState('');
  const [search, setSearch] = React.useState('');

  React.useEffect(() => {
    const timer = setTimeout(() => setSearch(draft), 250);
    return () => clearTimeout(timer);
  }, [draft]);

  return (
    <div className="border-b border-border/40 last:border-b-0">
      <div className="flex items-center">
        <button
          type="button"
          onClick={() => setOpen((value) => !value)}
          className={cn(
            'flex flex-1 items-center gap-1.5 px-2 py-1.5 text-left text-sm hover:bg-accent/40',
            blocked && 'opacity-50',
          )}
        >
          {open ? (
            <ChevronDown className="h-3 w-3 shrink-0 text-muted-foreground" />
          ) : (
            <ChevronRight className="h-3 w-3 shrink-0 text-muted-foreground" />
          )}
          <span className="font-medium">{attribute.label}</span>
          {chosen.length > 0 ? (
            <Badge tone="primary" className="px-1.5 py-0 text-[10px]">
              {chosen.length}
            </Badge>
          ) : null}
        </button>
        {chosen.length > 0 ? (
          <button
            type="button"
            onClick={onClear}
            aria-label={`Clear the ${attribute.label} filter`}
            className="px-2 py-1.5 text-muted-foreground hover:text-foreground"
          >
            <X className="h-3 w-3" />
          </button>
        ) : null}
      </div>

      {open ? (
        <div className="pb-2">
          {blocked ? (
            // Never a bare "unavailable": the reason is the whole content of
            // the message, and it says which grain does answer it.
            <p className="px-3 pb-1 text-[11px] leading-relaxed text-muted-foreground">{blocked}</p>
          ) : null}

          {attribute.values ? (
            <FixedValues values={attribute.values} chosen={chosen} onToggle={onToggle} />
          ) : (
            <>
              <div className="mx-2 mb-1 flex items-center gap-1.5 rounded-md border border-border px-2 py-1">
                <Search className="h-3 w-3 shrink-0 text-muted-foreground" />
                <input
                  value={draft}
                  onChange={(event) => setDraft(event.target.value)}
                  placeholder={`Find a ${attribute.label.toLowerCase()}…`}
                  className="w-full bg-transparent text-xs outline-none placeholder:text-muted-foreground"
                />
              </div>
              <ObservedValues
                attribute={attribute}
                chosen={chosen}
                onToggle={onToggle}
                search={search}
                windowSeconds={windowSeconds}
              />
            </>
          )}
        </div>
      ) : null}
    </div>
  );
}

function FixedValues({
  values,
  chosen,
  onToggle,
}: {
  values: string[];
  chosen: string[];
  onToggle: (value: string) => void;
}) {
  return (
    <div className="flex flex-wrap gap-1 px-2">
      {values.map((value) => (
        <ValueChip
          key={value}
          label={value}
          on={chosen.includes(value)}
          onClick={() => onToggle(value)}
        />
      ))}
    </div>
  );
}

function ObservedValues({
  attribute,
  chosen,
  onToggle,
  search,
  windowSeconds,
}: {
  attribute: Attribute;
  chosen: string[];
  onToggle: (value: string) => void;
  search: string;
  windowSeconds: number | undefined;
}) {
  const kind = attribute.dimension;
  const rows = useQuery({
    queryKey: ['dimensions', kind, { search, window: windowSeconds, picker: true }],
    enabled: kind !== undefined,
    queryFn: async () => {
      const response = await listDimension({
        path: { kind: kind! },
        query: {
          window_seconds: windowParam(windowSeconds),
          search: search || undefined,
          // A picker is a shortlist. Somebody looking past thirty rows is
          // looking for one in particular, and that is what the search box is.
          limit: 30,
        },
        throwOnError: true,
      });
      return response.data;
    },
  });

  if (rows.isPending) {
    return (
      <p className="flex items-center gap-2 px-3 py-1 text-xs text-muted-foreground">
        <Spinner /> Reading what has run…
      </p>
    );
  }

  if (rows.isError) {
    return (
      <p className="px-3 py-1 text-xs text-danger">
        Could not read the {attribute.label.toLowerCase()} list.
      </p>
    );
  }

  const page = rows.data;
  if (!page || page.rows.length === 0) {
    return (
      <p className="px-3 py-1 text-[11px] leading-relaxed text-muted-foreground">
        {search
          ? `Nothing matching "${search}" in the period.`
          : `No ${attribute.label.toLowerCase()} has been seen in the period. Widen it, or pick a longer window.`}
      </p>
    );
  }

  return (
    <div className="flex flex-col gap-0.5 px-2">
      {page.rows.map((row) => (
        <button
          key={row.key}
          type="button"
          onClick={() => onToggle(row.key)}
          className={cn(
            'flex items-center gap-2 rounded-md px-1.5 py-1 text-left text-xs transition-colors hover:bg-accent/50',
            chosen.includes(row.key) && 'bg-accent',
          )}
        >
          <span
            className={cn(
              'flex h-3 w-3 shrink-0 items-center justify-center rounded-[3px] border',
              chosen.includes(row.key)
                ? 'border-primary bg-primary text-primary-foreground'
                : 'border-border',
            )}
          >
            {chosen.includes(row.key) ? <Check className="h-2.5 w-2.5" /> : null}
          </span>
          <span className="truncate" title={row.key}>
            {shortId(row.key, 28)}
          </span>
          <span className="ml-auto shrink-0 tabular-nums text-muted-foreground">
            {formatCount(row.runs)}
          </span>
          {/* A dot for a row that is *working*, not merely one whose count of
              running runs is above zero: a killed producer keeps that count up
              for ever, and a permanent green dot is worse than none. Same
              fifteen minutes the explorer and the span assembler use. */}
          {row.running > 0 && !isStalled(row.running_last_event_at) ? (
            <span
              className="h-1.5 w-1.5 shrink-0 rounded-full bg-success"
              title="Something is running"
            />
          ) : null}
        </button>
      ))}
      {page.next_cursor ? (
        <p className="px-1.5 pt-1 text-[10px] text-muted-foreground">
          {formatCount(page.total)} in the period — search to reach the rest.
        </p>
      ) : null}
    </div>
  );
}

function ValueChip({ label, on, onClick }: { label: string; on: boolean; onClick: () => void }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        'rounded-full border px-2 py-0.5 text-xs transition-colors',
        on
          ? 'border-primary bg-primary/10 text-foreground'
          : 'border-border text-muted-foreground hover:text-foreground',
      )}
    >
      {label}
    </button>
  );
}
