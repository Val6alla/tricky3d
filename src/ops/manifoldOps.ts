// Boolean operations and plane split with manifold-3d. Plain functions over an already loaded module
// (setup() called), so the same code runs in the worker and in vitest under node.
//
// manifold-3d objects live in wasm memory and are not garbage collected: every Manifold made here is
// deleted in a finally block, the inputs as well as the results of the Booleans and splitByPlane(). Its Mesh
// is a plain JS object (no delete), and getMesh() copies the arrays out of wasm memory into buffers of
// their own. One leak is out of reach: when `new Manifold(mesh)` rejects a mesh, the wrapper throws
// before handing back the object it built, so that object (about 250 bytes, without the mesh data)
// can never be freed.
import type { Manifold, ManifoldToplevel } from "manifold-3d";
import type { MeshData } from "../store";
import type { CutPlane } from "./types";

/** What Boolean operations computes: everything fused, the main parts minus the others, or what they share. */
export type BooleanOp = "union" | "difference" | "intersection";

/**
 * "invalid" lists the inputs (by index into main, then tool) that are not closed, oriented solids, so nothing was
 * computed. `bodies` is, per input, how many separate solids it was on its own once its overlapping shells were fused:
 * a union has fewer shells than their sum when parts touched. `noise` is how many result shells were dropped as
 * smaller than the noise volume. An empty mesh means nothing is left (no overlap, or everything subtracted).
 */
export type BooleanOutcome = { kind: "ok"; mesh: MeshData; bodies: number[]; noise: number } | { kind: "invalid"; indices: number[] };

/** Progress of a Boolean: 0..1 and what it is doing. */
export type BooleanProgress = (fraction: number, label: string) => void;

/**
 * "invalid": the input is not a closed, oriented solid (or has no triangles), so the caller cuts it another
 * way. Both sides null: the plane does not cross the part, which stays as it is. Otherwise both are set.
 */
export type SplitOutcome = { kind: "ok"; below: MeshData | null; above: MeshData | null } | { kind: "invalid" };

const OP_LABEL: Record<BooleanOp, string> = { union: "Uniting", difference: "Subtracting", intersection: "Intersecting" };

/**
 * A Boolean of world-space meshes. Union fuses `main` and `tool` together (batch union, faster than pairwise);
 * difference subtracts the union of `tool` from the union of `main`; intersection keeps what the union of `main` and
 * the union of `tool` share. Every input is checked first; if any is not a valid solid, nothing is computed and all of
 * those are listed. Result shells whose volume is below `noiseVolume` (mm³; 0 keeps them all) are dropped.
 *
 * Two things a plain Boolean of the inputs gets wrong are fixed first, per input:
 * - A part whose triangles all face inward is a valid manifold to manifold-3d, which would take it as "everything
 *   outside it". Its volume is negative, so it is turned round (a hollow part's volume is positive: the cavity stays
 *   a cavity).
 * - manifold-3d does not fuse shells of one input that overlap each other (a Merge parts result, a multi-body STL):
 *   it takes them as one self-intersecting solid. Each input is split into its shells, which are fused first (see
 *   fusedBody).
 */
export function booleanMeshes(
  wasm: ManifoldToplevel,
  op: BooleanOp,
  main: readonly MeshData[],
  tool: readonly MeshData[],
  noiseVolume = 0,
  progress: BooleanProgress = () => {},
): BooleanOutcome {
  const owned = new Set<Manifold>();
  const own = <M extends Manifold>(m: M): M => (owned.add(m), m);
  try {
    const meshes = [...main, ...tool];
    // Reading the inputs takes about as long as the operation, roughly in proportion to their triangles.
    const total = meshes.reduce((n, m) => n + m.indices.length, 0) || 1;
    let read = 0;
    const invalid: number[] = [];
    const solids: Manifold[] = [];
    meshes.forEach((mesh, i) => {
      progress((0.45 * read) / total, `Reading part ${i + 1} of ${meshes.length}`);
      read += mesh.indices.length;
      const solid = outwardSolid(wasm, mesh);
      if (solid) solids.push(own(solid));
      else invalid.push(i);
    });
    if (invalid.length > 0) return { kind: "invalid", indices: invalid };
    progress(0.45, "Fusing overlapping shells");
    const fused = solids.map((solid) => fusedBody(wasm, solid, own));
    const bodies = fused.map((f) => f.body);
    const all = (list: Manifold[]) => (list.length === 1 ? list[0] : own(wasm.Manifold.union(list)));
    progress(0.5, OP_LABEL[op]);
    let result: Manifold;
    if (op === "union") result = all(bodies);
    else {
      const kept = all(bodies.slice(0, main.length));
      const other = all(bodies.slice(main.length));
      result = own(op === "difference" ? kept.subtract(other) : kept.intersect(other));
    }
    // status() runs the deferred operation, so the progress below comes after it.
    result.status();
    progress(0.9, "Reading the result");
    const { mesh, noise } = withoutNoise(result, noiseVolume, own);
    return { kind: "ok", mesh, bodies: fused.map((f) => f.count), noise };
  } finally {
    for (const m of owned) m.delete();
  }
}

/**
 * The result's mesh without the shells whose volume (either sign: a cavity counts too) is below `noiseVolume`, and
 * how many were dropped. The kept shells' meshes are joined as they are, which is all composing them would do.
 */
function withoutNoise(result: Manifold, noiseVolume: number, own: <M extends Manifold>(m: M) => M): { mesh: MeshData; noise: number } {
  if (!(noiseVolume > 0) || result.isEmpty()) return { mesh: checkedMesh(result), noise: 0 };
  const shells = result.decompose().map(own);
  const kept = shells.filter((s) => Math.abs(s.volume()) >= noiseVolume);
  const noise = shells.length - kept.length;
  return { mesh: noise === 0 ? checkedMesh(result) : joined(kept.map(checkedMesh)), noise };
}

/** One mesh of several, their vertices renumbered. */
function joined(meshes: MeshData[]): MeshData {
  const positions = new Float32Array(meshes.reduce((n, m) => n + m.positions.length, 0));
  const indices = new Uint32Array(meshes.reduce((n, m) => n + m.indices.length, 0));
  let p = 0;
  let t = 0;
  for (const m of meshes) {
    const base = p / 3;
    positions.set(m.positions, p);
    for (let i = 0; i < m.indices.length; i++) indices[t + i] = m.indices[i] + base;
    p += m.positions.length;
    t += m.indices.length;
  }
  return { positions, indices };
}

/** A Manifold of `data` facing outward overall, or null when it is not a valid solid. */
function outwardSolid(wasm: ManifoldToplevel, data: MeshData): Manifold | null {
  const solid = toManifold(wasm, data);
  if (!solid || solid.volume() >= 0) return solid;
  solid.delete();
  return toManifold(wasm, { positions: data.positions, indices: turnedRound(data.indices) });
}

/** Triangle corners with each triangle's winding reversed (corners 2 and 3 swapped). */
function turnedRound(indices: Uint32Array): Uint32Array {
  const out = indices.slice();
  for (let t = 0; t + 2 < out.length; t += 3) {
    out[t + 1] = indices[t + 2];
    out[t + 2] = indices[t + 1];
  }
  return out;
}

/**
 * One input as the solid it stands for: its outward shells fused, and each cavity (an inward-facing
 * shell inside an outward one's box) cut out of the smallest such shell before the fusing, so another
 * shell overlapping that cavity still fills it. An inward shell inside no other shell is a defect, not
 * a cavity, and is turned round and fused like the rest. `count` is how many separate solids that
 * leaves. Every Manifold made here goes through `own`, which the caller deletes.
 */
function fusedBody(wasm: ManifoldToplevel, solid: Manifold, own: <M extends Manifold>(m: M) => M): { body: Manifold; count: number } {
  const shells = solid.decompose().map(own);
  if (shells.length <= 1) return { body: solid, count: 1 };
  const outer = shells.filter((s) => s.volume() >= 0);
  const holes = new Map<Manifold, Manifold[]>(outer.map((o) => [o, []]));
  for (const shell of shells.filter((s) => s.volume() < 0)) {
    const turned = own(new wasm.Manifold(new wasm.Mesh(turnedMesh(shell))));
    const box = turned.boundingBox();
    const around = outer.filter((o) => contains(o.boundingBox(), box)).sort((a, b) => a.volume() - b.volume())[0];
    if (around) holes.get(around)!.push(turned);
    else outer.push(turned);
  }
  const pieces = outer.map((o) => {
    const cavities = holes.get(o) ?? [];
    return cavities.length ? own(o.subtract(cavities.length === 1 ? cavities[0] : own(wasm.Manifold.union(cavities)))) : o;
  });
  const body = pieces.length === 1 ? pieces[0] : own(wasm.Manifold.union(pieces));
  return { body, count: body.decompose().map(own).length };
}

/** The mesh of a Manifold with its winding reversed, for rebuilding it facing the other way. */
function turnedMesh(m: Manifold): { numProp: number; vertProperties: Float32Array; triVerts: Uint32Array } {
  const mesh = m.getMesh();
  return { numProp: mesh.numProp, vertProperties: mesh.vertProperties, triVerts: turnedRound(mesh.triVerts) };
}

function contains(outer: { min: readonly number[]; max: readonly number[] }, inner: { min: readonly number[]; max: readonly number[] }): boolean {
  return [0, 1, 2].every((a) => outer.min[a] <= inner.min[a] && inner.max[a] <= outer.max[a]);
}

/**
 * Which side of the plane the mesh's vertices lie on, with the Rust clipper's tolerance (ops/cut.rs):
 * a vertex within a few f32 steps of the plane counts as on it. A part whose face lies on the plane up
 * to rounding (its box was worked out in f64, its vertices are f32) is then not crossed, where
 * splitByPlane would cut off a zero-volume sliver.
 */
export function planeSides(mesh: MeshData, plane: CutPlane): { below: boolean; above: boolean } {
  const p = mesh.positions;
  let largest = 1;
  for (let i = 0; i < p.length; i++) largest = Math.max(largest, Math.abs(p[i]));
  const eps = 4 * 2 ** -23 * largest;
  const [nx, ny, nz] = plane.normal;
  let below = false;
  let above = false;
  for (let i = 0; i < p.length && !(below && above); i += 3) {
    const s = nx * p[i] + ny * p[i + 1] + nz * p[i + 2] - plane.offset;
    if (s > eps) above = true;
    else if (s < -eps) below = true;
  }
  return { below, above };
}

/**
 * Cuts a world-space mesh with `plane` into two closed solids. splitByPlane's first result is the side
 * the normal points to ("above", cut 2), the second the side it points away from ("below", cut 1).
 * `gap` > 0 moves `above` by gap × normal, baked into its vertices.
 */
export function splitMesh(wasm: ManifoldToplevel, mesh: MeshData, plane: CutPlane, gap: number): SplitOutcome {
  if (mesh.indices.length === 0) return { kind: "invalid" };
  // Not crossed: the whole part is on one side and stays as it is, whether or not it is a solid.
  const sides = planeSides(mesh, plane);
  if (!sides.above || !sides.below) return { kind: "ok", below: null, above: null };
  const solid = toManifold(wasm, mesh);
  if (!solid) return { kind: "invalid" };
  let pieces: Manifold[] = [];
  try {
    pieces = solid.splitByPlane(plane.normal, plane.offset);
    const [above, below] = pieces;
    const moved = above.isEmpty() ? null : checkedMesh(above);
    if (moved && gap > 0) translate(moved.positions, plane.normal.map((n) => n * gap));
    return { kind: "ok", below: below.isEmpty() ? null : checkedMesh(below), above: moved };
  } finally {
    for (const piece of pieces) piece.delete();
    solid.delete();
  }
}

/**
 * A Manifold of `data`, or null when manifold-3d does not accept it as an oriented 2-manifold (open,
 * non-manifold, out-of-range indices, non-finite coordinates) or it is empty. merge() first welds
 * vertices along open edges that sit within the mesh tolerance of each other, as an STL's near-duplicate
 * corners do. manifold-3d copies the arrays in, so `data` is only read.
 */
function toManifold(wasm: ManifoldToplevel, data: MeshData): Manifold | null {
  if (data.indices.length === 0) return null;
  const mesh = new wasm.Mesh({ numProp: 3, vertProperties: data.positions, triVerts: data.indices });
  mesh.merge();
  let solid: Manifold;
  try {
    solid = new wasm.Manifold(mesh);
  } catch (err) {
    // The wrapper throws a ManifoldError for any bad status; anything else (a wasm abort) is a real failure.
    if (err instanceof Error && err.name === "ManifoldError") return null;
    throw err;
  }
  if (solid.status() !== "NoError" || solid.isEmpty()) {
    solid.delete();
    return null;
  }
  return solid;
}

/**
 * The mesh of a result. status() evaluates the deferred boolean; a bad status here is a failure of the
 * operation, not of an input. Positions only (numProp 3) never need merge vectors, which would mark
 * vertices the caller must weld; both are checked rather than assumed, since this is manifold-3d's output.
 */
function checkedMesh(result: Manifold): MeshData {
  const status = result.status();
  if (status !== "NoError") throw new Error(`manifold-3d reported ${status}`);
  const mesh = result.getMesh();
  if (mesh.numProp !== 3 || (mesh.mergeFromVert?.length ?? 0) > 0) {
    throw new Error("manifold-3d returned a mesh with vertex properties or merge vectors");
  }
  return { positions: ownCopy(mesh.vertProperties), indices: ownCopy(mesh.triVerts) };
}

/**
 * `a` when it is the whole of its own buffer (getMesh() already copies out of wasm memory), else a copy.
 * Never a view into wasm memory: that would change under later operations, and transferring it to the
 * main thread would detach the module's memory.
 */
function ownCopy<T extends Float32Array | Uint32Array>(a: T): T {
  return a.byteOffset === 0 && a.byteLength === a.buffer.byteLength ? a : (a.slice() as T);
}

/** Adds `offset` to every vertex in place: f64 sums, one rounding to f32, like Rust ops::translate. */
function translate(positions: Float32Array, offset: number[]) {
  for (let i = 0; i < positions.length; i += 3) {
    positions[i] += offset[0];
    positions[i + 1] += offset[1];
    positions[i + 2] += offset[2];
  }
}
