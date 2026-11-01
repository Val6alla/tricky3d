// Typed wrapper for the modify_run command (src-tauri/src/jobs/modify.rs). Rust does the geometry and the checks;
// the window only sends the working copy and its selection, and keeps the answer as a preview until OK.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { decodeBundle, encodeBundle } from "../io/bundle";
import type { MeshStats } from "../io/ipc";
import { primeMeshCaches } from "../meshCache";
import type { MeshData } from "../store";
import type { ModifyProgress, ToolSettings } from "./types";

/** The Tauri event a mesh tool reports its progress with. */
export const MODIFY_PROGRESS = "modify-progress";

export interface ModifyReply {
  changed: boolean;
  /** The result; the input itself when nothing changed. */
  mesh: MeshData;
  /** The input triangle of each result triangle, or null when the triangles are the input's in order. */
  origin: Uint32Array | null;
  message: string;
  warnings: string[];
  before: number;
  after: number;
}

interface Meta {
  changed: boolean;
  message: string;
  warnings: string[];
  before: number;
  after: number;
}

/**
 * Runs `tool` on `mesh` (world space) with the triangles `selection` (sorted ids; empty for none). Cancel with
 * cancelRepair(job), which makes this reject with CANCELLED; any other failure rejects with Rust's sentence.
 */
export async function runModify(
  mesh: MeshData,
  tool: ToolSettings,
  selection: Uint32Array,
  job: number,
  onProgress?: (p: ModifyProgress) => void,
): Promise<ModifyReply> {
  const unlisten = onProgress
    ? await listen<ModifyProgress>(MODIFY_PROGRESS, (event) => {
        if (event.payload.job === job) onProgress(event.payload);
      })
    : null;
  try {
    const request = encodeBundle({ job, tool }, [{ info: {}, positions: mesh.positions, indices: mesh.indices, extra: { selection } }]);
    return modifyReply(mesh, await invoke<ArrayBuffer>("modify_run", request));
  } finally {
    unlisten?.();
  }
}

/** Rust's answer for a run on `input`. A changed result is primed in meshCache with its stats and problem edges. */
export function modifyReply(input: MeshData, buffer: ArrayBuffer): ModifyReply {
  const { meta, meshes } = decodeBundle<Meta, { stats?: MeshStats }>(buffer);
  const out = meshes[0];
  if (!out) throw new Error("The mesh tool's result is missing its mesh.");
  const changed = meta.changed === true;
  const mesh = changed ? { positions: out.positions, indices: out.indices } : input;
  if (changed) primeMeshCaches(mesh, { stats: out.info.stats ?? null, edges: out.edges });
  const origin = out.extra.origin;
  return {
    changed,
    mesh,
    origin: changed && origin instanceof Uint32Array ? origin : null,
    message: typeof meta.message === "string" ? meta.message : "",
    warnings: Array.isArray(meta.warnings) ? meta.warnings.filter((w): w is string => typeof w === "string") : [],
    before: Number(meta.before) || 0,
    after: Number(meta.after) || 0,
  };
}
