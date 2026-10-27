// Box selection's geometry: which triangles a rectangle dragged on the canvas touches. Plain arrays in, no three.js
// objects or DOM, so it is tested on its own; src/viewport/picking.ts runs the drag and src/viewport/engine.ts feeds
// in the camera and the meshes.

/** A rectangle on the canvas in CSS pixels from its top-left corner, with left <= right and top <= bottom. */
export interface ScreenRect {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

/** The rectangle between two canvas points, whichever way the drag went. */
export function rectBetween(x0: number, y0: number, x1: number, y1: number): ScreenRect {
  return { left: Math.min(x0, x1), top: Math.min(y0, y1), right: Math.max(x0, x1), bottom: Math.max(y0, y1) };
}

/**
 * Each vertex of `positions` on the canvas (`width` × `height` CSS pixels), as x, y pairs, under `m`: a column-major
 * 4×4 matrix from the mesh's coordinates to clip space (projection × view × model). A vertex at or behind the camera
 * gets NaN, and no triangle that uses it counts as on the screen.
 */
export function projectVertices(positions: ArrayLike<number>, m: ArrayLike<number>, width: number, height: number): Float64Array {
  const n = Math.floor(positions.length / 3);
  const out = new Float64Array(n * 2);
  for (let i = 0; i < n; i++) {
    const x = positions[3 * i];
    const y = positions[3 * i + 1];
    const z = positions[3 * i + 2];
    const w = m[3] * x + m[7] * y + m[11] * z + m[15];
    if (!(w > 1e-12)) {
      out[2 * i] = NaN;
      out[2 * i + 1] = NaN;
      continue;
    }
    const cx = (m[0] * x + m[4] * y + m[8] * z + m[12]) / w;
    const cy = (m[1] * x + m[5] * y + m[9] * z + m[13]) / w;
    out[2 * i] = (cx + 1) * 0.5 * width;
    out[2 * i + 1] = (1 - cy) * 0.5 * height;
  }
  return out;
}

const inside = (x: number, y: number, r: ScreenRect) => x >= r.left && x <= r.right && y >= r.top && y <= r.bottom;

/** True when segment (ax, ay)–(bx, by) meets the rectangle (Liang–Barsky clipping). */
function segmentMeetsRect(ax: number, ay: number, bx: number, by: number, r: ScreenRect): boolean {
  const dx = bx - ax;
  const dy = by - ay;
  let t0 = 0;
  let t1 = 1;
  for (const [p, q] of [
    [-dx, ax - r.left],
    [dx, r.right - ax],
    [-dy, ay - r.top],
    [dy, r.bottom - ay],
  ]) {
    if (p === 0) {
      if (q < 0) return false;
    } else {
      const t = q / p;
      if (p < 0) t0 = Math.max(t0, t);
      else t1 = Math.min(t1, t);
      if (t0 > t1) return false;
    }
  }
  return true;
}

/** True when point (px, py) lies inside or on triangle a, b, c (either winding). */
function pointInTriangle(px: number, py: number, ax: number, ay: number, bx: number, by: number, cx: number, cy: number): boolean {
  const d1 = (bx - ax) * (py - ay) - (by - ay) * (px - ax);
  const d2 = (cx - bx) * (py - by) - (cy - by) * (px - bx);
  const d3 = (ax - cx) * (py - cy) - (ay - cy) * (px - cx);
  return !((d1 < 0 || d2 < 0 || d3 < 0) && (d1 > 0 || d2 > 0 || d3 > 0));
}

/** True when the canvas triangle a, b, c is at least partly inside the rectangle. */
export function triangleTouchesRect(ax: number, ay: number, bx: number, by: number, cx: number, cy: number, r: ScreenRect): boolean {
  if (Math.max(ax, bx, cx) < r.left || Math.min(ax, bx, cx) > r.right) return false;
  if (Math.max(ay, by, cy) < r.top || Math.min(ay, by, cy) > r.bottom) return false;
  if (inside(ax, ay, r) || inside(bx, by, r) || inside(cx, cy, r)) return true;
  // The rectangle inside the triangle, or an edge across it.
  if (pointInTriangle(r.left, r.top, ax, ay, bx, by, cx, cy)) return true;
  return segmentMeetsRect(ax, ay, bx, by, r) || segmentMeetsRect(bx, by, cx, cy, r) || segmentMeetsRect(cx, cy, ax, ay, r);
}

/**
 * The triangles of `indices` over the canvas points `screen` (from projectVertices) at least partly inside the
 * rectangle, in order. With `frontOnly`, only those facing the camera: counter-clockwise as seen, the way three.js
 * draws a front face (clockwise in canvas pixels, whose y runs down). `keep(t)` can rule triangles out (the ones a
 * clip plane removes).
 */
export function trianglesInRect(
  indices: ArrayLike<number>,
  screen: Float64Array,
  r: ScreenRect,
  frontOnly: boolean,
  keep: (t: number) => boolean = () => true,
): number[] {
  const out: number[] = [];
  const n = Math.floor(indices.length / 3);
  for (let t = 0; t < n; t++) {
    if (touches(indices, screen, t, r, frontOnly) && keep(t)) out.push(t);
  }
  return out;
}

/** True when any triangle of `indices` is at least partly inside the rectangle (see trianglesInRect). */
export function anyTriangleInRect(
  indices: ArrayLike<number>,
  screen: Float64Array,
  r: ScreenRect,
  keep: (t: number) => boolean = () => true,
): boolean {
  const n = Math.floor(indices.length / 3);
  for (let t = 0; t < n; t++) {
    if (touches(indices, screen, t, r, false) && keep(t)) return true;
  }
  return false;
}

function touches(indices: ArrayLike<number>, screen: Float64Array, t: number, r: ScreenRect, frontOnly: boolean): boolean {
  const a = 2 * indices[3 * t];
  const b = 2 * indices[3 * t + 1];
  const c = 2 * indices[3 * t + 2];
  const ax = screen[a];
  const ay = screen[a + 1];
  const bx = screen[b];
  const by = screen[b + 1];
  const cx = screen[c];
  const cy = screen[c + 1];
  // NaN (a corner behind the camera) fails every comparison below, so such a triangle never counts.
  if (Number.isNaN(ax + bx + cx)) return false;
  if (frontOnly && !((bx - ax) * (cy - ay) - (by - ay) * (cx - ax) < 0)) return false;
  return triangleTouchesRect(ax, ay, bx, by, cx, cy, r);
}
