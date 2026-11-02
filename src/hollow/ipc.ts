// Typed wrappers for Hollow's Rust commands (src-tauri/src/commands.rs hollow_*). Rust does all the geometry: the
// cavity, the holes' checks and shapes, and the cut; the window sends the part and the holes' places.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { decodeBundle, encodeBundle } from "../io/bundle";
import { analyzed, opResult, type AnalyzedMesh } from "../io/ipc";
import type { MeshData } from "../store";
import type { Hole, HoleCheck, HoleShape, HollowProgress, HollowRefusal, HollowSettings, ThinWalls } from "./types";

export const HOLLOW_PROGRESS = "hollow-progress";

/** The cavity for the part, or why the part was refused. */
export type PreviewReply =
  | { kind: "refused"; refused: HollowRefusal }
  | { kind: "made"; key: number; cavity: MeshData | null; partVolume: number; cavityVolume: number; thin: ThinWalls | null; warnings: string[] };

async function withProgress<T>(job: number, onProgress: ((p: HollowProgress) => void) | undefined, run: () => Promise<T>): Promise<T> {
  const unlisten = onProgress
    ? await listen<HollowProgress>(HOLLOW_PROGRESS, (event) => {
        if (event.payload.job === job) onProgress(event.payload);
      })
    : null;
  try {
    return await run();
  } finally {
    unlisten?.();
  }
}

/** Works out the cavity of `part` (world space) for `settings`; Rust keeps it under `key`. Cancel with cancelRepair(job). */
export function previewHollow(
  part: MeshData,
  settings: HollowSettings,
  job: number,
  key: number,
  onProgress?: (p: HollowProgress) => void,
): Promise<PreviewReply> {
  return withProgress(job, onProgress, async () => {
    const buffer = await invoke<ArrayBuffer>("hollow_preview", encodeBundle({ job, key, settings }, [{ info: { name: "part" }, positions: part.positions, indices: part.indices }]));
    const { meta, meshes } = decodeBundle<Record<string, unknown>, Record<string, unknown>>(buffer);
    if (meta.refused) return { kind: "refused", refused: meta.refused as HollowRefusal };
    const m = meshes[0];
    const cavity = m && m.indices.length > 0 ? { positions: m.positions, indices: m.indices } : null;
    return {
      kind: "made",
      key: Number(meta.key),
      cavity,
      partVolume: Number(meta.partVolume) || 0,
      cavityVolume: Number(meta.cavityVolume) || 0,
      thin: (meta.thin as ThinWalls | null) ?? null,
      warnings: Array.isArray(meta.warnings) ? (meta.warnings as string[]) : [],
    };
  });
}

/** The holes checked against the cavity kept under `key` (with `auto`, Auto holes' after them), and each one's solid to draw. */
export async function placeHoles(
  key: number,
  holes: Hole[],
  auto: { count: number; template: Hole } | null = null,
): Promise<{ holes: Hole[]; checks: HoleCheck[]; solids: MeshData[] }> {
  const buffer = await invoke<ArrayBuffer>("hollow_holes", encodeBundle({ key, holes, auto }, []));
  const { meta, meshes } = decodeBundle<{ holes: Hole[]; checks: HoleCheck[] }, Record<string, unknown>>(buffer);
  return {
    holes: meta.holes ?? [],
    checks: meta.checks ?? [],
    solids: meshes.map((m) => ({ positions: m.positions, indices: m.indices })),
  };
}

/** What Apply made: the hollow part with its holes, a plug per hole when asked, and the holes left out. */
export interface ApplyReply {
  mesh: AnalyzedMesh;
  plugs: AnalyzedMesh[];
  skipped: { index: number; reason: string }[];
  warnings: string[];
}

/** Makes the hollow part from the preview kept under `key`, with `holes` and, with `plugs` (a clearance), plugs. */
export function applyHollow(
  job: number,
  key: number,
  holes: Hole[],
  plugs: number | null,
  onProgress?: (p: HollowProgress) => void,
): Promise<ApplyReply> {
  return withProgress(job, onProgress, async () => {
    const { meta, meshes } = opResult(await invoke<ArrayBuffer>("hollow_apply", encodeBundle({ job, key, holes, plugs }, [])));
    const [first, ...rest] = meshes;
    if (!first) throw new Error("Hollow's result is missing its mesh.");
    return {
      mesh: analyzed(first),
      plugs: rest.map(analyzed),
      skipped: Array.isArray(meta.skipped) ? (meta.skipped as ApplyReply["skipped"]) : [],
      warnings: Array.isArray(meta.warnings) ? (meta.warnings as string[]) : [],
    };
  });
}

/** The panel closed: Rust forgets the preview. */
export function forgetHollow(): Promise<void> {
  return invoke("hollow_close");
}

/** A hole's place with the shape `shape`. */
export function holeAt(at: [number, number, number], normal: [number, number, number], shape: HoleShape): Hole {
  return { at, normal, ...shape };
}
