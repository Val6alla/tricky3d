// Typed wrapper for the analyze_part command (src-tauri/src/jobs/analysis.rs). Rust does all the geometry and
// answers with the numbers and what the view draws; nothing here looks at a mesh.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { decodeBundle, encodeBundle } from "../io/bundle";
import { partMeshes, type PartMesh } from "../io/ipc";
import type { AnalysisDrawing, AnalysisKind, AnalysisProgress, AnalysisSettings, CompareSurface } from "./types";

/** The Tauri event an analysis reports its progress with. */
export const ANALYSIS_PROGRESS = "analysis-progress";

export interface AnalysisReply {
  result: unknown;
  drawing: AnalysisDrawing;
}

/**
 * Runs the analysis `kind` with its settings on the parts and returns Rust's answer. Cancel with cancelRepair(job),
 * which makes this reject with CANCELLED.
 */
export async function runAnalysis(
  kind: AnalysisKind | "collisions" | "interlock",
  settings: AnalysisSettings | { minDistance: number },
  parts: PartMesh[],
  job: number,
  onProgress?: (fraction: number) => void,
): Promise<AnalysisReply> {
  const unlisten = onProgress
    ? await listen<AnalysisProgress>(ANALYSIS_PROGRESS, (event) => {
        if (event.payload.job === job) onProgress(event.payload.fraction);
      })
    : null;
  try {
    const buffer = await invoke<ArrayBuffer>("analyze_part", encodeBundle({ job, kind, settings }, partMeshes(parts)));
    return analysisReply(buffer);
  } finally {
    unlisten?.();
  }
}

/** Rust's answer: the result, and the drawing its one mesh carries (src-tauri/src/jobs/analysis.rs encode). */
export function analysisReply(buffer: ArrayBuffer): AnalysisReply {
  const { meta, meshes } = decodeBundle<{ result: unknown }, { surface?: string }>(buffer);
  const [carrier, ...rest] = meshes;
  const drawing: AnalysisDrawing = {};
  if (carrier) {
    if (carrier.positions.length > 0) drawing.triangles = carrier.positions;
    const classes = carrier.extra.classes;
    if (classes) drawing.classes = Uint8Array.from(classes);
    const lines = carrier.extra.lines;
    if (lines instanceof Float32Array) drawing.lines = lines;
    const values = carrier.extra.values;
    if (values instanceof Float32Array) drawing.values = values;
  }
  const surfaces = rest.flatMap((m): CompareSurface[] => {
    const values = m.extra.values;
    const role = m.info.surface;
    if (!(values instanceof Float32Array) || (role !== "reference" && role !== "comparison")) return [];
    return [{ role, positions: m.positions, indices: m.indices, values }];
  });
  if (surfaces.length > 0) drawing.surfaces = surfaces;
  return { result: meta.result, drawing };
}
