// Typed wrappers for the Part library's Rust commands (src-tauri/src/commands.rs library_catalog, library_build,
// library_pick_image). Rust builds every mesh; the window sends the values typed into the form, the selected parts
// for the items made from them, and a picked picture as grey levels, which the WebView decodes.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { decodeBundle, encodeBundle, type OutMesh } from "../io/bundle";
import { partMeshes, type AnalyzedMesh, type MeshStats, type PartMesh } from "../io/ipc";

export type LibraryCategory = "primitive" | "design" | "mathematical" | "quality";

/** An option of a choice parameter; `soon` ones show but cannot be picked. */
export interface LibraryChoice {
  value: string;
  label: string;
  soon: boolean;
}

interface ParamBase {
  key: string;
  label: string;
  /** One sentence for the field's tooltip; may be empty. */
  help: string;
  /** Shown only while the toggle with this key is on, or off when it starts with "!". */
  when: string | null;
}

/** A parameter of a library item (src-tauri/src/library/mod.rs Param). */
export type LibraryParam = ParamBase &
  (
    | { kind: "number"; unit: string; default: number; min: number; max: number; integer: boolean }
    | { kind: "choice"; default: string; options: LibraryChoice[] }
    | { kind: "text"; default: string; maxLength: number }
    | { kind: "toggle"; default: boolean }
    | { kind: "image" }
  );

export type NumberParam = Extract<LibraryParam, { kind: "number" }>;

/** An item of the catalog (src-tauri/src/library/mod.rs Item). */
export interface LibraryItem {
  id: string;
  label: string;
  category: LibraryCategory;
  description: string;
  /** Why it is not available yet; null when it is. */
  soon: string | null;
  /** "parts": made from the selected parts, and added where they are. */
  input: "none" | "parts";
  params: LibraryParam[];
}

/** A picture as grey levels, 0 black to 1 white, row by row from the top. */
export interface Picture {
  name: string;
  width: number;
  height: number;
  gray: Float32Array;
}

/** `LibraryProgress` in src-tauri/src/jobs.rs. */
export interface LibraryProgress {
  job: number;
  fraction: number;
}

/** The Tauri event library_build reports its progress with. */
export const LIBRARY_PROGRESS = "library-progress";

/** Every item with its parameters, in the order the library shows them. */
export function fetchLibraryCatalog(): Promise<LibraryItem[]> {
  return invoke<LibraryItem[]>("library_catalog");
}

/** A made part: its mesh with stats and problem edges, and whether it is in world space (made from the selection). */
export interface BuiltPart extends AnalyzedMesh {
  world: boolean;
}

/**
 * Item `id` built from `params` (numbers, texts and toggles by key). Without `world` its mesh is part-local (centred
 * on the Z axis, resting on z = 0). `preview` builds the tile picture (coarser for field-based items, and from a
 * sample when the item needs a selection). Rejects with Rust's sentence when a value is refused, and with CANCELLED
 * after cancelRepair(job).
 */
export async function buildItem(
  id: string,
  params: Record<string, number | string | boolean>,
  job: number,
  options: { preview?: boolean; parts?: PartMesh[]; picture?: Picture | null; onProgress?: (fraction: number) => void } = {},
): Promise<BuiltPart> {
  const meshes: OutMesh[] = partMeshes(options.parts ?? []);
  const picture = options.picture;
  if (picture) {
    meshes.push({
      info: { picture: true, width: picture.width, height: picture.height },
      positions: new Float32Array(0),
      indices: new Uint32Array(0),
      extra: { gray: picture.gray },
    });
  }
  const onProgress = options.onProgress;
  const unlisten = onProgress
    ? await listen<LibraryProgress>(LIBRARY_PROGRESS, (event) => {
        if (event.payload.job === job) onProgress(event.payload.fraction);
      })
    : null;
  try {
    const buffer = await invoke<ArrayBuffer>("library_build", encodeBundle({ id, params, job, preview: options.preview ?? false }, meshes));
    const [m] = decodeBundle<Record<string, unknown>, { stats?: MeshStats; world?: boolean }>(buffer).meshes;
    if (!m?.info.stats || !m.edges) throw new Error("The part's measurements are missing");
    return { mesh: { positions: m.positions, indices: m.indices }, stats: m.info.stats, edges: m.edges, world: m.info.world === true };
  } finally {
    unlisten?.();
  }
}

/** Largest side a picked picture is decoded at; Rust samples it down further to the item's detail. */
export const PICTURE_MAX_SIDE = 1024;

/**
 * The picture dialog (Rust shows it and reads the file); the picture decoded to grey levels, or null when the dialog
 * is cancelled. Rejects with a sentence when the file cannot be read or decoded.
 */
export async function pickPicture(): Promise<Picture | null> {
  const buffer = await invoke<ArrayBuffer>("library_pick_image");
  if (buffer.byteLength === 0) return null;
  const nameLength = new DataView(buffer).getUint32(0, true);
  const name = new TextDecoder().decode(new Uint8Array(buffer, 4, nameLength));
  const bytes = new Uint8Array(buffer, 4 + nameLength);
  return decodePicture(name, bytes);
}

/** Decodes an image file's bytes in the WebView to grey levels (transparent pixels count as white). */
export async function decodePicture(name: string, bytes: Uint8Array): Promise<Picture> {
  let bitmap: ImageBitmap;
  try {
    bitmap = await createImageBitmap(new Blob([bytes as Uint8Array<ArrayBuffer>]));
  } catch {
    throw `Could not read "${name}": it is not a picture tricky3D can open (PNG, JPEG, BMP, GIF or WebP).`;
  }
  const scale = Math.min(1, PICTURE_MAX_SIDE / Math.max(bitmap.width, bitmap.height));
  const width = Math.max(1, Math.round(bitmap.width * scale));
  const height = Math.max(1, Math.round(bitmap.height * scale));
  const canvas = new OffscreenCanvas(width, height);
  const context = canvas.getContext("2d");
  if (!context) throw `Could not read "${name}": the picture could not be drawn.`;
  context.fillStyle = "#ffffff";
  context.fillRect(0, 0, width, height);
  context.drawImage(bitmap, 0, 0, width, height);
  bitmap.close();
  return { name, width, height, gray: grayLevels(context.getImageData(0, 0, width, height).data) };
}

/** RGBA bytes to grey levels 0..1 (Rec. 709 luma of the sRGB values). */
export function grayLevels(rgba: Uint8ClampedArray): Float32Array {
  const out = new Float32Array(rgba.length / 4);
  for (let i = 0; i < out.length; i++) {
    out[i] = (0.2126 * rgba[4 * i] + 0.7152 * rgba[4 * i + 1] + 0.0722 * rgba[4 * i + 2]) / 255;
  }
  return out;
}

export type { PartMesh };
