// Typed wrappers for the Part library's Rust commands (src-tauri/src/commands.rs library_catalog, library_primitive).
// Rust builds every mesh; the window only sends the numbers typed into the form.
import { invoke } from "@tauri-apps/api/core";
import { decodeBundle } from "../io/bundle";
import type { AnalyzedMesh, MeshStats } from "../io/ipc";

/** A parameter of a primitive (src-tauri/src/library.rs Param). */
export interface LibraryParam {
  key: string;
  label: string;
  /** "mm", "°" or "" (a count). */
  unit: string;
  default: number;
  min: number;
  max: number;
  /** Whole numbers only. */
  integer: boolean;
}

/** A primitive of the catalog (src-tauri/src/library.rs Primitive). */
export interface LibraryPrimitive {
  id: string;
  label: string;
  params: LibraryParam[];
}

/** Every primitive with its parameters, in the order the library shows them. */
export function fetchLibraryCatalog(): Promise<LibraryPrimitive[]> {
  return invoke<LibraryPrimitive[]>("library_catalog");
}

/**
 * The mesh of primitive `id` with `params`, part-local (centred on the Z axis, resting on z = 0), measured like an
 * import. Rejects with Rust's sentence when a value is out of range, and with CANCELLED after cancelRepair(job).
 */
export async function buildPrimitive(id: string, params: Record<string, number>, job: number): Promise<AnalyzedMesh> {
  const buffer = await invoke<ArrayBuffer>("library_primitive", { id, params, job });
  const [m] = decodeBundle<Record<string, unknown>, { stats?: MeshStats }>(buffer).meshes;
  if (!m?.info.stats || !m.edges) throw new Error("The primitive's measurements are missing");
  return { mesh: { positions: m.positions, indices: m.indices }, stats: m.info.stats, edges: m.edges };
}
