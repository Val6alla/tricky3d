// Column-major 4×4 matrices in the layout of Part.matrix and three.js Matrix4.elements: the element
// in row r, column c is m[c * 4 + r], and the translation sits in m[12], m[13], m[14].
// Every function returns a new array; inputs are never changed.

export type Axis = 0 | 1 | 2;
type V3 = readonly [number, number, number];

/** a × b: the transform that applies b first, then a. */
export function multiply(a: readonly number[], b: readonly number[]): number[] {
  const out = new Array<number>(16);
  for (let c = 0; c < 4; c++) {
    for (let r = 0; r < 4; r++) {
      out[c * 4 + r] =
        a[r] * b[c * 4] + a[4 + r] * b[c * 4 + 1] + a[8 + r] * b[c * 4 + 2] + a[12 + r] * b[c * 4 + 3];
    }
  }
  return out;
}

/** Determinant of the linear (upper-left 3×3) part. Negative means the transform mirrors. */
export function det3(m: readonly number[]): number {
  return (
    m[0] * (m[5] * m[10] - m[9] * m[6]) -
    m[4] * (m[1] * m[10] - m[9] * m[2]) +
    m[8] * (m[1] * m[6] - m[5] * m[2])
  );
}

/** Inverse of an affine matrix (last row 0, 0, 0, 1). Null when it flattens space and has no inverse. */
export function invert(m: readonly number[]): number[] | null {
  const det = det3(m);
  if (det === 0 || !Number.isFinite(det)) return null;
  const d = 1 / det;
  // Inverse of the 3×3 part by cofactors, then the translation is -L⁻¹·t.
  const a = (m[5] * m[10] - m[9] * m[6]) * d;
  const b = (m[9] * m[2] - m[1] * m[10]) * d;
  const c = (m[1] * m[6] - m[5] * m[2]) * d;
  const e = (m[8] * m[6] - m[4] * m[10]) * d;
  const f = (m[0] * m[10] - m[8] * m[2]) * d;
  const g = (m[4] * m[2] - m[0] * m[6]) * d;
  const h = (m[4] * m[9] - m[8] * m[5]) * d;
  const i = (m[8] * m[1] - m[0] * m[9]) * d;
  const j = (m[0] * m[5] - m[4] * m[1]) * d;
  const [tx, ty, tz] = [m[12], m[13], m[14]];
  // prettier-ignore
  return [
    a, b, c, 0,
    e, f, g, 0,
    h, i, j, 0,
    -(a * tx + e * ty + h * tz), -(b * tx + f * ty + i * tz), -(c * tx + g * ty + j * tz), 1,
  ];
}

export function translationMatrix([x, y, z]: V3): number[] {
  return [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, x, y, z, 1];
}

/** cos/sin of right angles come out as 6e-17 instead of 0; snapping keeps 90° turns exact and repeatable. */
function snap(v: number): number {
  return Math.abs(v) < 1e-15 ? 0 : v;
}

/** Linear part L (column-major 3×3 inside a 4×4) placed so that `pivot` stays where it is: T(p)·L·T(−p). */
function aboutPivot(l: number[], [px, py, pz]: V3): number[] {
  l[12] = px - (l[0] * px + l[4] * py + l[8] * pz);
  l[13] = py - (l[1] * px + l[5] * py + l[9] * pz);
  l[14] = pz - (l[2] * px + l[6] * py + l[10] * pz);
  return l;
}

/**
 * Rotation by `radians` about the world axis through `pivot`, counter-clockwise when looking from the
 * positive end of the axis towards the pivot (right-handed, like three.js).
 */
export function rotationAbout(axis: Axis, radians: number, pivot: V3 = [0, 0, 0]): number[] {
  const c = snap(Math.cos(radians));
  const s = snap(Math.sin(radians));
  const n = 0 - s; // -s, but +0 rather than -0 for right angles
  // prettier-ignore
  const l =
    axis === 0 ? [1, 0, 0, 0,  0, c, s, 0,  0, n, c, 0,  0, 0, 0, 1] :
    axis === 1 ? [c, 0, n, 0,  0, 1, 0, 0,  s, 0, c, 0,  0, 0, 0, 1] :
                 [c, s, 0, 0,  n, c, 0, 0,  0, 0, 1, 0,  0, 0, 0, 1];
  return aboutPivot(l, pivot);
}

/** Scale by [sx, sy, sz] along the world axes, keeping `pivot` fixed. */
export function scaleAbout([sx, sy, sz]: V3, pivot: V3 = [0, 0, 0]): number[] {
  return aboutPivot([sx, 0, 0, 0, 0, sy, 0, 0, 0, 0, sz, 0, 0, 0, 0, 1], pivot);
}

/** Applies a world-space `delta` to each part (new matrix = delta × matrix), keyed by part id. */
export function applyDelta(
  parts: readonly { id: string; matrix: readonly number[] }[],
  delta: readonly number[],
): Map<string, number[]> {
  return new Map(parts.map((p) => [p.id, multiply(delta, p.matrix)]));
}

/** Transforms a point by a matrix. */
export function transformPoint(m: readonly number[], [x, y, z]: V3): [number, number, number] {
  return [
    m[0] * x + m[4] * y + m[8] * z + m[12],
    m[1] * x + m[5] * y + m[9] * z + m[13],
    m[2] * x + m[6] * y + m[10] * z + m[14],
  ];
}
