import * as React from 'react';

import type { Annotation, Geometry, LabelClass } from '@/api/generated/types.gen';
import {
  boundsOf,
  classColor,
  classOf,
  hitTest,
  movePoint,
  pointsOf,
  translate,
  type Point,
} from '@/lib/annotations';
import { cn } from '@/lib/utils';

/**
 * The drawing surface.
 *
 * An `<img>` and an `<svg>` in one transformed container, both sized to the
 * image's natural pixels. That is the whole trick: SVG user units *are* image
 * coordinates, so no shape ever carries a zoom level and the browser does the
 * scaling. The alternative — converting on every render — is where an
 * annotation tool acquires its off-by-a-pixel drift.
 *
 * Interaction is deliberately close to CVAT's, because that is what a labeller
 * who has done this before will try:
 *
 * * wheel zooms at the cursor, shift-drag pans, and so does a drag on empty
 *   canvas while the select tool is active;
 * * a draw tool takes clicks; `Enter` finishes, `Backspace` removes the last
 *   point, `Escape` abandons the shape;
 * * a selected shape shows its vertices, which drag; the shape itself drags
 *   with `alt` held, so a stray drag does not silently move a wall.
 *
 * What it does not do is decide anything. Validation lives in the registry, so
 * a half-drawn door is refused with a reason rather than quietly fixed here
 * into something the labeller did not draw.
 */

export type Tool = 'select' | 'draw';

export interface CanvasProps {
  src: string;
  width: number;
  height: number;
  classes: LabelClass[];
  annotations: Annotation[];
  tool: Tool;
  /** The class a draw tool produces. */
  activeClass: string;
  selectedId: string | null;
  /** Ids to draw as the target of the link being picked, if any. */
  linkTargets?: string[];
  onSelect: (id: string | null) => void;
  onChange: (id: string, geometry: Geometry) => void;
  onCreate: (geometry: Geometry) => void;
  /** Ids that failed validation, drawn in the danger colour. */
  invalid?: string[];
  className?: string;
}

interface View {
  scale: number;
  x: number;
  y: number;
}

const MIN_SCALE = 0.05;
const MAX_SCALE = 16;
/** How close a click has to land, in screen pixels, before it counts as a hit. */
const HIT_TOLERANCE = 8;

export function AnnotationCanvas({
  src,
  width,
  height,
  classes,
  annotations,
  tool,
  activeClass,
  selectedId,
  linkTargets,
  onSelect,
  onChange,
  onCreate,
  invalid,
  className,
}: CanvasProps) {
  const container = React.useRef<HTMLDivElement>(null);
  const [view, setView] = React.useState<View>({ scale: 1, x: 0, y: 0 });
  const [draft, setDraft] = React.useState<Point[]>([]);
  const [cursor, setCursor] = React.useState<Point | null>(null);
  const [drag, setDrag] = React.useState<
    | { kind: 'pan'; from: Point; view: View }
    | { kind: 'vertex'; id: string; index: number }
    | { kind: 'shape'; id: string; from: Point }
    | { kind: 'bbox'; from: Point }
    | null
  >(null);

  const definition = classOf(classes, activeClass);
  const geometryKind = definition?.geometry ?? 'polygon';
  const invalidIds = React.useMemo(() => new Set(invalid ?? []), [invalid]);
  const linkIds = React.useMemo(() => new Set(linkTargets ?? []), [linkTargets]);

  // Fit the plan to the pane, and re-fit until the pane has a real size.
  //
  // A `getBoundingClientRect` on the first paint routinely reports a fraction
  // of the eventual width — the grid has not laid out yet — and fitting to
  // that leaves the plan at 8% in the corner, which reads as a broken image.
  // A `ResizeObserver` is the only thing that reliably knows when the pane is
  // done; it stops fitting as soon as somebody has zoomed or panned, so it
  // never fights the person using it.
  const fitted = React.useRef(false);
  React.useEffect(() => {
    fitted.current = false;
    setDraft([]);
  }, [src]);

  React.useEffect(() => {
    const element = container.current;
    if (!element) return;
    const fit = () => {
      if (fitted.current) return;
      const box = element.getBoundingClientRect();
      if (box.width < 8 || box.height < 8) return;
      const scale = Math.min(box.width / width, box.height / height) * 0.95;
      fitted.current = true;
      setView({
        scale,
        x: (box.width - width * scale) / 2,
        y: (box.height - height * scale) / 2,
      });
    };
    fit();
    const observer = new ResizeObserver(fit);
    observer.observe(element);
    return () => observer.disconnect();
  }, [src, width, height]);

  const toImage = React.useCallback(
    (event: { clientX: number; clientY: number }): Point => {
      const box = container.current?.getBoundingClientRect();
      if (!box) return [0, 0];
      return [
        (event.clientX - box.left - view.x) / view.scale,
        (event.clientY - box.top - view.y) / view.scale,
      ];
    },
    [view],
  );

  const zoomAt = React.useCallback((at: Point, factor: number) => {
    fitted.current = true;
    setView((previous) => {
      const scale = Math.min(MAX_SCALE, Math.max(MIN_SCALE, previous.scale * factor));
      const applied = scale / previous.scale;
      return {
        scale,
        // Keep the image point under the cursor under the cursor. Zooming to
        // the centre instead is what makes people zoom out to find their place
        // again after every step.
        x: at[0] - (at[0] - previous.x) * applied,
        y: at[1] - (at[1] - previous.y) * applied,
      };
    });
  }, []);

  React.useEffect(() => {
    const element = container.current;
    if (!element) return;
    // Non-passive, because the page must not scroll while the plan zooms.
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      const box = element.getBoundingClientRect();
      zoomAt(
        [event.clientX - box.left, event.clientY - box.top],
        Math.exp(-event.deltaY * 0.0015),
      );
    };
    element.addEventListener('wheel', onWheel, { passive: false });
    return () => element.removeEventListener('wheel', onWheel);
  }, [zoomAt]);

  const finishDraft = React.useCallback(
    (points: Point[]) => {
      setDraft([]);
      if (points.length === 0) return;
      switch (geometryKind) {
        case 'point':
          if (points[0]) onCreate({ kind: 'point', at: points[0] });
          return;
        case 'polyline':
          if (points.length >= 2) onCreate({ kind: 'polyline', points });
          return;
        case 'polygon':
          if (points.length >= 3) onCreate({ kind: 'polygon', exterior: points, holes: [] });
          return;
        case 'keypoints': {
          const names = definition?.keypoints ?? [];
          onCreate({
            kind: 'keypoints',
            points: points.slice(0, names.length).map((at, index) => ({
              name: names[index] ?? `point_${index}`,
              at,
              visible: true,
            })),
          });
          return;
        }
        case 'bbox':
          return;
      }
    },
    [definition, geometryKind, onCreate],
  );

  const onPointerDown = (event: React.PointerEvent) => {
    if (event.button === 1 || event.shiftKey) {
      event.currentTarget.setPointerCapture(event.pointerId);
      setDrag({
        kind: 'pan',
        from: [event.clientX, event.clientY],
        view,
      });
      return;
    }
    const at = toImage(event);
    const tolerance = HIT_TOLERANCE / view.scale;

    if (tool === 'draw') {
      if (geometryKind === 'bbox') {
        event.currentTarget.setPointerCapture(event.pointerId);
        setDrag({ kind: 'bbox', from: at });
        setDraft([at, at]);
        return;
      }
      const next = [...draft, at];
      // A point instance and a keypoint set both finish on their own: one
      // click, or one per declared keypoint.
      const target =
        geometryKind === 'point' ? 1 : geometryKind === 'keypoints' ? (definition?.keypoints?.length ?? 0) : 0;
      if (target > 0 && next.length >= target) {
        finishDraft(next);
      } else {
        setDraft(next);
      }
      return;
    }

    // Select: a vertex of the selected shape first, then any shape, then pan.
    const selected = annotations.find((annotation) => annotation.id === selectedId);
    if (selected) {
      const index = pointsOf(selected.geometry).findIndex(
        (point) => Math.hypot(point[0] - at[0], point[1] - at[1]) <= tolerance,
      );
      if (index >= 0) {
        event.currentTarget.setPointerCapture(event.pointerId);
        setDrag({ kind: 'vertex', id: selected.id, index });
        return;
      }
    }
    const hit = [...annotations]
      .reverse()
      .find((annotation) => hitTest(annotation.geometry, at, tolerance));
    if (hit) {
      onSelect(hit.id);
      if (event.altKey) {
        event.currentTarget.setPointerCapture(event.pointerId);
        setDrag({ kind: 'shape', id: hit.id, from: at });
      }
      return;
    }
    onSelect(null);
    event.currentTarget.setPointerCapture(event.pointerId);
    setDrag({ kind: 'pan', from: [event.clientX, event.clientY], view });
  };

  const onPointerMove = (event: React.PointerEvent) => {
    const at = toImage(event);
    setCursor(at);
    if (!drag) return;
    switch (drag.kind) {
      case 'pan':
        setView({
          scale: drag.view.scale,
          x: drag.view.x + (event.clientX - drag.from[0]),
          y: drag.view.y + (event.clientY - drag.from[1]),
        });
        return;
      case 'bbox':
        setDraft([drag.from, at]);
        return;
      case 'vertex': {
        const annotation = annotations.find((entry) => entry.id === drag.id);
        if (annotation) onChange(drag.id, movePoint(annotation.geometry, drag.index, at));
        return;
      }
      case 'shape': {
        const annotation = annotations.find((entry) => entry.id === drag.id);
        if (annotation) {
          onChange(drag.id, translate(annotation.geometry, at[0] - drag.from[0], at[1] - drag.from[1]));
          setDrag({ ...drag, from: at });
        }
        return;
      }
    }
  };

  const onPointerUp = () => {
    if (drag?.kind === 'bbox') {
      const [from, to] = draft;
      setDraft([]);
      if (from && to && Math.abs(to[0] - from[0]) > 2 && Math.abs(to[1] - from[1]) > 2) {
        onCreate({
          kind: 'bbox',
          min: [Math.min(from[0], to[0]), Math.min(from[1], to[1])],
          max: [Math.max(from[0], to[0]), Math.max(from[1], to[1])],
        });
      }
    }
    setDrag(null);
  };

  // On the window rather than on the element, and only while a shape is being
  // drawn. A labeller who has just clicked a class in the palette no longer has
  // the canvas focused, and `Enter` doing nothing at that moment is the bug
  // that makes somebody think the tool is stuck. Typing in a field is excluded
  // so `Backspace` still deletes a character.
  React.useEffect(() => {
    if (draft.length === 0) return;
    const onKey = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (target && ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName)) return;
      if (event.key === 'Enter') {
        event.preventDefault();
        finishDraft(draft);
      } else if (event.key === 'Escape') {
        setDraft([]);
      } else if (event.key === 'Backspace') {
        event.preventDefault();
        setDraft(draft.slice(0, -1));
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [draft, finishDraft]);

  const nextKeypoint =
    tool === 'draw' && geometryKind === 'keypoints'
      ? definition?.keypoints?.[draft.length]
      : undefined;

  return (
    <div
      ref={container}
      tabIndex={0}
      role="application"
      aria-label="Annotation canvas"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerLeave={() => setCursor(null)}
      onDoubleClick={() => draft.length > 0 && finishDraft(draft)}
      onContextMenu={(event) => event.preventDefault()}
      className={cn(
        'relative select-none overflow-hidden rounded-lg border border-border bg-muted/40 outline-none focus-visible:ring-2 focus-visible:ring-primary',
        tool === 'draw' ? 'cursor-crosshair' : 'cursor-grab',
        className,
      )}
    >
      <div
        className="absolute left-0 top-0 origin-top-left"
        style={{
          transform: `translate(${view.x}px, ${view.y}px) scale(${view.scale})`,
          width,
          height,
        }}
      >
        <img
          src={src}
          alt=""
          width={width}
          height={height}
          draggable={false}
          className="pointer-events-none block"
        />
        <svg
          viewBox={`0 0 ${width} ${height}`}
          width={width}
          height={height}
          className="pointer-events-none absolute left-0 top-0"
        >
          {annotations.map((annotation) => (
            <Shape
              key={annotation.id}
              annotation={annotation}
              classes={classes}
              selected={annotation.id === selectedId}
              invalid={invalidIds.has(annotation.id)}
              linkTarget={linkIds.has(annotation.id)}
              scale={view.scale}
            />
          ))}
          {draft.length > 0 && (
            <DraftShape
              points={draft}
              cursor={cursor}
              kind={geometryKind}
              color={classColor(classes, activeClass)}
              scale={view.scale}
            />
          )}
        </svg>
      </div>

      <div className="pointer-events-none absolute bottom-2 left-2 flex gap-2 text-[11px]">
        <span className="rounded bg-background/80 px-2 py-1 font-mono text-muted-foreground shadow-sm">
          {Math.round(view.scale * 100)}%
        </span>
        {cursor && (
          <span className="rounded bg-background/80 px-2 py-1 font-mono text-muted-foreground shadow-sm">
            {Math.round(cursor[0])}, {Math.round(cursor[1])}
          </span>
        )}
        {nextKeypoint && (
          <span className="rounded bg-primary/15 px-2 py-1 font-medium text-primary shadow-sm">
            place {nextKeypoint}
          </span>
        )}
      </div>
    </div>
  );
}

function Shape({
  annotation,
  classes,
  selected,
  invalid,
  linkTarget,
  scale,
}: {
  annotation: Annotation;
  classes: LabelClass[];
  selected: boolean;
  invalid: boolean;
  linkTarget: boolean;
  scale: number;
}) {
  const definition = classOf(classes, annotation.class);
  const color = invalid ? '#ef4444' : classColor(classes, annotation.class);
  // Stroke and vertex sizes divide by the zoom so a line stays one screen
  // pixel wide at 25% and at 400%. Without this the whole plan is covered in
  // ink the moment somebody zooms out.
  const stroke = (selected ? 2.5 : 1.5) / scale;
  const radius = 4 / scale;
  // A model's proposal is drawn dashed. It is the fastest way to see that a
  // page of labels is a page of predictions nobody has checked.
  const dash = annotation.origin === 'human' ? undefined : `${6 / scale} ${4 / scale}`;
  const fill = definition?.ignore ? `${color}33` : `${color}1f`;
  const points = pointsOf(annotation.geometry);

  return (
    <g opacity={linkTarget ? 1 : undefined}>
      {linkTarget && (
        <Outline
          geometry={annotation.geometry}
          color="#eab308"
          stroke={stroke * 3}
          fill="none"
          dash={undefined}
        />
      )}
      <Outline
        geometry={annotation.geometry}
        color={color}
        stroke={stroke}
        fill={fill}
        dash={dash}
      />
      {selected &&
        points.map((point, index) => (
          <circle
            key={index}
            cx={point[0]}
            cy={point[1]}
            r={radius}
            fill="#fff"
            stroke={color}
            strokeWidth={stroke}
          />
        ))}
      {selected && annotation.geometry.kind === 'keypoints' && (
        <>
          {annotation.geometry.points.map((keypoint) => (
            <text
              key={keypoint.name}
              x={(keypoint.at as Point)[0] + radius * 1.5}
              y={(keypoint.at as Point)[1] - radius}
              fontSize={11 / scale}
              fill={color}
            >
              {keypoint.name}
            </text>
          ))}
        </>
      )}
    </g>
  );
}

function Outline({
  geometry,
  color,
  stroke,
  fill,
  dash,
}: {
  geometry: Geometry;
  color: string;
  stroke: number;
  fill: string;
  dash: string | undefined;
}) {
  const common = {
    stroke: color,
    strokeWidth: stroke,
    strokeDasharray: dash,
    strokeLinejoin: 'round' as const,
    strokeLinecap: 'round' as const,
  };
  switch (geometry.kind) {
    case 'point': {
      const [x, y] = geometry.at as Point;
      return <circle cx={x} cy={y} r={stroke * 3} fill={color} {...common} />;
    }
    case 'bbox': {
      const [x, y] = geometry.min as Point;
      const [x2, y2] = geometry.max as Point;
      return <rect x={x} y={y} width={x2 - x} height={y2 - y} fill={fill} {...common} />;
    }
    case 'polyline':
      return (
        <polyline
          points={(geometry.points as Point[]).map((point) => point.join(',')).join(' ')}
          fill="none"
          {...common}
        />
      );
    case 'polygon':
      return (
        <>
          <polygon
            points={(geometry.exterior as Point[]).map((point) => point.join(',')).join(' ')}
            fill={fill}
            {...common}
          />
          {((geometry.holes ?? []) as Point[][]).map((hole, index) => (
            <polygon
              key={index}
              points={hole.map((point) => point.join(',')).join(' ')}
              fill="var(--color-background)"
              {...common}
            />
          ))}
        </>
      );
    case 'keypoints': {
      const points = geometry.points.map((keypoint) => keypoint.at as Point);
      return (
        <>
          {points.length > 1 && (
            <polyline points={points.map((point) => point.join(',')).join(' ')} fill="none" {...common} />
          )}
          {points.map((point, index) => (
            <circle
              key={index}
              cx={point[0]}
              cy={point[1]}
              r={stroke * 2.5}
              fill={color}
              {...common}
            />
          ))}
        </>
      );
    }
  }
}

/** The shape being drawn, with a rubber band to the cursor. */
function DraftShape({
  points,
  cursor,
  kind,
  color,
  scale,
}: {
  points: Point[];
  cursor: Point | null;
  kind: LabelClass['geometry'];
  color: string;
  scale: number;
}) {
  const stroke = 2 / scale;
  if (kind === 'bbox') {
    const [from, to] = points;
    if (!from || !to) return null;
    return (
      <rect
        x={Math.min(from[0], to[0])}
        y={Math.min(from[1], to[1])}
        width={Math.abs(to[0] - from[0])}
        height={Math.abs(to[1] - from[1])}
        fill={`${color}1f`}
        stroke={color}
        strokeWidth={stroke}
        strokeDasharray={`${6 / scale} ${4 / scale}`}
      />
    );
  }
  const chain = cursor ? [...points, cursor] : points;
  return (
    <>
      <polyline
        points={chain.map((point) => point.join(',')).join(' ')}
        fill={kind === 'polygon' ? `${color}14` : 'none'}
        stroke={color}
        strokeWidth={stroke}
        strokeDasharray={`${6 / scale} ${4 / scale}`}
      />
      {points.map((point, index) => (
        <circle key={index} cx={point[0]} cy={point[1]} r={4 / scale} fill={color} />
      ))}
    </>
  );
}

/** Where a class label is drawn for a shape. */
export function labelAnchor(geometry: Geometry): Point {
  const [x, y] = boundsOf(geometry);
  return [x, y];
}
