// Typed wrappers for the Rust commands in src-tauri/src/commands.rs. Meshes travel as bundles
// (src/io/bundle.ts), never as JSON arrays.
import { Channel, invoke } from "@tauri-apps/api/core";
import type { CutPlane } from "../ops/types";
import type { MeshData, Vec3 } from "../store";
import { decodeBundle, encodeBundle, type InMesh, type OutMesh } from "./bundle";

export type ImportFormat = "stl" | "obj" | "ply" | "3mf" | "amf" | "off";
export type ExportFormat = "stl-binary" | "stl-ascii" | "obj" | "ply" | "3mf" | "amf";

/** Mesh measurements in part-local millimetres, computed in Rust. */
export interface MeshStats {
  triangles: number;
  vertices: number;
  /** mm³, signed-tetrahedron sum (positive for an outward-facing closed mesh). */
  volume: number;
  /** mm². */
  area: number;
  /** Triangles joined across edges with exactly two triangles facing the same way, as the reference tool counts shells. */
  shells: number;
  /** Edges used by exactly one triangle. */
  openEdges: number;
  /** Edges used by three or more triangles. */
  nonManifoldEdges: number;
}

export interface ImportProgress {
  stage: "read" | "parse" | "weld" | "analyze";
  /** 0..1 over the whole import. */
  fraction: number;
}

/** A mesh read from a file, with its part fields. */
export interface LoadedMesh {
  name: string;
  /** Column-major local-to-world matrix; identity when the file has none. */
  matrix: number[] | null;
  mesh: MeshData;
  stats: MeshStats | null;
  /** Problem-edge segments for the viewport, computed in Rust. */
  edges: Float32Array | null;
  // Project files only.
  color?: string;
  visible?: boolean;
  transparent?: boolean;
  locked?: boolean;
  /** Measure mode's measurements, as the project file holds them (JSON); checked by src/measure/persist.ts. */
  measurements?: string;
}

export interface ImportResult {
  format: ImportFormat;
  /** Objects in the file before any merging. */
  objectCount: number;
  /** True when the file has several objects that `merge` would join (OBJ only). */
  splittable: boolean;
  warnings: string[];
  meshes: LoadedMesh[];
}

export interface ProjectResult {
  /** From the tricky3d:platform metadata; null for a 3MF written by another program. */
  platform: Vec3 | null;
  warnings: string[];
  meshes: LoadedMesh[];
}

/** A part going to a file. The matrix is baked into the vertices on export and kept as-is in a project. */
export interface MeshOut {
  name: string;
  matrix: number[];
  mesh: MeshData;
  color?: string;
  visible?: boolean;
  transparent?: boolean;
  locked?: boolean;
  /** JSON of the part's measurements (projects only). */
  measurements?: string;
}

interface MeshInfo {
  name: string;
  matrix?: number[];
  stats?: MeshStats;
  color?: string;
  visible?: boolean;
  transparent?: boolean;
  locked?: boolean;
  measurements?: string;
}

function loadedMeshes(buffer: ArrayBuffer) {
  return decodeBundle<Record<string, unknown>, MeshInfo>(buffer);
}

function toLoaded(m: { info: MeshInfo; positions: Float32Array; indices: Uint32Array; edges: Float32Array | null }): LoadedMesh {
  const { name, matrix, stats, ...rest } = m.info;
  return { ...rest, name, matrix: matrix ?? null, stats: stats ?? null, edges: m.edges, mesh: { positions: m.positions, indices: m.indices } };
}

function progressChannel(onProgress?: (p: ImportProgress) => void): Channel<ImportProgress> {
  const channel = new Channel<ImportProgress>();
  if (onProgress) channel.onmessage = onProgress;
  return channel;
}

/**
 * Reads a mesh file on a background thread and welds it. With `merge` an OBJ with several objects
 * becomes one mesh; otherwise each object is its own mesh. Rejects with a message to show the user.
 */
export async function importMesh(path: string, merge: boolean, onProgress?: (p: ImportProgress) => void): Promise<ImportResult> {
  const buffer = await invoke<ArrayBuffer>("import_mesh", { path, merge, onProgress: progressChannel(onProgress) });
  const { meta, meshes } = loadedMeshes(buffer);
  return {
    format: meta.format as ImportFormat,
    objectCount: meta.objectCount as number,
    splittable: meta.splittable as boolean,
    warnings: (meta.warnings as string[] | undefined) ?? [],
    meshes: meshes.map(toLoaded),
  };
}

/** Reads a project (.3mf): parts with names, transforms and display fields, plus the platform size. */
export async function openProjectFile(path: string, onProgress?: (p: ImportProgress) => void): Promise<ProjectResult> {
  const buffer = await invoke<ArrayBuffer>("open_project", { path, onProgress: progressChannel(onProgress) });
  const { meta, meshes } = loadedMeshes(buffer);
  return {
    platform: (meta.platform as Vec3 | null | undefined) ?? null,
    warnings: (meta.warnings as string[] | undefined) ?? [],
    meshes: meshes.map(toLoaded),
  };
}

function outMeshes(meshes: MeshOut[]): OutMesh[] {
  return meshes.map(({ mesh, ...info }) => ({ info, positions: mesh.positions, indices: mesh.indices }));
}

/** Writes the meshes into one file of `format`, each with its matrix baked into the vertices. */
export async function exportMeshes(path: string, format: ExportFormat, meshes: MeshOut[]): Promise<void> {
  await invoke("export_mesh", encodeBundle({ path, format }, outMeshes(meshes)));
}

/** Writes a project 3MF: one object and one build item (with its transform) per part, plus the platform. */
export async function saveProjectFile(path: string, platform: Vec3, meshes: MeshOut[]): Promise<void> {
  await invoke("save_project", encodeBundle({ path, platform }, outMeshes(meshes)));
}

/** Measures meshes in Rust, in the given order. */
export async function fetchStats(meshes: MeshData[]): Promise<MeshStats[]> {
  const out = meshes.map((mesh, i) => ({ info: { name: String(i) }, positions: mesh.positions, indices: mesh.indices }));
  return invoke<MeshStats[]>("mesh_stats", encodeBundle({}, out));
}

// ---------------------------------------------------------------------------------------------
// Mesh operations (src-tauri/src/jobs.rs). Rust bakes each part's matrix into its vertices first and
// returns world-space meshes, measured like an import so the info panel and viewport need no pass.

/** A part going to a mesh operation: its part-local mesh and its matrix. */
export interface PartMesh {
  name: string;
  mesh: MeshData;
  matrix: readonly number[];
}

/** A world-space mesh an operation made, with its stats and problem edges (prime meshCache with both). */
export interface AnalyzedMesh {
  mesh: MeshData;
  stats: MeshStats;
  edges: Float32Array;
}

/** One side of a cut part: "below" is the side the plane normal points away from ("cut 1"). */
export interface CutPiece extends AnalyzedMesh {
  side: "below" | "above";
}

export interface OpInfo {
  source?: number;
  side?: "below" | "above";
  stats?: MeshStats;
}

export function partMeshes(parts: PartMesh[]): OutMesh[] {
  return parts.map(({ name, mesh, matrix }) => ({ info: { name, matrix: [...matrix] }, positions: mesh.positions, indices: mesh.indices }));
}

export function opResult(buffer: ArrayBuffer) {
  return decodeBundle<Record<string, unknown>, OpInfo>(buffer);
}

export function analyzed(m: InMesh<OpInfo>): AnalyzedMesh {
  if (!m.info.stats || !m.edges) throw new Error("The measurements of a result mesh are missing");
  return { mesh: { positions: m.positions, indices: m.indices }, stats: m.info.stats, edges: m.edges };
}

/** Each part's connected shells, largest volume first. `count` 1 means the part has one shell; `shells` is then empty. */
export async function splitShells(parts: PartMesh[]): Promise<{ count: number; shells: AnalyzedMesh[] }[]> {
  const { meta, meshes } = opResult(await invoke<ArrayBuffer>("split_shells", encodeBundle({}, partMeshes(parts))));
  const counts = meta.counts as number[];
  return parts.map((_, i) => ({ count: counts[i], shells: meshes.filter((m) => m.info.source === i).map(analyzed) }));
}

/** One mesh holding every part's triangles as they are (no welding), in the given order. */
export async function mergeMeshes(parts: PartMesh[]): Promise<AnalyzedMesh> {
  const { meshes } = opResult(await invoke<ArrayBuffer>("merge_meshes", encodeBundle({}, partMeshes(parts))));
  return analyzed(meshes[0]);
}

/**
 * Cuts each part with the Rust triangle clipper. With `cap` the closed loops of the outline are filled;
 * `openOutline` tells that some loop was open and stays unfilled. `gap` moves the "above" piece along the normal.
 */
export async function cutMeshes(
  parts: PartMesh[],
  plane: CutPlane,
  cap: boolean,
  gap: number,
): Promise<{ pieces: CutPiece[]; openOutline: boolean }[]> {
  const meta = { normal: plane.normal, offset: plane.offset, cap, gap };
  const result = opResult(await invoke<ArrayBuffer>("cut_meshes", encodeBundle(meta, partMeshes(parts))));
  const open = result.meta.openOutline as boolean[];
  return parts.map((_, i) => ({
    pieces: result.meshes.filter((m) => m.info.source === i).map((m) => ({ ...analyzed(m), side: m.info.side ?? "below" })),
    openOutline: open[i],
  }));
}

/** Stats and problem edges of world-space meshes made elsewhere (the manifold worker), in order. */
export async function analyzeMeshes(meshes: MeshData[]): Promise<{ stats: MeshStats; edges: Float32Array }[]> {
  const out = meshes.map((mesh, i) => ({ info: { name: String(i) }, positions: mesh.positions, indices: mesh.indices }));
  const result = opResult(await invoke<ArrayBuffer>("analyze_meshes", encodeBundle({}, out)));
  return result.meshes.map((m) => {
    const { stats, edges } = analyzed(m);
    return { stats, edges };
  });
}

// ---------------------------------------------------------------------------------------------
// File dialogs. Rust runs them so it knows which paths the user picked: the file commands above
// refuse every path that was not picked in one of these or dropped on the window.

export interface FileFilter {
  name: string;
  extensions: string[];
}

/**
 * The open dialog. Rust sets its format menu: all supported files (selected), then each format, then
 * all files. The files picked, or none when cancelled.
 */
export function pickFiles(options: { title: string; multiple: boolean }): Promise<string[]> {
  return invoke<string[]>("pick_files", options);
}

/**
 * The File › Open dialog (Rust sets its title, and the formats as for pickFiles); the file picked, or
 * null when cancelled. Only a 3MF picked here may be saved back to after it opens as a project; an
 * imported or dropped 3MF stays read-only.
 */
export function pickOpenFile(): Promise<string | null> {
  return invoke<string | null>("pick_open_file");
}

/** The save dialog; the path picked, or null when cancelled. */
export function pickSaveFile(options: { title: string; defaultPath?: string; filters: FileFilter[] }): Promise<string | null> {
  return invoke<string | null>("pick_save_file", options);
}
