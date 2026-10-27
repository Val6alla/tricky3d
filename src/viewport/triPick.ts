// Picking triangles of the repair working copy: a bounding volume hierarchy over its triangles in flat typed arrays,
// and the nearest triangle a ray hits. The hierarchy is built the first time a click needs it (750k triangles in
// about 150 ms) and kept for the latest mesh only, since only the working copy is picked and meshes are immutable;
// leaving repair mode lets go of it (clearTriangleBvh). Pure: no three.js, store or DOM.
import type { MeshData, Vec3 } from "../store";

/** A leaf holds at most this many triangles. */
const LEAF_SIZE = 4;
/** Marks a pending range that is the first child of its parent: its node number is the parent's plus one. */
const NO_PARENT = -1;
/** Cells per axis of the grid the triangle centres are sorted on (10 bits each, a 30-bit Morton code). */
const GRID = 1024;
/**
 * Node boxes grow outwards by this share of the mesh's largest coordinate, so the rounding of the box test can never
 * miss a hit the triangle test finds, at any scale. Far above f64 noise, far below anything visible.
 */
const BOX_PAD = 2 ** -20;
/**
 * A triangle whose |det| is at most this share of |e1|·|e2|·|d| counts as missed: it has no area (a degenerate
 * triangle, which is never drawn either) or the ray grazes its plane.
 */
const FLAT = 1e-12;

/**
 * A binary tree over a mesh's triangles. Nodes are numbered depth first, so an inner node's first child is the next
 * node; a node's box contains every triangle under it.
 */
export interface TriangleBvh {
  positions: Float32Array;
  indices: Uint32Array;
  /** Per node: min x, y, z, then max x, y, z. */
  boxes: Float32Array;
  /** Per node: a leaf's first slot in `order`, or an inner node's second child. */
  first: Uint32Array;
  /** Per node: a leaf's triangle count (1 to LEAF_SIZE), or 0 for an inner node. */
  count: Uint32Array;
  /** Triangle ids, each leaf's next to each other. */
  order: Uint32Array;
}

/** A triangle hit by a ray: the triangle, how far along the ray (in direction lengths), and the point. */
export interface TriangleHit {
  triangle: number;
  distance: number;
  point: Vec3;
}

/**
 * Builds the hierarchy in linear time, with no work per node but its box. The triangles are sorted by the Morton
 * code of their centre's cell on a grid over the centres' box (a radix sort): the code follows a curve through the
 * cells that keeps near cells near, so every run of that order is spatially compact. The tree then halves runs at
 * the middle (depth log2(n / LEAF_SIZE)), and the boxes are filled bottom up.
 */
export function buildTriangleBvh(positions: Float32Array, indices: Uint32Array): TriangleBvh {
  const triangles = Math.floor(indices.length / 3);
  // The largest coordinate, for the box pad (unused vertices count too, which only makes the pad safer).
  let reach = 0;
  for (let i = 0; i < positions.length; i++) {
    const v = Math.abs(positions[i]);
    if (v > reach) reach = v;
  }
  // Three times each centre (the corners' sum), and the box of those.
  const centres = new Float32Array(triangles * 3);
  let x0 = Infinity, y0 = Infinity, z0 = Infinity;
  let x1 = -Infinity, y1 = -Infinity, z1 = -Infinity;
  for (let c = 0; c < triangles * 3; c += 3) {
    const a = indices[c] * 3;
    const b = indices[c + 1] * 3;
    const d = indices[c + 2] * 3;
    for (let k = 0; k < 3; k++) centres[c + k] = positions[a + k] + positions[b + k] + positions[d + k];
    const x = centres[c];
    const y = centres[c + 1];
    const z = centres[c + 2];
    if (x < x0) x0 = x;
    if (x > x1) x1 = x;
    if (y < y0) y0 = y;
    if (y > y1) y1 = y;
    if (z < z0) z0 = z;
    if (z > z1) z1 = z;
  }
  const codes = new Uint32Array(triangles);
  const cell = (v: number, lo: number, hi: number) => (hi > lo ? Math.min(GRID - 1, Math.floor(((v - lo) / (hi - lo)) * GRID)) : 0);
  for (let t = 0, c = 0; t < triangles; t++, c += 3) {
    codes[t] = spread(cell(centres[c], x0, x1)) | (spread(cell(centres[c + 1], y0, y1)) << 1) | (spread(cell(centres[c + 2], z0, z1)) << 2);
  }
  const order = sortByCode(codes);

  // Middle splits make between 3/8 and 3/5 nodes per triangle; grown by doubling when half is not enough.
  let first: Uint32Array = new Uint32Array(Math.max(1, Math.ceil(triangles / 2)));
  let count: Uint32Array = new Uint32Array(first.length);
  let nodes = 0;
  // Pending ranges as (start, end, parent): the parent links its second child once that range gets its number.
  const pending: number[] = triangles > 0 ? [0, triangles, NO_PARENT] : [];
  while (pending.length > 0) {
    const parent = pending.pop() as number;
    const end = pending.pop() as number;
    const start = pending.pop() as number;
    if (nodes === first.length) {
      first = grow(first);
      count = grow(count);
    }
    const node = nodes++;
    if (parent !== NO_PARENT) first[parent] = node;
    if (end - start <= LEAF_SIZE) {
      first[node] = start;
      count[node] = end - start;
      continue;
    }
    const mid = (start + end) >>> 1;
    // The second half goes first, so the first half comes off next and is numbered node + 1.
    pending.push(mid, end, node, start, mid, NO_PARENT);
  }

  const boxes = new Float32Array(nodes * 6);
  const pad = reach * BOX_PAD;
  // Children are numbered after their parent, so going backwards meets them first.
  for (let node = nodes - 1; node >= 0; node--) {
    const o = node * 6;
    if (count[node] === 0) {
      const l = (node + 1) * 6;
      const r = first[node] * 6;
      for (let k = 0; k < 3; k++) {
        boxes[o + k] = Math.min(boxes[l + k], boxes[r + k]);
        boxes[o + 3 + k] = Math.max(boxes[l + 3 + k], boxes[r + 3 + k]);
      }
      continue;
    }
    let bx0 = Infinity, by0 = Infinity, bz0 = Infinity;
    let bx1 = -Infinity, by1 = -Infinity, bz1 = -Infinity;
    for (let i = first[node]; i < first[node] + count[node]; i++) {
      const c = order[i] * 3;
      for (let k = 0; k < 3; k++) {
        const v = indices[c + k] * 3;
        const x = positions[v];
        const y = positions[v + 1];
        const z = positions[v + 2];
        if (x < bx0) bx0 = x;
        if (x > bx1) bx1 = x;
        if (y < by0) by0 = y;
        if (y > by1) by1 = y;
        if (z < bz0) bz0 = z;
        if (z > bz1) bz1 = z;
      }
    }
    // The pad is at least 8 float32 steps of every coordinate, so storing as float32 cannot round it away.
    boxes[o] = bx0 - pad;
    boxes[o + 1] = by0 - pad;
    boxes[o + 2] = bz0 - pad;
    boxes[o + 3] = bx1 + pad;
    boxes[o + 4] = by1 + pad;
    boxes[o + 5] = bz1 + pad;
  }
  return { positions, indices, boxes, first: first.slice(0, nodes), count: count.slice(0, nodes), order };
}

function grow(array: Uint32Array): Uint32Array {
  const out = new Uint32Array(array.length * 2);
  out.set(array);
  return out;
}

/** The low 10 bits of x with two zero bits after each: one axis's share of a 30-bit Morton code. */
function spread(x: number): number {
  x &= 0x3ff;
  x = (x | (x << 16)) & 0x030000ff;
  x = (x | (x << 8)) & 0x0300f00f;
  x = (x | (x << 4)) & 0x030c30c3;
  x = (x | (x << 2)) & 0x09249249;
  return x;
}

/**
 * The ids 0..n-1 sorted by `codes` (30-bit keys; equal keys keep their id order): a least-significant-digit radix
 * sort in three passes of 10 bits, carrying each key along with its id so every pass reads in order. Overwrites
 * `codes`.
 */
function sortByCode(codes: Uint32Array): Uint32Array {
  const n = codes.length;
  let order: Uint32Array = new Uint32Array(n);
  let keys: Uint32Array = codes;
  let nextOrder: Uint32Array = new Uint32Array(n);
  let nextKeys: Uint32Array = new Uint32Array(n);
  for (let t = 0; t < n; t++) order[t] = t;
  const slots = new Uint32Array(1024);
  for (let shift = 0; shift < 30; shift += 10) {
    slots.fill(0);
    for (let i = 0; i < n; i++) slots[(keys[i] >>> shift) & 1023]++;
    for (let digit = 0, sum = 0; digit < 1024; digit++) {
      const size = slots[digit];
      slots[digit] = sum;
      sum += size;
    }
    for (let i = 0; i < n; i++) {
      const key = keys[i];
      const to = slots[(key >>> shift) & 1023]++;
      nextOrder[to] = order[i];
      nextKeys[to] = key;
    }
    const doneOrder = order;
    order = nextOrder;
    nextOrder = doneOrder;
    const doneKeys = keys;
    keys = nextKeys;
    nextKeys = doneKeys;
  }
  return order;
}

/**
 * How far along the ray (origin o, direction d) it meets triangle t, both faces counting, or -1 for a miss: behind the
 * origin, outside the triangle (its edges and corners count as inside), or a triangle without area. Möller–Trumbore
 * in f64.
 */
export function rayTriangle(
  positions: Float32Array,
  indices: Uint32Array,
  t: number,
  ox: number,
  oy: number,
  oz: number,
  dx: number,
  dy: number,
  dz: number,
): number {
  const a = indices[t * 3] * 3;
  const b = indices[t * 3 + 1] * 3;
  const c = indices[t * 3 + 2] * 3;
  const ax = positions[a], ay = positions[a + 1], az = positions[a + 2];
  const e1x = positions[b] - ax, e1y = positions[b + 1] - ay, e1z = positions[b + 2] - az;
  const e2x = positions[c] - ax, e2y = positions[c + 1] - ay, e2z = positions[c + 2] - az;
  const px = dy * e2z - dz * e2y;
  const py = dz * e2x - dx * e2z;
  const pz = dx * e2y - dy * e2x;
  const det = e1x * px + e1y * py + e1z * pz;
  const scale = Math.sqrt((e1x * e1x + e1y * e1y + e1z * e1z) * (e2x * e2x + e2y * e2y + e2z * e2z) * (dx * dx + dy * dy + dz * dz));
  if (!(Math.abs(det) > FLAT * scale)) return -1;
  const inv = 1 / det;
  const sx = ox - ax, sy = oy - ay, sz = oz - az;
  const u = (sx * px + sy * py + sz * pz) * inv;
  if (u < 0 || u > 1) return -1;
  const qx = sy * e1z - sz * e1y;
  const qy = sz * e1x - sx * e1z;
  const qz = sx * e1y - sy * e1x;
  const v = (dx * qx + dy * qy + dz * qz) * inv;
  if (v < 0 || u + v > 1) return -1;
  const distance = (e2x * qx + e2y * qy + e2z * qz) * inv;
  return distance >= 0 ? distance : -1;
}

/**
 * The nearest triangle the ray from `origin` along `direction` hits (both faces count; distance >= 0), among the hits
 * `accept` takes: the viewport passes the clip planes, so a click on a hidden part of the surface reaches what is
 * drawn behind it. Equally near hits go to the lower triangle id. Null
 * when nothing is hit.
 */
export function raycastTriangles(
  bvh: TriangleBvh,
  origin: Vec3,
  direction: Vec3,
  accept?: (point: Vec3) => boolean,
  hidden?: Uint8Array | null,
): TriangleHit | null {
  const { positions, indices, boxes, first, count, order } = bvh;
  if (count.length === 0) return null;
  const [ox, oy, oz] = origin;
  const [dx, dy, dz] = direction;
  const inverse = [1 / dx, 1 / dy, 1 / dz];
  // A direction component of zero (or so small that its inverse overflows): the ray stays in its slab or misses it.
  const level = inverse.map((v) => !Number.isFinite(v));

  /** Where the ray enters node n's box (0 when the origin is inside), or Infinity when it misses the box. */
  const enter = (n: number): number => {
    const o = n * 6;
    let near = 0;
    let far = Infinity;
    for (let k = 0; k < 3; k++) {
      const from = origin[k];
      if (level[k]) {
        if (from < boxes[o + k] || from > boxes[o + 3 + k]) return Infinity;
        continue;
      }
      let lo = (boxes[o + k] - from) * inverse[k];
      let hi = (boxes[o + 3 + k] - from) * inverse[k];
      if (lo > hi) {
        const swap = lo;
        lo = hi;
        hi = swap;
      }
      if (lo > near) near = lo;
      if (hi < far) far = hi;
    }
    return near <= far ? near : Infinity;
  };

  let best = Infinity;
  let bestTriangle = -1;
  // Depth first, nearer child first; each pending node with the distance at which the ray enters it.
  const stack: number[] = [];
  const at: number[] = [];
  const rootEntry = enter(0);
  if (rootEntry < Infinity) {
    stack.push(0);
    at.push(rootEntry);
  }
  while (stack.length > 0) {
    const node = stack.pop() as number;
    // Strictly further only: an equally near hit with a lower id may still be inside.
    if ((at.pop() as number) > best) continue;
    if (count[node] > 0) {
      for (let i = first[node]; i < first[node] + count[node]; i++) {
        const t = order[i];
        if (hidden?.[t]) continue;
        const distance = rayTriangle(positions, indices, t, ox, oy, oz, dx, dy, dz);
        if (distance < 0 || distance > best || (distance === best && t > bestTriangle)) continue;
        if (accept && !accept([ox + dx * distance, oy + dy * distance, oz + dz * distance])) continue;
        best = distance;
        bestTriangle = t;
      }
      continue;
    }
    const a = node + 1;
    const b = first[node];
    const ta = enter(a);
    const tb = enter(b);
    // The farther child goes on the stack first, so the nearer one comes off next. A missed box (Infinity) never
    // goes on, even while nothing is hit yet and `best` is Infinity too.
    const aFirst = ta <= tb;
    const tNear = aFirst ? ta : tb;
    const tFar = aFirst ? tb : ta;
    if (tFar < Infinity && tFar <= best) {
      stack.push(aFirst ? b : a);
      at.push(tFar);
    }
    if (tNear < Infinity && tNear <= best) {
      stack.push(aFirst ? a : b);
      at.push(tNear);
    }
  }
  if (bestTriangle < 0) return null;
  return { triangle: bestTriangle, distance: best, point: [ox + dx * best, oy + dy * best, oz + dz * best] };
}

let latest: { mesh: MeshData; bvh: TriangleBvh } | null = null;

/** The hierarchy of `mesh`, built on first use. Only the latest mesh's is kept: picking only ever asks for the working copy. */
export function triangleBvh(mesh: MeshData): TriangleBvh {
  if (latest?.mesh !== mesh) latest = { mesh, bvh: buildTriangleBvh(mesh.positions, mesh.indices) };
  return latest.bvh;
}

/**
 * Lets go of the kept hierarchy and its mesh. Called when repair mode ends: the last working copy clicked, and some
 * 20 MB of tree for a big one, would otherwise stay alive until a click in a later session.
 */
export function clearTriangleBvh(): void {
  latest = null;
}

/** The nearest triangle of `mesh` the ray hits, as raycastTriangles, with the mesh's hierarchy built on first use. */
export function pickTriangle(
  mesh: MeshData,
  origin: Vec3,
  direction: Vec3,
  accept?: (point: Vec3) => boolean,
  hidden?: Uint8Array | null,
): TriangleHit | null {
  return raycastTriangles(triangleBvh(mesh), origin, direction, accept, hidden);
}
