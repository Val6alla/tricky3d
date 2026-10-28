// Typed wrappers for Platform statistics' Rust commands (src-tauri/src/commands.rs platform_stats, export_text).
// Rust measures every part; nothing here looks at a mesh.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { encodeBundle } from "../io/bundle";
import type { MeshData, Vec3 } from "../store";

/** The Tauri event platform_stats reports its progress with. */
export const STATS_PROGRESS = "platform-stats-progress";

/** One row of the parts table (src-tauri/src/stats.rs PartStats), world space. */
export interface PartStats {
  triangles: number;
  edges: number;
  points: number;
  shells: number;
  /** mm³; null when the mesh has holes. */
  volume: number | null;
  /** mm². */
  area: number;
  min: Vec3 | null;
  max: Vec3 | null;
  closed: boolean;
  oriented: boolean;
  holes: number;
  borderEdges: number;
  nonManifoldEdges: number;
  invertedTriangles: number;
  degenerateTriangles: number;
}

/** The Information section for a set of parts (src-tauri/src/stats.rs Summary). */
export interface StatsSummary {
  parts: number;
  min: Vec3 | null;
  max: Vec3 | null;
  /** Parts of the set that lie wholly on the platform: the ones the totals count. */
  onPlatform: number;
  /** mm³; null when a counted part has holes. */
  volume: number | null;
  /** mm². */
  area: number;
  /** Percent. */
  packingDensity: number | null;
  withObjectHeight: number | null;
  /** Height of the highest counted part (mm). */
  top: number | null;
}

export interface StatsReply {
  /** In the order the parts were sent. */
  parts: PartStats[];
  all: StatsSummary;
  selected: StatsSummary;
}

interface StatsProgress {
  job: number;
  fraction: number;
}

/** A part going to platform_stats: its part-local mesh and its matrix. */
export interface StatsPart {
  name: string;
  mesh: MeshData;
  matrix: readonly number[];
}

/**
 * Measures `parts` in Rust for the platform `platform`; `selected` are indices into `parts`. Cancel with
 * cancelRepair(job), which makes this reject with CANCELLED.
 */
export async function fetchPlatformStats(
  parts: StatsPart[],
  platform: Vec3,
  selected: number[],
  job: number,
  onProgress?: (fraction: number) => void,
): Promise<StatsReply> {
  const unlisten = onProgress
    ? await listen<StatsProgress>(STATS_PROGRESS, (event) => {
        if (event.payload.job === job) onProgress(event.payload.fraction);
      })
    : null;
  try {
    const meshes = parts.map(({ name, mesh, matrix }) => ({ info: { name, matrix: [...matrix] }, positions: mesh.positions, indices: mesh.indices }));
    return await invoke<StatsReply>("platform_stats", encodeBundle({ job, platform, selected }, meshes));
  } finally {
    unlisten?.();
  }
}

/** Writes `text` to `path`, a file picked in the save dialog (Export to CSV). */
export async function exportText(path: string, text: string): Promise<void> {
  await invoke("export_text", { path, text });
}
