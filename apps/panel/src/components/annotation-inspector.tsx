import { Link2, Trash2 } from 'lucide-react';

import type { Annotation, AttributeDef, LabelClass } from '@/api/generated/types.gen';
import { Badge, Button } from '@/components/ui/primitives';
import { classOf } from '@/lib/annotations';
import { cn } from '@/lib/utils';

/**
 * Everything about the drawing that is not the drawing: which class the next
 * shape will be, what has been drawn so far, and what the selected instance
 * says about itself.
 *
 * The attribute editor is the part that earns its place. A door is not
 * finished when its four points are down — it is finished when it says whether
 * it swings in or out and which wall it belongs to, and those are the fields
 * the model's output JSON has to carry. A tool that only draws shapes produces
 * a mask with extra steps.
 */

export function ClassPalette({
  classes,
  active,
  onPick,
  counts,
}: {
  classes: LabelClass[];
  active: string;
  onPick: (name: string) => void;
  counts: Record<string, number>;
}) {
  return (
    <div className="flex flex-col gap-1">
      {classes.map((definition, index) => (
        <button
          key={definition.name}
          type="button"
          onClick={() => onPick(definition.name)}
          className={cn(
            'flex items-center gap-2 rounded-md px-2 py-1.5 text-left text-xs transition-colors',
            definition.name === active ? 'bg-accent text-foreground' : 'hover:bg-accent/60',
          )}
        >
          <span
            className="h-3 w-3 shrink-0 rounded-sm border border-border"
            style={{ background: definition.color ?? '#94a3b8' }}
          />
          <span className="flex-1 truncate font-medium">{definition.name}</span>
          {counts[definition.name] ? (
            <span className="font-mono text-[10px] text-muted-foreground">
              {counts[definition.name]}
            </span>
          ) : null}
          {/* The first nine get a number key. Beyond that the palette is the
              only way in, which is the right trade: a hotkey nobody can
              remember is a hotkey that mislabels things. */}
          {index < 9 && (
            <kbd className="rounded border border-border px-1 font-mono text-[10px] text-muted-foreground">
              {index + 1}
            </kbd>
          )}
        </button>
      ))}
    </div>
  );
}

export function ShapeList({
  annotations,
  classes,
  selectedId,
  invalid,
  onSelect,
  onDelete,
}: {
  annotations: Annotation[];
  classes: LabelClass[];
  selectedId: string | null;
  invalid: Set<string>;
  onSelect: (id: string) => void;
  onDelete: (id: string) => void;
}) {
  if (annotations.length === 0) {
    return (
      <p className="px-2 py-6 text-center text-xs text-muted-foreground">
        Nothing drawn yet. Pick a class and start clicking.
      </p>
    );
  }
  return (
    <ul className="flex flex-col gap-0.5">
      {annotations.map((annotation) => (
        <li key={annotation.id}>
          <div
            className={cn(
              'group flex items-center gap-2 rounded-md px-2 py-1 text-xs',
              annotation.id === selectedId ? 'bg-accent' : 'hover:bg-accent/50',
              invalid.has(annotation.id) && 'text-danger',
            )}
          >
            <button
              type="button"
              onClick={() => onSelect(annotation.id)}
              className="flex flex-1 items-center gap-2 truncate text-left"
            >
              <span
                className="h-2.5 w-2.5 shrink-0 rounded-sm"
                style={{
                  background: classOf(classes, annotation.class)?.color ?? '#94a3b8',
                }}
              />
              <span className="truncate font-mono">{annotation.id}</span>
              {annotation.origin && annotation.origin !== 'human' && (
                <Badge tone="warning" className="px-1.5 py-0 text-[10px]">
                  {annotation.origin}
                </Badge>
              )}
            </button>
            <button
              type="button"
              aria-label={`Delete ${annotation.id}`}
              onClick={() => onDelete(annotation.id)}
              className="opacity-0 transition-opacity group-hover:opacity-100"
            >
              <Trash2 className="h-3.5 w-3.5 text-muted-foreground hover:text-danger" />
            </button>
          </div>
        </li>
      ))}
    </ul>
  );
}

export function ShapeInspector({
  annotation,
  classes,
  annotations,
  linking,
  onChange,
  onStartLink,
  onClearLink,
}: {
  annotation: Annotation;
  classes: LabelClass[];
  annotations: Annotation[];
  /** The link currently being picked, if any. */
  linking: string | null;
  onChange: (next: Annotation) => void;
  onStartLink: (name: string | null) => void;
  onClearLink: (name: string) => void;
}) {
  const definition = classOf(classes, annotation.class);
  if (!definition) {
    return (
      <p className="p-2 text-xs text-danger">
        {annotation.class} is not in this project&rsquo;s schema. Change the class or add it to the
        schema before saving.
      </p>
    );
  }

  const setAttribute = (name: string, value: unknown) => {
    const attributes = { ...(annotation.attributes ?? {}) };
    if (value === undefined || value === '') delete attributes[name];
    else attributes[name] = value;
    onChange({ ...annotation, attributes });
  };

  return (
    <div className="flex flex-col gap-3 p-2 text-xs">
      <div className="flex items-center gap-2">
        <span
          className="h-3 w-3 rounded-sm"
          style={{ background: definition.color ?? '#94a3b8' }}
        />
        <span className="font-mono font-medium">{annotation.id}</span>
        <Badge className="ml-auto px-1.5 py-0 text-[10px]">{definition.geometry}</Badge>
      </div>

      {(definition.attributes ?? []).map((attribute) => (
        <AttributeField
          key={attribute.name}
          attribute={attribute}
          value={annotation.attributes?.[attribute.name]}
          onChange={(value) => setAttribute(attribute.name, value)}
        />
      ))}

      {(definition.links ?? []).map((link) => {
        const targets = ((annotation.links?.[link.name] as string[] | undefined) ?? []).filter(
          (id) => annotations.some((candidate) => candidate.id === id),
        );
        const picking = linking === link.name;
        return (
          <div key={link.name} className="flex flex-col gap-1">
            <label className="flex items-center justify-between font-medium text-muted-foreground">
              {link.name}
              <span className="font-normal">
                {targets.length}/{link.max ?? 1}
                {link.min ? ' · required' : ''}
              </span>
            </label>
            <div className="flex flex-wrap items-center gap-1">
              {targets.map((id) => (
                <Badge key={id} className="gap-1 px-1.5 py-0 font-mono text-[10px]">
                  {id}
                </Badge>
              ))}
              <Button
                size="sm"
                variant={picking ? 'default' : 'outline'}
                className="h-6 px-2 text-[11px]"
                onClick={() => onStartLink(picking ? null : link.name)}
              >
                <Link2 className="h-3 w-3" />
                {picking ? 'click a shape…' : 'link'}
              </Button>
              {targets.length > 0 && (
                <Button
                  size="sm"
                  variant="ghost"
                  className="h-6 px-2 text-[11px]"
                  onClick={() => onClearLink(link.name)}
                >
                  clear
                </Button>
              )}
            </div>
          </div>
        );
      })}

      {annotation.geometry.kind === 'keypoints' && (
        <div className="flex flex-col gap-1">
          <span className="font-medium text-muted-foreground">keypoints</span>
          {annotation.geometry.points.map((keypoint, index) => (
            <label key={keypoint.name} className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={keypoint.visible ?? true}
                onChange={(event) => {
                  const geometry = annotation.geometry;
                  if (geometry.kind !== 'keypoints') return;
                  onChange({
                    ...annotation,
                    geometry: {
                      kind: 'keypoints',
                      points: geometry.points.map((point, at) =>
                        at === index ? { ...point, visible: event.target.checked } : point,
                      ),
                    },
                  });
                }}
              />
              <span className="font-mono">{keypoint.name}</span>
              <span className="ml-auto font-mono text-[10px] text-muted-foreground">
                {Math.round(keypoint.at[0] ?? 0)}, {Math.round(keypoint.at[1] ?? 0)}
              </span>
            </label>
          ))}
          <p className="text-[11px] text-muted-foreground">
            Uncheck a keypoint the plan does not show. A door drawn open past the page edge still
            has a hinge.
          </p>
        </div>
      )}

      {definition.geometry === 'bbox' && (
        <label className="flex flex-col gap-1">
          <span className="font-medium text-muted-foreground">text</span>
          <input
            value={annotation.text ?? ''}
            onChange={(event) =>
              onChange({ ...annotation, text: event.target.value || null })
            }
            placeholder="what it says"
            className="rounded-md border border-border bg-background px-2 py-1"
          />
        </label>
      )}
    </div>
  );
}

function AttributeField({
  attribute,
  value,
  onChange,
}: {
  attribute: AttributeDef;
  value: unknown;
  onChange: (value: unknown) => void;
}) {
  const label = (
    <span className="flex items-center gap-1 font-medium text-muted-foreground">
      {attribute.name}
      {attribute.required && <span className="text-danger">*</span>}
    </span>
  );

  if (attribute.kind === 'enum') {
    return (
      <label className="flex flex-col gap-1" title={attribute.description}>
        {label}
        <select
          value={typeof value === 'string' ? value : ''}
          onChange={(event) => onChange(event.target.value || undefined)}
          className="rounded-md border border-border bg-background px-2 py-1"
        >
          <option value="">—</option>
          {(attribute.values ?? []).map((option) => (
            <option key={option} value={option}>
              {option}
            </option>
          ))}
        </select>
      </label>
    );
  }
  if (attribute.kind === 'bool') {
    return (
      <label className="flex items-center gap-2" title={attribute.description}>
        <input
          type="checkbox"
          checked={value === true}
          onChange={(event) => onChange(event.target.checked)}
        />
        {label}
      </label>
    );
  }
  if (attribute.kind === 'number') {
    return (
      <label className="flex flex-col gap-1" title={attribute.description}>
        {label}
        <input
          type="number"
          step="any"
          value={typeof value === 'number' ? value : ''}
          onChange={(event) =>
            onChange(event.target.value === '' ? undefined : Number(event.target.value))
          }
          className="rounded-md border border-border bg-background px-2 py-1 font-mono"
        />
      </label>
    );
  }
  return (
    <label className="flex flex-col gap-1" title={attribute.description}>
      {label}
      <input
        value={typeof value === 'string' ? value : ''}
        onChange={(event) => onChange(event.target.value || undefined)}
        className="rounded-md border border-border bg-background px-2 py-1"
      />
    </label>
  );
}
