// Typed wrapper for the arrange command (src-tauri/src/commands.rs arrange). Rust does all the geometry and answers
// with matrices; nothing here looks at a mesh.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { encodeBundle } from "../io/bundle";
import { opResult, partMeshes, type PartMesh } from "../io/ipc";
import type { PartRotation, Vec3 } from "../store";
import type { ArrangeProgress } from "./types";

/** The Tauri event the arrange command reports its progress with. */
export const ARRANGE_PROGRESS = "arrange-progress";

/** A part going to Rust: the ones the action works on (`target`), and for some actions the ones in the way. */
export interface ArrangePart extends PartMesh {
  target: boolean;
  /** Packing only: the part's priority (0 to 10) and permitted rotation. */
  priority?: number;
  rotation?: PartRotation;
}

/**
 * Runs `action` ("align", "duplicate", "lay-flat", "lay-flat-auto", "min-box", "orient", "pack") with its settings on
 * the parts, and returns Rust's answer. Cancel with cancelRepair(job), which makes this reject with CANCELLED.
 */
export async function runArrange(
  action: string,
  settings: Record<string, unknown>,
  parts: ArrangePart[],
  platform: Vec3,
  job: number,
  onProgress?: (fraction: number) => void,
): Promise<Record<string, unknown>> {
  const unlisten = onProgress
    ? await listen<ArrangeProgress>(ARRANGE_PROGRESS, (event) => {
        if (event.payload.job === job) onProgress(event.payload.fraction);
      })
    : null;
  try {
    const meshes = partMeshes(parts).map((m, i) => {
      const { target, priority, rotation } = parts[i];
      return { ...m, info: { ...m.info, target, ...(priority === undefined ? {} : { priority, rotation }) } };
    });
    const buffer = await invoke<ArrayBuffer>("arrange", encodeBundle({ job, action, platform, ...settings }, meshes));
    return opResult(buffer).meta;
  } finally {
    unlisten?.();
  }
}
