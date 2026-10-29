// Typed wrappers for Generate shell's Rust commands (src-tauri/src/commands.rs shell_generate, shell_estimate). Rust
// does all the geometry; nothing here looks at a mesh.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { encodeBundle } from "../io/bundle";
import { analyzed, opResult, partMeshes, type AnalyzedMesh, type PartMesh } from "../io/ipc";
import type { EstimatePart, ShellEstimate, ShellProgress, ShellRefusal, ShellSettings } from "./types";

/** The Tauri event shell_generate reports its progress with. */
export const SHELL_PROGRESS = "shell-progress";

/** One part's outcome: its new mesh (world space, measured) or null when nothing was left of it, and its warnings. */
export interface ShellResult {
  result: AnalyzedMesh | null;
  warnings: string[];
}

/** Every part was closed and oriented, and these are the results in part order; or some part was not and none ran. */
export type ShellReply = { kind: "made"; results: ShellResult[] } | { kind: "refused"; refused: ShellRefusal[] };

interface Outcome {
  source: number;
  made: boolean;
  warnings: string[];
}

/** Generates a shell of each part. Cancel with cancelRepair(job), which makes this reject with CANCELLED. */
export async function generateShells(
  parts: PartMesh[],
  settings: ShellSettings,
  job: number,
  onProgress?: (p: ShellProgress) => void,
): Promise<ShellReply> {
  const unlisten = onProgress
    ? await listen<ShellProgress>(SHELL_PROGRESS, (event) => {
        if (event.payload.job === job) onProgress(event.payload);
      })
    : null;
  try {
    const { meta, meshes } = opResult(await invoke<ArrayBuffer>("shell_generate", encodeBundle({ job, settings }, partMeshes(parts))));
    if (Array.isArray(meta.refused)) return { kind: "refused", refused: meta.refused as ShellRefusal[] };
    const outcomes = (meta.results ?? []) as Outcome[];
    return {
      kind: "made",
      results: parts.map((_, i) => {
        const outcome = outcomes.find((o) => o.source === i);
        const mesh = meshes.find((m) => m.info.source === i);
        return { result: outcome?.made && mesh ? analyzed(mesh) : null, warnings: outcome?.warnings ?? [] };
      }),
    };
  } finally {
    unlisten?.();
  }
}

/** What a run would take for parts of these sizes. Instant: Rust only does arithmetic. */
export function estimateShells(parts: EstimatePart[], settings: ShellSettings): Promise<ShellEstimate> {
  return invoke<ShellEstimate>("shell_estimate", { parts, settings });
}
