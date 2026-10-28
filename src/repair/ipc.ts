// Typed wrappers for the repair commands in src-tauri/src/commands.rs (the work is in jobs.rs). The working copy
// travels as a bundle (src/io/bundle.ts) with its selection and highlights as extra arrays, never as JSON arrays;
// catalogs, scripts and reports are JSON. Nothing about a repair session is kept in Rust: every run sends the mesh
// and gets the result back.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { decodeBundle, encodeBundle, type ExtraBlob } from "../io/bundle";
import type { MeshStats } from "../io/ipc";
import { primeMeshCaches } from "../meshCache";
import type { MeshData, Vec3 } from "../store";
import type {
  ActionReport,
  Catalog,
  Highlights,
  RepairCheck,
  RepairProgress,
  RepairScript,
  RepairStatus,
  RunResult,
  ScriptStep,
  ShellRow,
  StoredScripts,
} from "./types";

/** What repair_run rejects with when the user cancelled: not an error, and nothing changed. */
export const CANCELLED = "cancelled";

/** The Tauri event repair_run reports its progress with. */
export const PROGRESS_EVENT = "repair-progress";

/** Every action with its parameters, and the built-in scripts (src-tauri/src/repair/defaults.rs). */
export function fetchCatalog(): Promise<Catalog> {
  return invoke<Catalog>("repair_catalog");
}

export interface RunOptions {
  /** Identifies the run for its progress events and for cancelRepair. */
  job: number;
  /** The run's name in progress reports: the action's or the script's. */
  label: string;
  /** Sorted triangle ids of `mesh`; empty for none. */
  selection: Uint32Array;
  /** Also compute the Status tab's numbers and highlights for the result. */
  withStatus: boolean;
  onProgress?: (p: RepairProgress) => void;
}

interface RunMeta {
  changed: boolean;
  selected?: boolean;
  reports?: ActionReport[];
  status?: RepairStatus | null;
  warnings?: string[];
}

interface RunInfo {
  stats?: MeshStats;
}

/**
 * Runs `steps` (disabled ones are skipped) on `mesh` in Rust, off the UI thread. With no steps and `withStatus`
 * this only computes the status (the Update button). Rejects with CANCELLED when cancelRepair(job) stopped it, and
 * with Rust's sentence for any other failure. A changed result's stats and problem edges go into meshCache.
 */
export async function runRepair(mesh: MeshData, steps: ScriptStep[], opts: RunOptions): Promise<RunResult> {
  const { job, label, selection, withStatus, onProgress } = opts;
  // Listening starts before the run so no early progress event is missed.
  const unlisten = onProgress
    ? await listen<RepairProgress>(PROGRESS_EVENT, (event) => {
        if (event.payload.job === job) onProgress(event.payload);
      })
    : null;
  try {
    const request = encodeBundle({ job, label, actions: steps, withStatus }, [
      { info: {}, positions: mesh.positions, indices: mesh.indices, extra: { selection } },
    ]);
    return runResult(mesh, await invoke<ArrayBuffer>("repair_run", request));
  } finally {
    unlisten?.();
  }
}

/**
 * Reads repair_run's answer for the run on `input`. An unchanged result comes without arrays, so its mesh is
 * `input` itself; a changed one is primed in meshCache with its stats and problem edges.
 */
function runResult(input: MeshData, buffer: ArrayBuffer): RunResult {
  const { meta, meshes } = decodeBundle<RunMeta, RunInfo>(buffer);
  const out = meshes[0];
  if (!out) throw new Error("The repair result is missing its mesh.");
  const extra = out.extra;
  const changed = meta.changed === true;
  const mesh = changed ? { positions: out.positions, indices: out.indices } : input;
  const stats = changed ? (out.info.stats ?? null) : null;
  const edges = changed ? out.edges : null;
  if (changed) primeMeshCaches(mesh, { stats, edges });
  const status = meta.status ?? null;
  const highlights: Highlights | null = status
    ? {
        border: f32(extra, "border") ?? new Float32Array(0),
        nonManifold: f32(extra, "nonManifold") ?? new Float32Array(0),
        inverted: u32(extra, "inverted") ?? new Uint32Array(0),
        degenerate: u32(extra, "degenerate") ?? new Uint32Array(0),
      }
    : null;
  return {
    changed,
    mesh,
    stats,
    edges,
    selection: u32(extra, "selection") ?? new Uint32Array(0),
    selected: meta.selected === true,
    reports: meta.reports ?? [],
    status,
    highlights,
    intersecting: u32(extra, "intersecting"),
    warnings: meta.warnings ?? [],
  };
}

function f32(extra: Record<string, ExtraBlob>, name: string): Float32Array | null {
  const blob = extra[name];
  if (blob === undefined) return null;
  if (!(blob instanceof Float32Array)) throw new Error(`The repair result's ${name} array has the wrong type.`);
  return blob;
}

function u32(extra: Record<string, ExtraBlob>, name: string): Uint32Array | null {
  const blob = extra[name];
  if (blob === undefined) return null;
  if (!(blob instanceof Uint32Array)) throw new Error(`The repair result's ${name} array has the wrong type.`);
  return blob;
}

/** Asks the run `job` to stop; it then rejects with CANCELLED. Unknown or finished jobs are ignored. */
export async function cancelRepair(job: number): Promise<void> {
  await invoke("repair_cancel", { job });
}

/** Closed, oriented and the problem counts of each mesh (the parts tree's damage badges), in order. */
export function checkMeshes(meshes: MeshData[]): Promise<RepairCheck[]> {
  const out = meshes.map((mesh, i) => ({ info: { name: String(i) }, positions: mesh.positions, indices: mesh.indices }));
  return invoke<RepairCheck[]>("repair_check", encodeBundle({}, out));
}

/** The Shells tab's rows (in order of first triangle) and the row of each triangle. */
export async function fetchShells(mesh: MeshData): Promise<{ rows: ShellRow[]; shellOf: Uint32Array }> {
  const request = encodeBundle({}, [{ info: {}, positions: mesh.positions, indices: mesh.indices }]);
  const { meta, meshes } = decodeBundle<{ rows?: ShellRow[] }, unknown>(await invoke<ArrayBuffer>("repair_shells", request));
  const shellOf = meshes[0] ? u32(meshes[0].extra, "shellOf") : null;
  if (!shellOf) throw new Error("The shell list came back without the shell of each triangle.");
  return { rows: meta.rows ?? [], shellOf };
}

/** A clicked point of a cut line: where, and the working copy's triangle the click hit. */
export interface CutPointInput {
  at: Vec3;
  triangle: number;
}

/** The cut panel's settings Rust needs (`CutOptions` in src-tauri/src/repair/surface_cut.rs). */
export interface CutLineOptions {
  smooth: boolean;
  /** mm. */
  step: number;
  /** mm. */
  snap: number;
  closed: boolean;
}

/** The routed cut line (repair_cut_path). */
export interface CutPath {
  /** Each piece's points (x, y, z each), or null for a piece that found no way along the surface. */
  pieces: (Float32Array | null)[];
  /** The clicked points each piece joins, by index. */
  spans: [number, number][];
  /** Where each clicked point went: snapped to a vertex or laid on the surface. */
  anchors: Vec3[];
}

/** The `cut_surface` action's parameters, and repair_cut_path's request meta: the same shape. */
export function cutParams(points: readonly CutPointInput[], options: CutLineOptions): Record<string, unknown> {
  return { points: points.map((p) => [p.at[0], p.at[1], p.at[2], p.triangle]), ...options };
}

/** What repair_cut_path and measure_pick reject with when they hold no mesh under the key sent (MESH_NEEDED in jobs.rs). */
export const MESH_NEEDED = "mesh needed";
/** A number per working copy, for Rust's copy of it (CutCache in jobs.rs). Each run of the window starts its numbers
 *  somewhere else, so a number never names a working copy from before a reload. */
const meshKeys = new WeakMap<MeshData, number>();
const keyBase = Math.floor(Math.random() * 2 ** 20) * 2 ** 32;
let lastKey = 0;
/** The working copy Rust was sent last for the cut line. */
let sentKey: number | null = null;

function meshKey(mesh: MeshData): number {
  let key = meshKeys.get(mesh);
  if (key === undefined) {
    key = keyBase + ++lastKey;
    meshKeys.set(mesh, key);
  }
  return key;
}

/**
 * The line Cut surfaces would cut along, routed across the working copy in Rust (off the UI thread), for the
 * viewport to draw. The cut itself (the `cut_surface` action) routes it the same way. The working copy goes to Rust
 * once and stays there for the next lines (forgetCutMesh lets go of it); were it gone, it is sent again.
 */
export async function fetchCutPath(mesh: MeshData, points: readonly CutPointInput[], options: CutLineOptions): Promise<CutPath> {
  const key = meshKey(mesh);
  const empty = { info: {}, positions: new Float32Array(0), indices: new Uint32Array(0) };
  const ask = (send: boolean) =>
    invoke<ArrayBuffer>(
      "repair_cut_path",
      encodeBundle({ ...cutParams(points, options), meshKey: key }, [send ? { info: {}, positions: mesh.positions, indices: mesh.indices } : empty]),
    );
  let answer: ArrayBuffer;
  if (sentKey === key) {
    try {
      answer = await ask(false);
    } catch (err) {
      if (err !== MESH_NEEDED) throw err;
      answer = await ask(true);
    }
  } else {
    answer = await ask(true);
  }
  sentKey = key;
  const { meta, meshes } = decodeBundle<{ pieces?: (number | null)[]; spans?: [number, number][]; anchors?: Vec3[] }, unknown>(answer);
  const path = (meshes[0] && f32(meshes[0].extra, "path")) ?? new Float32Array(0);
  let at = 0;
  const pieces = (meta.pieces ?? []).map((count) => {
    if (count === null) return null;
    const piece = path.subarray(at * 3, (at + count) * 3);
    at += count;
    return piece;
  });
  if (at * 3 !== path.length) throw new Error("The cut line came back damaged.");
  return { pieces, spans: meta.spans ?? [], anchors: meta.anchors ?? [] };
}

/** Lets Rust drop the working copy fetchCutPath left with it (the cutting mode ended). */
export async function forgetCutMesh(): Promise<void> {
  if (sentKey === null) return;
  sentKey = null;
  await invoke("repair_cut_reset");
}

/** The user's scripts and the picker order from the config folder; null before anything was saved. */
export function loadStoredScripts(): Promise<StoredScripts | null> {
  return invoke<StoredScripts | null>("repair_scripts_load");
}

/** Writes the user's scripts and the picker order to the config folder. */
export async function saveStoredScripts(data: StoredScripts): Promise<void> {
  await invoke("repair_scripts_save", { data });
}

/**
 * Shows the open dialog for a script file (ours as .json, or the reference tool's .repair) and reads it. The
 * script has a fresh own id; the warnings name what was skipped. Null when the dialog was cancelled.
 */
export function importScriptFile(): Promise<{ script: RepairScript; warnings: string[] } | null> {
  return invoke<{ script: RepairScript; warnings: string[] } | null>("repair_script_import");
}

/** Shows the save dialog and writes `script` in our export format; false when the dialog was cancelled. */
export function exportScriptFile(script: RepairScript): Promise<boolean> {
  return invoke<boolean>("repair_script_export", { script });
}
