import type {
  Annotation,
  Geometry,
  LabelClass,
  ReviewState,
  Split,
} from '@/api/generated/types.gen';

/**
 * The vector side of the annotation tool: coordinates, defaults and the small
 * decisions that would otherwise be re-made in three components.
 *
 * Everything here works in **original image pixels**. The canvas transform is
 * applied by the browser, so a shape never carries a zoom level and a label
 * drawn at 400% is the same label drawn at 25%. Normalised coordinates would
 * have been the other option, and they bake a preprocessing decision into the
 * label that nobody can recover later.
 */

export type Point = [number, number];

/** The colour a class draws in, with a sane fallback for an unknown one. */
export function classColor(classes: LabelClass[], name: string): string {
  return classes.find((entry) => entry.name === name)?.color ?? '#94a3b8';
}

export function classOf(classes: LabelClass[], name: string): LabelClass | undefined {
  return classes.find((entry) => entry.name === name);
}

/** Every point of a shape, in drawing order. */
export function pointsOf(geometry: Geometry): Point[] {
  switch (geometry.kind) {
    case 'point':
      return [geometry.at as Point];
    case 'bbox':
      return [geometry.min as Point, geometry.max as Point];
    case 'polyline':
      return geometry.points as Point[];
    case 'polygon':
      return [...(geometry.exterior as Point[]), ...((geometry.holes ?? []) as Point[][]).flat()];
    case 'keypoints':
      return geometry.points.map((keypoint) => keypoint.at as Point);
  }
}

/** `[x, y, width, height]` around a shape, for a label anchor or a hit test. */
export function boundsOf(geometry: Geometry): [number, number, number, number] {
  const points = pointsOf(geometry);
  const first = points[0];
  if (!first) return [0, 0, 0, 0];
  let [minX, minY] = first;
  let [maxX, maxY] = first;
  for (const [x, y] of points) {
    minX = Math.min(minX, x);
    minY = Math.min(minY, y);
    maxX = Math.max(maxX, x);
    maxY = Math.max(maxY, y);
  }
  return [minX, minY, maxX - minX, maxY - minY];
}

/**
 * Whether a click at `at` lands on a shape.
 *
 * Distance-based rather than fill-based on purpose: a wall centreline and a
 * dimension line have no area at all, and a polygon's interior is usually
 * covered by the plan the labeller is trying to see. `tolerance` arrives in
 * image pixels, so it is the screen tolerance divided by the zoom — a
 * hairline is as easy to grab at 25% as at 400%.
 */
export function hitTest(geometry: Geometry, at: Point, tolerance: number): boolean {
  const points = pointsOf(geometry);
  if (points.some((point) => distance(point, at) <= tolerance)) return true;

  switch (geometry.kind) {
    case 'point':
      return false;
    case 'bbox': {
      const [min, max] = [geometry.min as Point, geometry.max as Point];
      return (
        at[0] >= min[0] - tolerance &&
        at[0] <= max[0] + tolerance &&
        at[1] >= min[1] - tolerance &&
        at[1] <= max[1] + tolerance
      );
    }
    case 'keypoints':
      return false;
    case 'polyline':
      return nearAnySegment(points, at, tolerance, false);
    case 'polygon':
      return (
        nearAnySegment(geometry.exterior as Point[], at, tolerance, true) ||
        insideRing(geometry.exterior as Point[], at)
      );
  }
}

function nearAnySegment(ring: Point[], at: Point, tolerance: number, closed: boolean): boolean {
  const limit = closed ? ring.length : ring.length - 1;
  for (let index = 0; index < limit; index += 1) {
    const start = ring[index];
    const end = ring[(index + 1) % ring.length];
    if (start && end && distanceToSegment(at, start, end) <= tolerance) return true;
  }
  return false;
}

function insideRing(ring: Point[], at: Point): boolean {
  let inside = false;
  for (let i = 0, j = ring.length - 1; i < ring.length; j = i, i += 1) {
    const a = ring[i];
    const b = ring[j];
    if (!a || !b) continue;
    const straddles = a[1] > at[1] !== b[1] > at[1];
    if (straddles && at[0] < ((b[0] - a[0]) * (at[1] - a[1])) / (b[1] - a[1]) + a[0]) {
      inside = !inside;
    }
  }
  return inside;
}

function distance(a: Point, b: Point): number {
  return Math.hypot(a[0] - b[0], a[1] - b[1]);
}

function distanceToSegment(point: Point, start: Point, end: Point): number {
  const dx = end[0] - start[0];
  const dy = end[1] - start[1];
  const lengthSquared = dx * dx + dy * dy;
  if (lengthSquared === 0) return distance(point, start);
  const t = Math.max(
    0,
    Math.min(1, ((point[0] - start[0]) * dx + (point[1] - start[1]) * dy) / lengthSquared),
  );
  return distance(point, [start[0] + t * dx, start[1] + t * dy]);
}

/** Replace the nth point of a shape, keeping its kind. */
export function movePoint(geometry: Geometry, index: number, to: Point): Geometry {
  switch (geometry.kind) {
    case 'point':
      return { kind: 'point', at: to };
    case 'bbox': {
      const min = (index === 0 ? to : geometry.min) as Point;
      const max = (index === 0 ? geometry.max : to) as Point;
      // Dragging a corner past its opposite would produce a negative width,
      // which the API refuses. Normalising here means the drag simply flips.
      return {
        kind: 'bbox',
        min: [Math.min(min[0], max[0]), Math.min(min[1], max[1])],
        max: [Math.max(min[0], max[0]), Math.max(min[1], max[1])],
      };
    }
    case 'polyline':
      return {
        kind: 'polyline',
        points: (geometry.points as Point[]).map((point, at) => (at === index ? to : point)),
      };
    case 'polygon':
      return {
        ...geometry,
        exterior: (geometry.exterior as Point[]).map((point, at) => (at === index ? to : point)),
      };
    case 'keypoints':
      return {
        kind: 'keypoints',
        points: geometry.points.map((keypoint, at) =>
          at === index ? { ...keypoint, at: to } : keypoint,
        ),
      };
  }
}

/** Shift every point of a shape, for dragging a whole instance. */
export function translate(geometry: Geometry, dx: number, dy: number): Geometry {
  const shift = ([x, y]: Point): Point => [x + dx, y + dy];
  switch (geometry.kind) {
    case 'point':
      return { kind: 'point', at: shift(geometry.at as Point) };
    case 'bbox':
      return {
        kind: 'bbox',
        min: shift(geometry.min as Point),
        max: shift(geometry.max as Point),
      };
    case 'polyline':
      return { kind: 'polyline', points: (geometry.points as Point[]).map(shift) };
    case 'polygon':
      return {
        kind: 'polygon',
        exterior: (geometry.exterior as Point[]).map(shift),
        holes: ((geometry.holes ?? []) as Point[][]).map((hole) => hole.map(shift)),
      };
    case 'keypoints':
      return {
        kind: 'keypoints',
        points: geometry.points.map((keypoint) => ({
          ...keypoint,
          at: shift(keypoint.at as Point),
        })),
      };
  }
}

/**
 * The attributes a new instance starts with.
 *
 * Every declared default is filled in, which is what makes `role: unknown` and
 * `door_type: hinged` the cost-free answer and a considered one the deliberate
 * edit. A required attribute with no default is left out, so the save fails
 * loudly rather than inventing a value.
 */
export function defaultAttributes(definition: LabelClass): Record<string, unknown> {
  const attributes: Record<string, unknown> = {};
  for (const attribute of definition.attributes ?? []) {
    if (attribute.default !== undefined && attribute.default !== null) {
      attributes[attribute.name] = attribute.default;
    }
  }
  return attributes;
}

/** `wall_7`, unique within the revision. Ids are the labeller's, not the server's. */
export function nextId(existing: Annotation[], className: string): string {
  const prefix = className.replaceAll(/[^a-z0-9]+/gi, '_');
  let index = existing.length + 1;
  const taken = new Set(existing.map((annotation) => annotation.id));
  while (taken.has(`${prefix}_${index}`)) index += 1;
  return `${prefix}_${index}`;
}

/**
 * The problems a refused save carries.
 *
 * The API answers 422 with one line per problem rather than 400 with the
 * first, because a drawing can be wrong in nine ways at once and fixing them
 * one round trip at a time is how somebody stops using the tool.
 */
export function rejectionDetails(error: unknown): string[] {
  const body = error as { code?: string; message?: string; details?: string[] } | null | undefined;
  if (!body) return [];
  if (body.details?.length) return body.details;
  return body.message ? [body.message] : [];
}

/**
 * Whether two drawings are the same drawing.
 *
 * A `JSON.stringify` comparison of the two is wrong in both directions, which
 * is what left the "unsaved" marker lit after a save that had already
 * succeeded. The registry omits `holes`, `attributes` and `links` when they
 * are empty, so a revision read back never matches the draft it was made
 * from; and serde writes fields in declaration order, which is not the order
 * this panel builds an object in. Neither difference is a change somebody
 * made, so neither may read as one.
 */
export function sameAnnotations(left: Annotation[], right: Annotation[]): boolean {
  return JSON.stringify(canonical(left)) === JSON.stringify(canonical(right));
}

function canonical(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonical);
  if (value === null || typeof value !== 'object') return value;
  const out: Record<string, unknown> = {};
  for (const key of Object.keys(value as Record<string, unknown>).sort()) {
    const entry = canonical((value as Record<string, unknown>)[key]);
    if (entry === undefined || entry === null) continue;
    // An absent optional container and an empty one say the same thing, and
    // the registry answers with the absent one.
    if (Array.isArray(entry) && entry.length === 0) continue;
    if (typeof entry === 'object' && Object.keys(entry as object).length === 0) continue;
    out[key] = entry;
  }
  return out;
}

export const REVIEW_TONES: Record<ReviewState, 'neutral' | 'running' | 'success' | 'danger'> = {
  draft: 'neutral',
  in_review: 'running',
  accepted: 'success',
  rejected: 'danger',
};

export const SPLIT_LABELS: Record<Split, string> = {
  train: 'train',
  validation: 'val',
  test: 'test',
};
