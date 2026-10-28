// Typed wrappers for Measure mode's Rust commands (src-tauri/src/commands.rs measure_*). Rust does all the geometry;
// the window sends the click and the part's mesh, once per mesh: Rust keeps a few under a number (MeasureCache).
import { invoke } from "@tauri-apps/api/core";
import { encodeBundle } from "../io/bundle";
import type { PlaneSpec } from "../clip";
import { MESH_NEEDED } from "../repair/ipc";
import type { MeshData, Vec3 } from "../store";
import type { MeasureFeature, MeasureOp, MeasureResult, PickKind, SnapMode } from "./types";

export interface PickRequest {
  kind: PickKind;
  mesh: MeshData;
  /** The clicked part's matrix, and the matrix of the part the measurement belongs to. */
  matrix: readonly number[];
  owner: readonly number[];
  triangle: number;
  /** World space. */
  hit: Vec3;
  snap: SnapMode;
  /** World mm: how far a corner or edge may be from the click and still be snapped to. */
  tolerance: number;
  clip: readonly PlaneSpec[];
}

export interface PickReply {
  /** In the owning part's frame. */
  feature: MeasureFeature;
  world: MeasureFeature;
  /** A surface's outline, world space, x, y, z, x, y, z per segment. */
  outline: number[];
}

const keys = new WeakMap<MeshData, number>();
let keySeq = 0;
/** The meshes Rust holds, by key, newest last (it keeps four). */
let sent: number[] = [];

function meshKey(mesh: MeshData): number {
  let key = keys.get(mesh);
  if (key === undefined) keys.set(mesh, (key = ++keySeq));
  return key;
}

/** What a click picks. Rejects with Rust's sentence (no round edge there, no wall behind it...). */
export async function measurePick(r: PickRequest): Promise<PickReply> {
  const key = meshKey(r.mesh);
  const meta = { meshKey: key, kind: r.kind, triangle: r.triangle, hit: r.hit, matrix: [...r.matrix], owner: [...r.owner], snap: r.snap, tolerance: r.tolerance, clip: r.clip };
  const empty = { info: {}, positions: new Float32Array(0), indices: new Uint32Array(0) };
  const ask = (send: boolean) =>
    invoke<PickReply>("measure_pick", encodeBundle(meta, [send ? { info: {}, positions: r.mesh.positions, indices: r.mesh.indices } : empty]));
  let reply: PickReply;
  if (sent.includes(key)) {
    try {
      reply = await ask(false);
    } catch (err) {
      if (err !== MESH_NEEDED) throw err;
      reply = await ask(true);
    }
  } else {
    reply = await ask(true);
  }
  sent = [...sent.filter((k) => k !== key), key].slice(-4);
  return reply;
}

/** The 3-point circle or 4-point sphere through point features. */
export function measureFit(kind: "circle3" | "sphere4", points: MeasureFeature[]): Promise<MeasureFeature> {
  return invoke<MeasureFeature>("measure_fit", { kind, points });
}

/** What `op` measures on `features` (part-local) under the part's `matrix`, in world space. */
export function measureEvaluate(op: MeasureOp, features: MeasureFeature[], matrix: readonly number[]): Promise<MeasureResult> {
  return invoke<MeasureResult>("measure_evaluate", { op, features, matrix: [...matrix] });
}

/** Lets Rust drop the meshes it keeps for picking (Measure mode closed). */
export async function forgetMeasureMeshes(): Promise<void> {
  sent = [];
  await invoke("measure_reset");
}
