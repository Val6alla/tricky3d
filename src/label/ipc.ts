// Typed wrappers for label mode's Rust commands (src-tauri/src/commands.rs label_*). Rust reads the fonts, turns text
// and pictures into shapes and puts the labels into the part; the window places them and draws what Rust sends back.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { decodeBundle, encodeBundle, type OutMesh } from "../io/bundle";
import { partMeshes, type AnalyzedMesh, type MeshStats, type PartMesh } from "../io/ipc";
import type { Picture } from "../library/ipc";
import type { Vec3 } from "../store";
import type { FontInfo, LabelKind, PlacedLabel } from "./types";
import { toSpec } from "./types";

/** A label's flat shape at unit height, centred: triangles and outline segments, x, y per point. */
export interface ShapeReply {
  /** Width ÷ height of the label's frame. */
  aspect: number;
  fill: number[];
  outline: number[];
}

/** What a shape depends on. A list's shape is its widest entry's. */
export interface ShapeRequest {
  kind: LabelKind;
  text: string;
  entries: string[];
  font: string;
  fontSize: number;
  threshold: number;
}

/** `Fit` in src-tauri/src/label/fit.rs: a new label's starting height there, and the part's thickness under it. */
export interface FitReply {
  height: number;
  /** False when not even 1 mm fits: the label starts at 1 mm and runs over an edge. */
  fits: boolean;
  thickness: number | null;
}

/** `LabelProgress` in src-tauri/src/jobs.rs. */
export interface LabelProgress {
  job: number;
  fraction: number;
}

export const LABEL_PROGRESS = "label-progress";

/** A CSV file picked for a list label. */
export interface CsvFile {
  name: string;
  rows: string[][];
}

/** The bundled font, then the ones installed on this computer. */
export function fetchFonts(): Promise<FontInfo[]> {
  return invoke<FontInfo[]>("label_fonts");
}

function pictureMesh(picture: Picture, info: Record<string, unknown> = {}): OutMesh {
  return {
    info: { ...info, picture: true, width: picture.width, height: picture.height },
    positions: new Float32Array(0),
    indices: new Uint32Array(0),
    extra: { gray: picture.gray },
  };
}

/** The shape to draw for a label. Rejects with Rust's sentence (no caption, letters the font lacks, no picture). */
export function labelShape(request: ShapeRequest, picture: Picture | null): Promise<ShapeReply> {
  return invoke<ShapeReply>("label_shape", encodeBundle({ ...request }, picture ? [pictureMesh(picture)] : []));
}

/** How tall a label of `request` starts at `point` (world) facing `normal` on `part`, turned `angle` degrees. */
export function fitLabel(part: PartMesh, request: ShapeRequest, at: { point: Vec3; normal: Vec3; angle: number }, picture: Picture | null): Promise<FitReply> {
  const meshes = partMeshes([part]);
  if (picture) meshes.push(pictureMesh(picture));
  return invoke<FitReply>("label_fit", encodeBundle({ ...request, ...at }, meshes));
}

/** One labelled copy: its mesh (world space), stats and problem edges, and its list entries ("" without a list). */
export interface LabelledCopy extends AnalyzedMesh {
  name: string;
}

/**
 * Every label put on `part`, one after the other. `widths` are the labels' widths in mm (their height × the shape's
 * aspect while Keep aspect ratio is on). Rejects with Rust's sentence when a label cannot go where it is, and with
 * CANCELLED after cancelRepair(job).
 */
export async function applyLabels(
  part: PartMesh,
  labels: PlacedLabel[],
  widths: number[],
  job: number,
  onProgress: (fraction: number) => void,
): Promise<{ copies: LabelledCopy[]; note: string | null }> {
  const meshes: OutMesh[] = partMeshes([part]);
  labels.forEach((label, i) => {
    if (label.kind === "image" && label.picture) meshes.push(pictureMesh(label.picture, { label: i }));
  });
  const unlisten = await listen<LabelProgress>(LABEL_PROGRESS, (event) => {
    if (event.payload.job === job) onProgress(event.payload.fraction);
  });
  try {
    const specs = labels.map((label, i) => toSpec(label, widths[i]));
    const buffer = await invoke<ArrayBuffer>("label_apply", encodeBundle({ labels: specs, job }, meshes));
    const bundle = decodeBundle<{ note?: string | null }, { name?: string; stats?: MeshStats }>(buffer);
    const copies = bundle.meshes.map((m) => {
      if (!m.info.stats || !m.edges) throw new Error("The labelled part's measurements are missing");
      return { name: m.info.name ?? "", mesh: { positions: m.positions, indices: m.indices }, stats: m.info.stats, edges: m.edges };
    });
    return { copies, note: bundle.meta.note ?? null };
  } finally {
    unlisten();
  }
}

/** The open dialog for a list's CSV file (Rust reads it); null when cancelled. */
export function pickCsv(): Promise<CsvFile | null> {
  return invoke<CsvFile | null>("label_pick_csv");
}
