// Data derived from a MeshData, cached per mesh object. Meshes are immutable, so an entry stays
// valid as long as the mesh lives, and an undo that brings a mesh back finds its entry again.
import { isTauri } from "@tauri-apps/api/core";
import { fetchStats, type MeshStats } from "./io/ipc";
import type { MeshData } from "./store";

const stats = new WeakMap<MeshData, MeshStats>();
const pending = new WeakMap<MeshData, Promise<MeshStats | null>>();
const problemEdges = new WeakMap<MeshData, Float32Array>();

/** Stores data computed elsewhere (by the Rust importer) so nothing is computed twice. */
export function primeMeshCaches(mesh: MeshData, data: { stats?: MeshStats | null; edges?: Float32Array | null }) {
  if (data.stats) stats.set(mesh, data.stats);
  if (data.edges) problemEdges.set(mesh, data.edges);
}

export function cachedStats(mesh: MeshData): MeshStats | undefined {
  return stats.get(mesh);
}

/** Problem-edge segments if the importer supplied them; the viewport computes them otherwise. */
export function cachedProblemEdges(mesh: MeshData): Float32Array | undefined {
  return problemEdges.get(mesh);
}

/**
 * The mesh's stats, measured in Rust on first request. Resolves to null outside the desktop app
 * (plain browser during development) or if measuring fails.
 */
export function loadStats(mesh: MeshData): Promise<MeshStats | null> {
  const known = stats.get(mesh);
  if (known) return Promise.resolve(known);
  let request = pending.get(mesh);
  if (!request) {
    request = (isTauri() ? fetchStats([mesh]) : Promise.resolve([]))
      .then(([result]) => {
        if (result) stats.set(mesh, result);
        return result ?? null;
      })
      .catch((err: unknown) => {
        console.error("mesh_stats failed", err);
        return null;
      })
      .finally(() => pending.delete(mesh));
    pending.set(mesh, request);
  }
  return request;
}
