import type { MeshData, Part, Vec3 } from "./store";

export const IDENTITY: readonly number[] = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];

/** Column-major translation matrix. */
export function translation(x: number, y: number, z: number): number[] {
  return [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, x, y, z, 1];
}

/** Axis-aligned box centred on the origin, 8 welded vertices and 12 outward-facing triangles. */
export function makeBox([sx, sy, sz]: Vec3): MeshData {
  const [x, y, z] = [sx / 2, sy / 2, sz / 2];
  // prettier-ignore
  const positions = new Float32Array([
    -x, -y, -z,  x, -y, -z,  x, y, -z,  -x, y, -z,
    -x, -y,  z,  x, -y,  z,  x, y,  z,  -x, y,  z,
  ]);
  // prettier-ignore
  const indices = new Uint32Array([
    0, 2, 1, 0, 3, 2, // bottom (-Z)
    4, 5, 6, 4, 6, 7, // top (+Z)
    0, 1, 5, 0, 5, 4, // front (-Y)
    3, 7, 6, 3, 6, 2, // back (+Y)
    0, 4, 7, 0, 7, 3, // left (-X)
    1, 2, 6, 1, 6, 5, // right (+X)
  ]);
  return { positions, indices };
}

export interface Bounds {
  min: Vec3;
  max: Vec3;
}

/** World-space axis-aligned bounds of a mesh under a column-major matrix. Null for an empty mesh. */
export function boundsOf(mesh: MeshData, m: readonly number[]): Bounds | null {
  const p = mesh.positions;
  if (p.length < 3) return null;
  // Scalars rather than arrays in the loop: this runs on every gizmo move over meshes of millions of vertices.
  let x0 = Infinity, y0 = Infinity, z0 = Infinity;
  let x1 = -Infinity, y1 = -Infinity, z1 = -Infinity;
  for (let i = 0; i < p.length; i += 3) {
    const x = p[i];
    const y = p[i + 1];
    const z = p[i + 2];
    const wx = m[0] * x + m[4] * y + m[8] * z + m[12];
    const wy = m[1] * x + m[5] * y + m[9] * z + m[13];
    const wz = m[2] * x + m[6] * y + m[10] * z + m[14];
    if (wx < x0) x0 = wx;
    if (wx > x1) x1 = wx;
    if (wy < y0) y0 = wy;
    if (wy > y1) y1 = wy;
    if (wz < z0) z0 = wz;
    if (wz > z1) z1 = wz;
  }
  return { min: [x0, y0, z0], max: [x1, y1, z1] };
}

const partBoundsCache = new WeakMap<Part, Bounds | null>();

/**
 * Exact world bounds of a part. Parts are immutable and replaced on every change, so the result is
 * cached per Part object: a gizmo drag recomputes only the parts it moves.
 */
export function partBounds(part: Part): Bounds | null {
  let b = partBoundsCache.get(part);
  if (b === undefined) {
    b = boundsOf(part.mesh, part.matrix);
    partBoundsCache.set(part, b);
  }
  return b;
}

/** Centre of a box. */
export function centreOf(b: Bounds): Vec3 {
  return [(b.min[0] + b.max[0]) / 2, (b.min[1] + b.max[1]) / 2, (b.min[2] + b.max[2]) / 2];
}

/** Union of several bounds, ignoring nulls. Null when there is nothing to enclose. */
export function unionBounds(list: (Bounds | null)[]): Bounds | null {
  let out: Bounds | null = null;
  for (const b of list) {
    if (!b) continue;
    if (!out) {
      out = { min: [...b.min], max: [...b.max] };
      continue;
    }
    for (let a = 0; a < 3; a++) {
      out.min[a] = Math.min(out.min[a], b.min[a]);
      out.max[a] = Math.max(out.max[a], b.max[a]);
    }
  }
  return out;
}
