// Triangle selection in repair mode. A selection is a per-triangle mask (1 = selected) that is never changed in
// place, because the inner undo history keeps old masks; every change returns a new mask. The tools grow a pick
// through edge adjacency (Surface, Shell) or shared vertices (expand, shrink), built with counting sorts in linear
// time with no hash map per corner, so a working copy of a million triangles stays quick. A mesh's adjacency,
// normals and shells are built the first time a tool needs them and kept for the latest mesh only: older working
// copies can stay alive in the undo history, and their caches would hold tens of megabytes each. Leaving repair mode
// clears them (clearSelectionCaches), since they would otherwise keep the last working copy alive with them. Pure: no
// React, three, Tauri or store.
import { triangleCount } from "../info";
import type { Bounds } from "../mesh";
import type { MeshData } from "../store";

/**
 * How a pick combines with the selection: replace it, add to it, toggle (see applySelection), or remove from it.
 * A plain click replaces; Shift+click toggles.
 */
export type SelectMode = "replace" | "add" | "toggle" | "remove";

const isTriangle = (t: number, triangles: number) => Number.isInteger(t) && t >= 0 && t < triangles;

const vertexCount = (mesh: MeshData) => Math.floor(mesh.positions.length / 3);

// ---------------------------------------------------------------------------------------------
// Masks and ids

/** How many triangles the mask selects. */
export function countSelected(mask: Uint8Array): number {
  let n = 0;
  for (let t = 0; t < mask.length; t++) if (mask[t]) n++;
  return n;
}

/** The selected triangle ids, ascending (the form Rust takes). */
export function maskToIds(mask: Uint8Array): Uint32Array {
  const ids = new Uint32Array(countSelected(mask));
  for (let t = 0, k = 0; t < mask.length; t++) if (mask[t]) ids[k++] = t;
  return ids;
}

/** A mask over `triangles` triangles with `ids` selected; ids that name no triangle are ignored. */
export function idsToMask(ids: ArrayLike<number>, triangles: number): Uint8Array {
  const mask = new Uint8Array(triangles);
  for (let i = 0; i < ids.length; i++) {
    const t = ids[i];
    if (isTriangle(t, triangles)) mask[t] = 1;
  }
  return mask;
}

/**
 * The selection after a pick of `ids`, as a new mask and its count (`count` is the count of `mask`). Toggle removes
 * the ids when the first of them, the clicked triangle, is selected, and adds them otherwise, so Shift+click on a
 * surface or a shell takes the whole region out or puts it in; for one triangle it is a plain toggle. Ids that name
 * no triangle are ignored. Returns `mask` itself when nothing changes.
 */
export function applySelection(mask: Uint8Array, count: number, ids: ArrayLike<number>, mode: SelectMode): { mask: Uint8Array; count: number } {
  const n = mask.length;
  if (mode === "replace") {
    const out = new Uint8Array(n);
    let selected = 0;
    for (let i = 0; i < ids.length; i++) {
      const t = ids[i];
      if (isTriangle(t, n) && !out[t]) {
        out[t] = 1;
        selected++;
      }
    }
    return { mask: out, count: selected };
  }
  let value = mode === "remove" ? 0 : 1;
  if (mode === "toggle") {
    let first = -1;
    for (let i = 0; i < ids.length && first < 0; i++) if (isTriangle(ids[i], n)) first = ids[i];
    if (first < 0) return { mask, count };
    value = mask[first] ? 0 : 1;
  }
  let out = mask;
  let selected = count;
  for (let i = 0; i < ids.length; i++) {
    const t = ids[i];
    if (!isTriangle(t, n) || out[t] === value) continue;
    if (out === mask) out = mask.slice();
    out[t] = value;
    selected += value ? 1 : -1;
  }
  return { mask: out, count: selected };
}

/** Every unselected triangle selected and every selected one not. */
export function invertMask(mask: Uint8Array): Uint8Array {
  const out = new Uint8Array(mask.length);
  for (let t = 0; t < mask.length; t++) out[t] = mask[t] ? 0 : 1;
  return out;
}

/** Marks the corners of the triangles whose mask value is `value`. */
function cornersOf(mesh: MeshData, mask: Uint8Array, value: number): Uint8Array {
  const { indices } = mesh;
  const marked = new Uint8Array(vertexCount(mesh));
  for (let t = 0, c = 0; t < mask.length; t++, c += 3) {
    if ((mask[t] ? 1 : 0) !== value) continue;
    marked[indices[c]] = 1;
    marked[indices[c + 1]] = 1;
    marked[indices[c + 2]] = 1;
  }
  return marked;
}

/** The selection plus every triangle that shares a corner with it (the `=` key). */
export function expandMask(mesh: MeshData, mask: Uint8Array): Uint8Array {
  const { indices } = mesh;
  const touched = cornersOf(mesh, mask, 1);
  const out = mask.slice();
  for (let t = 0, c = 0; t < out.length; t++, c += 3) {
    if (!out[t] && (touched[indices[c]] || touched[indices[c + 1]] || touched[indices[c + 2]])) out[t] = 1;
  }
  return out;
}

/** The selection without its triangles that share a corner with an unselected triangle (the `-` key). */
export function shrinkMask(mesh: MeshData, mask: Uint8Array): Uint8Array {
  const { indices } = mesh;
  const outside = cornersOf(mesh, mask, 0);
  const out = mask.slice();
  for (let t = 0, c = 0; t < out.length; t++, c += 3) {
    if (out[t] && (outside[indices[c]] || outside[indices[c + 1]] || outside[indices[c + 2]])) out[t] = 0;
  }
  return out;
}

// ---------------------------------------------------------------------------------------------
// Adjacency

/** Marks a corner whose edge has no length (a degenerate triangle repeats an index). */
export const NO_EDGE = 0xffffffff;

/**
 * Which triangles meet at each edge. Corner c = 3t + k stands for the edge from corner k to corner k + 1 (mod 3) of
 * triangle t; its undirected edge is e = edgeOf[c] (NO_EDGE for a zero-length edge), and the corners of all the
 * triangles on that edge are edgeCorners[edgeStart[e] .. edgeStart[e + 1]], c among them. A manifold edge has two,
 * a border edge one, a non-manifold edge three or more.
 */
export interface EdgeAdjacency {
  edgeOf: Uint32Array;
  edgeStart: Uint32Array;
  edgeCorners: Uint32Array;
}

/**
 * Builds the edge adjacency in linear time: every corner's edge as the key higher vertex * scale + corner, bucketed
 * by its lower vertex (counting sort) and sorted within the bucket, so the corners of one edge sit next to each
 * other; as src/viewport/edges.ts does. `scale` is a power of two above the corner count, so keys split back
 * exactly while vertexCount * scale < 2^53.
 */
export function buildEdgeAdjacency(indices: Uint32Array, vertexCount: number): EdgeAdjacency {
  const corners = indices.length - (indices.length % 3);
  const scale = 2 ** Math.ceil(Math.log2(corners + 1));
  const next = (c: number) => (c % 3 === 2 ? c - 2 : c + 1);
  const start = new Uint32Array(vertexCount + 1);
  for (let c = 0; c < corners; c++) {
    const a = indices[c];
    const b = indices[next(c)];
    if (a !== b) start[(a < b ? a : b) + 1]++;
  }
  for (let v = 0; v < vertexCount; v++) start[v + 1] += start[v];

  const fill = start.slice(0, vertexCount);
  const keys = new Float64Array(start[vertexCount]);
  for (let c = 0; c < corners; c++) {
    const a = indices[c];
    const b = indices[next(c)];
    if (a < b) keys[fill[a]++] = b * scale + c;
    else if (b < a) keys[fill[b]++] = a * scale + c;
  }
  for (let v = 0; v < vertexCount; v++) sortRange(keys, start[v], start[v + 1]);

  const edgeOf = new Uint32Array(corners).fill(NO_EDGE);
  const edgeCorners = new Uint32Array(keys.length);
  const edgeStart = new Uint32Array(keys.length + 1);
  let edges = 0;
  for (let lo = 0; lo < vertexCount; lo++) {
    const end = start[lo + 1];
    for (let i = start[lo]; i < end; ) {
      const hi = Math.floor(keys[i] / scale);
      edgeStart[edges] = i;
      let j = i;
      for (; j < end && Math.floor(keys[j] / scale) === hi; j++) {
        const c = keys[j] - hi * scale;
        edgeCorners[j] = c;
        edgeOf[c] = edges;
      }
      edges++;
      i = j;
    }
  }
  edgeStart[edges] = keys.length;
  return { edgeOf, edgeStart: edgeStart.slice(0, edges + 1), edgeCorners };
}

/** A bucket holds one vertex's few edges, so insertion sort; the built-in sort guards big fans. */
function sortRange(keys: Float64Array, from: number, to: number) {
  if (to - from > 16) {
    keys.subarray(from, to).sort();
    return;
  }
  for (let i = from + 1; i < to; i++) {
    const key = keys[i];
    let j = i - 1;
    while (j >= from && keys[j] > key) {
      keys[j + 1] = keys[j];
      j--;
    }
    keys[j + 1] = key;
  }
}

/** What clearSelectionCaches runs: one reset per forLatestMesh cache. */
const cacheResets: (() => void)[] = [];

/** `build` remembered for the most recent mesh it was asked about, until clearSelectionCaches. */
function forLatestMesh<T>(build: (mesh: MeshData) => T): (mesh: MeshData) => T {
  let key: MeshData | null = null;
  let value: T | undefined;
  cacheResets.push(() => {
    key = null;
    value = undefined;
  });
  return (mesh) => {
    if (mesh !== key || value === undefined) {
      value = build(mesh);
      key = mesh;
    }
    return value;
  };
}

/**
 * Lets go of the cached adjacency, normals and shells and the mesh they were built for. Called when repair mode ends:
 * nothing else would free the last working copy and the tens of megabytes made from it until the next session.
 */
export function clearSelectionCaches(): void {
  for (const reset of cacheResets) reset();
}

const adjacencyOf = forLatestMesh((mesh) => buildEdgeAdjacency(mesh.indices, vertexCount(mesh)));

/** The mesh's edge adjacency, built on first use. */
export function edgeAdjacency(mesh: MeshData): EdgeAdjacency {
  return adjacencyOf(mesh);
}

/** Unit normal per triangle (x, y, z at 3t), zero for a triangle with no area; built on first use. */
export const triangleNormals: (mesh: MeshData) => Float32Array = forLatestMesh((mesh) => {
  const p = mesh.positions;
  const idx = mesh.indices;
  const normals = new Float32Array(triangleCount(mesh) * 3);
  for (let c = 0; c < normals.length; c += 3) {
    const a = idx[c] * 3;
    const b = idx[c + 1] * 3;
    const d = idx[c + 2] * 3;
    const ux = p[b] - p[a], uy = p[b + 1] - p[a + 1], uz = p[b + 2] - p[a + 2];
    const vx = p[d] - p[a], vy = p[d + 1] - p[a + 1], vz = p[d + 2] - p[a + 2];
    const nx = uy * vz - uz * vy;
    const ny = uz * vx - ux * vz;
    const nz = ux * vy - uy * vx;
    const length = Math.sqrt(nx * nx + ny * ny + nz * nz);
    if (length === 0) continue;
    normals[c] = nx / length;
    normals[c + 1] = ny / length;
    normals[c + 2] = nz / length;
  }
  return normals;
});

/**
 * The shell of each triangle, numbered in order of first triangle, as Rust's `Link::Shell` and the reference tool
 * count them: triangles joined across edges with exactly two triangles that run opposite ways, so shells part where an
 * edge is open or non-manifold or its two triangles disagree about which way they face.
 */
export interface TriangleShells {
  shellOf: Uint32Array;
  count: number;
}

/** Each triangle's shell, by union-find over the triangles across shell edges; built on first use. */
export const triangleShells: (mesh: MeshData) => TriangleShells = forLatestMesh((mesh) => {
  const idx = mesh.indices;
  const triangles = triangleCount(mesh);
  const { edgeStart, edgeCorners } = edgeAdjacency(mesh);
  const parent = new Int32Array(triangles);
  for (let t = 0; t < triangles; t++) parent[t] = t;
  const find = (t: number) => {
    while (parent[t] !== t) {
      parent[t] = parent[parent[t]];
      t = parent[t];
    }
    return t;
  };
  for (let e = 0; e + 1 < edgeStart.length; e++) {
    if (edgeStart[e + 1] - edgeStart[e] !== 2) continue;
    const c = edgeCorners[edgeStart[e]];
    const d = edgeCorners[edgeStart[e] + 1];
    // A corner's side runs from its own vertex to the next corner's: opposite ways start apart.
    if (idx[c] === idx[d]) continue;
    const a = find(Math.floor(c / 3));
    const b = find(Math.floor(d / 3));
    if (a < b) parent[b] = a;
    else if (b < a) parent[a] = b;
  }
  // A root is the lowest triangle of its shell, so it is labelled before the rest of it.
  const shellOf = new Uint32Array(triangles);
  let count = 0;
  for (let t = 0; t < triangles; t++) {
    const root = find(t);
    shellOf[t] = root === t ? count++ : shellOf[root];
  }
  return { shellOf, count };
});

// ---------------------------------------------------------------------------------------------
// Picks

/** Slack on the angle test so float noise in the normals cannot stop a fill across a flat surface at 0°. */
const ANGLE_SLACK = 1e-6;

/**
 * Surface selection: the triangles reachable from `seed` across edges whose dihedral angle (the angle between the
 * two triangles' normals) is at most `toleranceDeg`, seed first. At 180° every edge is crossed, so the pick is the
 * whole edge-connected region. Below that, a degenerate triangle (no normal) is never crossed into, and a
 * non-manifold edge leads to each triangle on it that passes. Empty when `seed` names no triangle.
 */
export function surfaceRegion(mesh: MeshData, seed: number, toleranceDeg: number): Uint32Array {
  const triangles = triangleCount(mesh);
  if (!isTriangle(seed, triangles)) return new Uint32Array(0);
  const { edgeOf, edgeStart, edgeCorners } = edgeAdjacency(mesh);
  const everything = toleranceDeg >= 180;
  const normals = everything ? null : triangleNormals(mesh);
  const minDot = Math.cos((Math.max(0, toleranceDeg) * Math.PI) / 180) - ANGLE_SLACK;
  const crosses = (t: number, u: number) => {
    if (!normals) return true;
    const a = t * 3;
    const b = u * 3;
    const flat = (i: number) => normals[i] === 0 && normals[i + 1] === 0 && normals[i + 2] === 0;
    if (flat(a) || flat(b)) return false;
    return normals[a] * normals[b] + normals[a + 1] * normals[b + 1] + normals[a + 2] * normals[b + 2] >= minDot;
  };
  const seen = new Uint8Array(triangles);
  // The queue doubles as the result: breadth-first order, seed first.
  const queue = new Uint32Array(triangles);
  let head = 0;
  let tail = 0;
  queue[tail++] = seed;
  seen[seed] = 1;
  while (head < tail) {
    const t = queue[head++];
    for (let c = t * 3; c < t * 3 + 3; c++) {
      const e = edgeOf[c];
      if (e === NO_EDGE) continue;
      for (let j = edgeStart[e]; j < edgeStart[e + 1]; j++) {
        const u = Math.floor(edgeCorners[j] / 3);
        if (seen[u] || !crosses(t, u)) continue;
        seen[u] = 1;
        queue[tail++] = u;
      }
    }
  }
  return queue.slice(0, tail);
}

/** Shell selection: every triangle of the seed's shell (`TriangleShells`), seed first. */
export function shellRegion(mesh: MeshData, seed: number): Uint32Array {
  const triangles = triangleCount(mesh);
  if (!isTriangle(seed, triangles)) return new Uint32Array(0);
  const { shellOf } = triangleShells(mesh);
  return trianglesOfShell(shellOf, shellOf[seed], seed);
}

/** The triangles whose entry in `shellOf` is `shell` (a Shells tab row), ascending, with `first` moved to the front when given. */
export function trianglesOfShell(shellOf: Uint32Array, shell: number, first?: number): Uint32Array {
  let n = 0;
  for (let t = 0; t < shellOf.length; t++) if (shellOf[t] === shell) n++;
  const out = new Uint32Array(n);
  let k = 0;
  const lead = first !== undefined && shellOf[first] === shell;
  if (lead) out[k++] = first;
  for (let t = 0; t < shellOf.length; t++) if (shellOf[t] === shell && !(lead && t === first)) out[k++] = t;
  return out;
}

// ---------------------------------------------------------------------------------------------
// Measures and copies

/** The Info group's numbers: triangles selected, their area in mm², and how many shells they touch. */
export interface SelectionInfo {
  count: number;
  area: number;
  shells: number;
}

/** Twice the area of triangle t, from its corners in f64. */
function doubleArea(p: Float32Array, idx: Uint32Array, t: number): number {
  const a = idx[t * 3] * 3;
  const b = idx[t * 3 + 1] * 3;
  const c = idx[t * 3 + 2] * 3;
  const ux = p[b] - p[a], uy = p[b + 1] - p[a + 1], uz = p[b + 2] - p[a + 2];
  const vx = p[c] - p[a], vy = p[c + 1] - p[a + 1], vz = p[c + 2] - p[a + 2];
  const nx = uy * vz - uz * vy;
  const ny = uz * vx - ux * vz;
  const nz = ux * vy - uy * vx;
  return Math.sqrt(nx * nx + ny * ny + nz * nz);
}

export function selectionInfoOf(mesh: MeshData, mask: Uint8Array): SelectionInfo {
  let count = 0;
  let area = 0;
  for (let t = 0; t < mask.length; t++) {
    if (!mask[t]) continue;
    count++;
    area += doubleArea(mesh.positions, mesh.indices, t);
  }
  if (count === 0) return { count: 0, area: 0, shells: 0 };
  const { shellOf, count: shellCount } = triangleShells(mesh);
  const touched = new Uint8Array(shellCount);
  let shells = 0;
  for (let t = 0; t < mask.length; t++) {
    if (mask[t] && !touched[shellOf[t]]) {
      touched[shellOf[t]] = 1;
      shells++;
    }
  }
  return { count, area: area / 2, shells };
}

/** The box around the selected triangles (Zoom to › Selected), or null when none is selected. */
export function selectionBounds(mesh: MeshData, mask: Uint8Array): Bounds | null {
  const p = mesh.positions;
  const idx = mesh.indices;
  let x0 = Infinity, y0 = Infinity, z0 = Infinity;
  let x1 = -Infinity, y1 = -Infinity, z1 = -Infinity;
  for (let t = 0; t < mask.length; t++) {
    if (!mask[t]) continue;
    for (let c = t * 3; c < t * 3 + 3; c++) {
      const v = idx[c] * 3;
      const x = p[v], y = p[v + 1], z = p[v + 2];
      if (x < x0) x0 = x;
      if (x > x1) x1 = x;
      if (y < y0) y0 = y;
      if (y > y1) y1 = y;
      if (z < z0) z0 = z;
      if (z > z1) z1 = z;
    }
  }
  return x0 <= x1 ? { min: [x0, y0, z0], max: [x1, y1, z1] } : null;
}

/** The selected triangles as a mesh of their own, with only the vertices they use, in their original order. */
export function extractTriangles(mesh: MeshData, mask: Uint8Array): MeshData {
  const p = mesh.positions;
  const idx = mesh.indices;
  const used = cornersOf(mesh, mask, 1);
  const remap = new Uint32Array(used.length);
  let vertices = 0;
  for (let v = 0; v < used.length; v++) if (used[v]) remap[v] = vertices++;
  const positions = new Float32Array(vertices * 3);
  for (let v = 0; v < used.length; v++) {
    if (!used[v]) continue;
    const to = remap[v] * 3;
    positions[to] = p[v * 3];
    positions[to + 1] = p[v * 3 + 1];
    positions[to + 2] = p[v * 3 + 2];
  }
  const indices = new Uint32Array(countSelected(mask) * 3);
  for (let t = 0, k = 0; t < mask.length; t++) {
    if (!mask[t]) continue;
    indices[k++] = remap[idx[t * 3]];
    indices[k++] = remap[idx[t * 3 + 1]];
    indices[k++] = remap[idx[t * 3 + 2]];
  }
  return { positions, indices };
}
