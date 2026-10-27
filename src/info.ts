// Information panel maths. Rust measures each mesh once in part-local space (MeshStats); this
// module turns those numbers into world values under each part's matrix, so a transform change
// costs a few multiplications instead of a new pass over the mesh. Pure: no React, three or Tauri.
import type { MeshStats } from "./io/ipc";
import { partBounds, unionBounds } from "./mesh";
import type { MeshData, Part } from "./store";

/** Stats for a mesh: undefined while they load, null when they are unavailable (plain browser, failed). */
export type StatsLookup = (mesh: MeshData) => MeshStats | null | undefined;

export interface InfoRow {
  label: string;
  /** What the panel shows: a formatted number, "…" while loading, or "—" when there is no value. */
  text: string;
  status: "value" | "loading" | "none";
  /** Shown as a warning icon with this tooltip. */
  warning?: string;
}

export interface InfoResult {
  rows: InfoRow[];
  /** Parts whose exact area the caller should compute in the background (see computeAreasInBackground). */
  pendingArea: Part[];
}

export const NOT_CLOSED = "mesh not closed";

/** Relative tolerance for treating a matrix as rotation × uniform scale (see similarityScale2). */
const SIMILARITY_TOL = 1e-7;

/** Determinant of the linear (upper-left 3×3) part of a column-major 4×4 matrix. */
export function linearDet(m: readonly number[]): number {
  return (
    m[0] * (m[5] * m[10] - m[6] * m[9]) - m[4] * (m[1] * m[10] - m[2] * m[9]) + m[8] * (m[1] * m[6] - m[2] * m[5])
  );
}

/**
 * s² when the linear part is a similarity (s × rotation, possibly mirrored): its columns are
 * orthogonal and of equal length s, within a relative tolerance. Null otherwise. Areas then scale
 * by exactly s². The tolerance (1e-7) is about float32 precision, so the error it allows is below
 * what the vertex data itself carries; matrices built by our tools in f64 pass with room to spare.
 */
export function similarityScale2(m: readonly number[]): number | null {
  const l0 = m[0] * m[0] + m[1] * m[1] + m[2] * m[2];
  const l1 = m[4] * m[4] + m[5] * m[5] + m[6] * m[6];
  const l2 = m[8] * m[8] + m[9] * m[9] + m[10] * m[10];
  const s2 = (l0 + l1 + l2) / 3;
  if (s2 === 0) return 0; // everything collapses to a point
  const tol = SIMILARITY_TOL * s2;
  const d01 = m[0] * m[4] + m[1] * m[5] + m[2] * m[6];
  const d02 = m[0] * m[8] + m[1] * m[9] + m[2] * m[10];
  const d12 = m[4] * m[8] + m[5] * m[9] + m[6] * m[10];
  const ok =
    Math.abs(l0 - s2) <= tol &&
    Math.abs(l1 - s2) <= tol &&
    Math.abs(l2 - s2) <= tol &&
    Math.abs(d01) <= tol &&
    Math.abs(d02) <= tol &&
    Math.abs(d12) <= tol;
  return ok ? s2 : null;
}

/**
 * World volume (mm³) of a mesh with signed local volume `localVolume` under `m`: |det A × V|.
 * The absolute value is taken per part, after the signed sum over its triangles, so:
 * - a mirrored part (det < 0) reports the same volume as the original;
 * - an inside-out mesh (all normals inward, negative signed volume) reports its material volume
 *   rather than a negative number; finding flipped normals is the repair tools' job, not this panel's;
 * - a hollow part (outer shell plus an inward-facing inner shell) keeps its cavity subtracted,
 *   which taking |V| per shell would not.
 */
export function worldVolume(localVolume: number, m: readonly number[]): number {
  return Math.abs(linearDet(m) * localVolume);
}

export function triangleCount(mesh: MeshData): number {
  return Math.floor(mesh.indices.length / 3);
}

/**
 * Exact world area (mm²) of triangles [start, end) under `m`, without transforming any vertex.
 * For a local edge cross product n = e1 × e2, the world cross product is (A e1) × (A e2) = cof(A) n,
 * where the cofactor matrix cof(A) = det(A) A^-T has columns a1×a2, a2×a0, a0×a1 (a_i = columns of
 * A). So each triangle adds |cof(A) n| / 2, which also stays correct when A is singular.
 */
export function transformedArea(mesh: MeshData, m: readonly number[], start = 0, end = triangleCount(mesh)): number {
  const p = mesh.positions;
  const idx = mesh.indices;
  // Columns of A.
  const a0x = m[0], a0y = m[1], a0z = m[2];
  const a1x = m[4], a1y = m[5], a1z = m[6];
  const a2x = m[8], a2y = m[9], a2z = m[10];
  // Columns of cof(A).
  const k0x = a1y * a2z - a1z * a2y, k0y = a1z * a2x - a1x * a2z, k0z = a1x * a2y - a1y * a2x;
  const k1x = a2y * a0z - a2z * a0y, k1y = a2z * a0x - a2x * a0z, k1z = a2x * a0y - a2y * a0x;
  const k2x = a0y * a1z - a0z * a1y, k2y = a0z * a1x - a0x * a1z, k2z = a0x * a1y - a0y * a1x;
  let sum = 0;
  // Scalars only in the loop: it runs over millions of triangles.
  for (let t = start * 3, stop = end * 3; t < stop; t += 3) {
    const i0 = idx[t] * 3;
    const i1 = idx[t + 1] * 3;
    const i2 = idx[t + 2] * 3;
    const ox = p[i0], oy = p[i0 + 1], oz = p[i0 + 2];
    const ux = p[i1] - ox, uy = p[i1 + 1] - oy, uz = p[i1 + 2] - oz;
    const vx = p[i2] - ox, vy = p[i2 + 1] - oy, vz = p[i2 + 2] - oz;
    const nx = uy * vz - uz * vy;
    const ny = uz * vx - ux * vz;
    const nz = ux * vy - uy * vx;
    const wx = k0x * nx + k1x * ny + k2x * nz;
    const wy = k0y * nx + k1y * ny + k2y * nz;
    const wz = k0z * nx + k1z * ny + k2z * nz;
    sum += Math.sqrt(wx * wx + wy * wy + wz * wz);
  }
  return sum / 2;
}

// ---------- Exact-area cache ----------
//
// The exact area depends on the mesh and the linear 3×3 of the matrix, never on the translation.
// Two caches:
// - per Part object, like partBounds: a part once measured is never measured again while the
//   store or the undo history holds it, even when copies of one mesh (duplicates share it) use
//   more linear parts than the per-mesh list keeps;
// - per mesh, the areas under its most recent linear parts. Move, Place on platform, Centre and
//   undo create new Part objects with a linear part already measured, and hit this one.

const byPart = new WeakMap<Part, number>();
const byMesh = new WeakMap<MeshData, Map<string, number>>();
/** Linear parts remembered per mesh. A scale drag adds one per frame, so the list is capped. */
const RECENT_PER_MESH = 16;

/** Triangles one panel update may measure during render (about a millisecond); the rest go to the background. */
export const SYNC_AREA_TRIANGLES = 200_000;
/** Triangles per background slice: a few milliseconds of work, so the UI keeps responding. */
const AREA_SLICE_TRIANGLES = 400_000;

/** The linear 3×3 of a matrix as text: the per-mesh cache key. Numbers print exactly, so equal text means equal values. */
function linearKey(m: readonly number[]): string {
  return `${m[0]},${m[1]},${m[2]},${m[4]},${m[5]},${m[6]},${m[8]},${m[9]},${m[10]}`;
}

function remember(part: Part, area: number): void {
  byPart.set(part, area);
  let recent = byMesh.get(part.mesh);
  if (!recent) byMesh.set(part.mesh, (recent = new Map()));
  const key = linearKey(part.matrix);
  recent.delete(key); // re-inserting makes it the newest
  recent.set(key, area);
  if (recent.size > RECENT_PER_MESH) recent.delete(recent.keys().next().value!);
}

/** The cached exact world area of a part (mm²), or undefined when it has not been measured. */
export function cachedExactArea(part: Part): number | undefined {
  let area = byPart.get(part);
  if (area === undefined) {
    area = byMesh.get(part.mesh)?.get(linearKey(part.matrix));
    if (area !== undefined) remember(part, area);
  }
  return area;
}

/** Measures a part's exact world area now, O(triangles), and caches it. */
export function measureExactArea(part: Part): number {
  const area = transformedArea(part.mesh, part.matrix);
  remember(part, area);
  return area;
}

/**
 * True when two lists of parts need the same area measurements: the same meshes under the same
 * linear parts, in order. A move keeps both, so the caller lets a background run carry on.
 */
export function sameAreaWork(a: readonly Part[], b: readonly Part[]): boolean {
  return a.length === b.length && a.every((p, i) => p.mesh === b[i].mesh && linearKey(p.matrix) === linearKey(b[i].matrix));
}

/**
 * Measures the exact areas of `parts` in slices of up to AREA_SLICE_TRIANGLES triangles, across
 * parts, yielding to the UI between slices, and calls `onDone` once all are cached. Returns a
 * cancel function, for when the caller needs different measurements (see sameAreaWork).
 */
export function computeAreasInBackground(parts: readonly Part[], onDone: () => void): () => void {
  let index = 0; // the part being measured
  let next = 0; // its first triangle not yet summed
  let sum = 0;
  let timer: ReturnType<typeof setTimeout>;
  const step = () => {
    let budget = AREA_SLICE_TRIANGLES;
    while (index < parts.length && budget > 0) {
      const part = parts[index];
      if (next === 0 && cachedExactArea(part) !== undefined) {
        index += 1; // measured meanwhile, e.g. a copy with the same mesh and linear part
        continue;
      }
      const total = triangleCount(part.mesh);
      const end = Math.min(total, next + budget);
      sum += transformedArea(part.mesh, part.matrix, next, end);
      budget -= end - next;
      next = end;
      if (next === total) {
        remember(part, sum);
        index += 1;
        next = 0;
        sum = 0;
      }
    }
    if (index < parts.length) timer = setTimeout(step, 0);
    else onDone();
  };
  timer = setTimeout(step, 0);
  return () => clearTimeout(timer);
}

// ---------- Rows ----------

const fixed2 = new Intl.NumberFormat("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
const fixed3 = new Intl.NumberFormat("en-US", { minimumFractionDigits: 3, maximumFractionDigits: 3 });
const count = new Intl.NumberFormat("en-US", { maximumFractionDigits: 0 });

/** Whole numbers with thousands separators: 1,234,567. */
export const formatCount = (n: number) => count.format(n);

const value = (label: string, text: string): InfoRow => ({ label, text, status: "value" });
const none = (label: string, warning?: string): InfoRow => ({ label, text: "—", status: "none", warning });
const loading = (label: string): InfoRow => ({ label, text: "…", status: "loading" });

const DIM_LABELS = ["Length (mm)", "Width (mm)", "Height (mm)"] as const;
const VOLUME = "Volume (cm³)";
const AREA = "Area (cm²)";
const COUNT_LABELS = ["Triangles", "Shells", "Open edges"] as const;

/**
 * The panel rows for `parts` (the selection, or all visible parts). Dimensions come from the
 * exact world box and never need Rust; the rest combines each mesh's local stats with its matrix.
 * Parts whose matrix is not a similarity need an exact area pass: cached ones cost nothing, and
 * this call measures uncached ones for at most `syncBudget` triangles in total. Whatever does not
 * fit is returned in pendingArea, so many parts just under the limit cannot add up to a long frame.
 */
export function infoRows(parts: readonly Part[], statsOf: StatsLookup, syncBudget = SYNC_AREA_TRIANGLES): InfoResult {
  const rows: InfoRow[] = [];
  const pendingArea: Part[] = [];

  const box = unionBounds(parts.map(partBounds));
  DIM_LABELS.forEach((label, axis) => {
    rows.push(box ? value(label, fixed2.format(box.max[axis] - box.min[axis])) : none(label));
  });

  const statLabels = [VOLUME, AREA, ...COUNT_LABELS];
  const stats: MeshStats[] = [];
  let state: InfoRow["status"] = parts.length ? "value" : "none";
  for (const part of parts) {
    const s = statsOf(part.mesh);
    if (s === null) state = "none";
    else if (s === undefined) state = state === "none" ? "none" : "loading";
    else stats.push(s);
  }
  if (state !== "value") {
    for (const label of statLabels) rows.push(state === "loading" ? loading(label) : none(label));
    return { rows, pendingArea };
  }

  let volume = 0;
  let area = 0;
  let areaPending = false;
  let triangles = 0;
  let shells = 0;
  let openEdges = 0;
  let budget = syncBudget;
  parts.forEach((part, i) => {
    const s = stats[i];
    volume += worldVolume(s.volume, part.matrix);
    const s2 = similarityScale2(part.matrix);
    if (s2 !== null) {
      area += s.area * s2;
    } else {
      let exact = cachedExactArea(part);
      const n = triangleCount(part.mesh);
      if (exact === undefined && n <= budget) {
        exact = measureExactArea(part);
        budget -= n;
      }
      if (exact === undefined) {
        areaPending = true;
        pendingArea.push(part);
      } else {
        area += exact;
      }
    }
    triangles += s.triangles;
    shells += s.shells;
    openEdges += s.openEdges;
  });

  // mm³ → cm³ and mm² → cm².
  rows.push(openEdges > 0 ? none(VOLUME, NOT_CLOSED) : value(VOLUME, fixed3.format(volume / 1000)));
  rows.push(areaPending ? loading(AREA) : value(AREA, fixed3.format(area / 100)));
  rows.push(value("Triangles", formatCount(triangles)));
  rows.push(value("Shells", formatCount(shells)));
  rows.push(value("Open edges", formatCount(openEdges)));
  return { rows, pendingArea };
}

/** "2 of 5 parts selected". */
export function selectionText(selected: number, total: number): string {
  return `${formatCount(selected)} of ${formatCount(total)} ${total === 1 ? "part" : "parts"} selected`;
}
