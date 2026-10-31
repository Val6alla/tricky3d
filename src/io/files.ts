// File › Import, Export, Open and Save. The Rust commands in src/io/ipc.ts read and write the files;
// this module runs the dialogs, prompts and status-bar tasks around them. The decisions (formats,
// file names, placement, the import sequence) are plain exported functions so tests can run them
// without Tauri.
import { isTauri } from "@tauri-apps/api/core";
import { ask, message } from "@tauri-apps/plugin-dialog";
import { boundsOf, IDENTITY, unionBounds, type Bounds } from "../mesh";
import { primeMeshCaches } from "../meshCache";
import { closeRepair } from "../repair/session";
import {
  DEFAULT_PACKING,
  isDirty,
  targetIds,
  useStore,
  type AppState,
  type ExportOptions,
  type ImportPlacement,
  type MeshData,
  type NewPart,
  type Part,
  type Vec3,
} from "../store";
import { prompt, startTask, toast, type ObjectsAnswer, type PlaceAnswer, type TaskHandle } from "../ui";
import { readMeasurements } from "../measure/persist";
import { fromBase64, readAnalyses, toBase64, writeAnalyses } from "../analysis/persist";

/** Triangle colours a project keeps for a part with `bytes` colour bytes (3 per triangle); undefined when they do not fit. */
function readColors(text: string | undefined, bytes: number): Uint8Array | undefined {
  const colors = text ? fromBase64(text) : null;
  return colors && colors.length === bytes ? colors : undefined;
}
import { viewport } from "../viewport/api";
import {
  exportEach,
  exportMeshes,
  importMesh,
  openProjectFile,
  pickExportFile,
  pickFiles,
  pickOpenFile,
  pickSaveFile,
  saveProjectFile,
  type EachResult,
  type ExportFormat,
  type ExportPick,
  type ExportType,
  type FileFilter,
  type ImportFormat,
  type ImportProgress,
  type ImportResult,
  type LoadedMesh,
  type MeshOut,
  type ProjectResult,
} from "./ipc";

// ---------------------------------------------------------------------------------------------
// Formats

/** Import formats in menu order; each is also its file extension. */
export const IMPORT_FORMATS: readonly ImportFormat[] = ["stl", "obj", "ply", "3mf", "amf", "off"];

const SUPPORTED_TEXT = "STL, OBJ, PLY, 3MF, AMF and OFF";

/** Export's types in the order its dialog lists them (src-tauri/src/save_dialog.rs); each is also its extension. */
export const EXPORT_TYPES: readonly ExportType[] = ["stl", "3mf", "obj", "ply", "amf", "off"];

const PROJECT_FILTERS: FileFilter[] = [{ name: "tricky3D project (*.3mf)", extensions: ["3mf"] }];

// ---------------------------------------------------------------------------------------------
// Paths and names (pure)

/** File name with its extension, from a Windows or POSIX path. */
export function fileName(path: string): string {
  return path.slice(Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\")) + 1);
}

/** Lower-case extension without the dot, or "" when the name has none. */
export function extensionOf(path: string): string {
  const name = fileName(path);
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
}

/** File name without its extension. */
export function fileStem(path: string): string {
  const name = fileName(path);
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(0, dot) : name;
}

/** `name` in the same folder as `reference`, or `name` alone when there is no reference. */
export function besidePath(reference: string | null, name: string): string {
  return reference ? folderOf(reference) + name : name;
}

/** The folder of a file path with its trailing separator, or "" for a bare name. */
export function folderOf(path: string): string {
  return path.slice(0, path.length - fileName(path).length);
}

/** `name` inside `folder` (a folder path, with or without its trailing separator), or `name` alone without one. */
export function inFolder(folder: string | null, name: string): string {
  if (!folder) return name;
  return /[\\/]$/.test(folder) ? folder + name : `${folder}${folder.includes("\\") ? "\\" : "/"}${name}`;
}

/** The import format for a path, by extension; null when tricky3D cannot read it. */
export function importFormatOf(path: string): ImportFormat | null {
  const ext = extensionOf(path);
  return IMPORT_FORMATS.find((f) => f === ext) ?? null;
}

/** `path` with `.ext` added unless it already ends in it (in any case). */
export function withExtension(path: string, ext: string): string {
  return extensionOf(path) === ext ? path : `${path}.${ext}`;
}

/**
 * Path and format for a file picked in the Export dialog with the type `selected`. A name ending in one of the types'
 * extensions is that type; any other name gets the selected type's extension added. STL is written as `stl` says.
 */
export function exportTarget(path: string, selected: ExportType, stl: ExportOptions["stl"]): { path: string; format: ExportFormat } {
  const named = EXPORT_TYPES.find((t) => t === extensionOf(path));
  const type = named ?? selected;
  return { path: named ? path : `${path}.${selected}`, format: type === "stl" ? (stl === "ascii" ? "stl-ascii" : "stl-binary") : type };
}

const RESERVED_NAME = /^(con|prn|aux|nul|com[1-9]|lpt[1-9])$/i;

/**
 * A file name that is valid on Windows, macOS and Linux: characters those systems forbid become
 * "_", trailing dots and spaces go (Windows drops them), and device names such as "CON" get a "_".
 */
export function sanitizeFileName(name: string, fallback = "Untitled"): string {
  // eslint-disable-next-line no-control-regex
  let out = name.replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").trim().slice(0, 200).replace(/[. ]+$/, "");
  if (RESERVED_NAME.test(out)) out += "_";
  return out || fallback;
}

/** Default export file name (no extension): the part's name for one part, else the project's. */
export function exportBaseName(parts: readonly { name: string }[], projectName: string): string {
  return sanitizeFileName(parts.length === 1 ? parts[0].name : projectName);
}

/** The file the parts came from: the source of the first of them (in the order given) that has one, or null. */
export function sourceOf(parts: readonly { source?: string }[]): string | null {
  return parts.find((p) => p.source)?.source ?? null;
}

/** Where a save dialog opens: the path it suggests (folder and name), and the folder to use when that folder is gone. */
export interface DialogDefault {
  defaultPath: string;
  fallbackFolder?: string;
}

/**
 * The type Export's dialog selects for parts: the format of the file they came from when tricky3D writes it, else STL
 * (also for parts that never came from a file, such as Part library parts). The name it is saved with decides in the
 * end (exportTarget).
 */
export function exportType(parts: readonly { source?: string }[]): ExportType {
  const source = sourceOf(parts);
  const ext = source ? extensionOf(source) : "";
  return EXPORT_TYPES.find((t) => t === ext) ?? "stl";
}

/**
 * The save dialog of an export of `parts` as `.ext`. Parts that came from a file (or were made from one) suggest that
 * file's own name, never a suffix such as "(cut 1)", in the folder it was imported from, even when the last export
 * went elsewhere; should that folder be gone, the last folder used. Parts that never came from a file suggest their
 * name (one part) or the project's, in the last folder used.
 */
export function exportDialogDefault(
  parts: readonly { name: string; source?: string }[],
  ext: string,
  projectName: string,
  lastFolder: string | null,
): DialogDefault {
  const source = sourceOf(parts);
  if (source) return { defaultPath: `${folderOf(source)}${fileStem(source)}.${ext}`, fallbackFolder: lastFolder ?? undefined };
  return { defaultPath: inFolder(lastFolder, `${exportBaseName(parts, projectName)}.${ext}`) };
}

/**
 * The Export dialog of one file per part: the folder exportDialogDefault opens in, with the first part's own file
 * name, since every part's file is named after the part there.
 */
export function eachDialogDefault(parts: readonly { name: string; source?: string }[], ext: string, lastFolder: string | null): DialogDefault {
  const name = `${sanitizeFileName(parts[0]?.name ?? "")}.${ext}`;
  const source = sourceOf(parts);
  if (source) return { defaultPath: folderOf(source) + name, fallbackFolder: lastFolder ?? undefined };
  return { defaultPath: inFolder(lastFolder, name) };
}

/** A folder path for a message: without its trailing separator. */
function shownFolder(folder: string): string {
  return folder.replace(/[\\/]+$/, "") || folder;
}

/** The question before one file per part replaces files that are there, naming up to three of them. */
export function replaceQuestion(existing: readonly string[], folder: string): string {
  const names = existing.slice(0, 3).map((n) => `"${n}"`).join(", ");
  const more = existing.length > 3 ? ` and ${existing.length - 3} more` : "";
  return existing.length === 1
    ? `${names} is already in "${shownFolder(folder)}". Replace it?`
    : `${names}${more} are already in "${shownFolder(folder)}". Replace them?`;
}

/**
 * The save dialog of Save project (as). Like an export: the parts' original file name with ".3mf", in its folder (else
 * the last folder used); without a source, the project's own file, else its name in the last folder used.
 */
export function saveDialogDefault(
  parts: readonly { source?: string }[],
  projectName: string,
  projectPath: string | null,
  lastFolder: string | null,
): DialogDefault {
  const source = sourceOf(parts);
  if (source) return { defaultPath: `${folderOf(source)}${fileStem(source)}.3mf`, fallbackFolder: lastFolder ?? undefined };
  if (projectPath) return { defaultPath: projectPath, fallbackFolder: lastFolder ?? undefined };
  return { defaultPath: inFolder(lastFolder, `${sanitizeFileName(projectName)}.3mf`) };
}

/** Remembers the folder of a file the user just picked or dropped as the last folder used. */
export function usedFile(path: string): void {
  const folder = folderOf(path);
  if (folder) useStore.getState().setLastFolder(folder);
}

/** Error text for files tricky3D cannot read, naming up to three of them. */
export function unsupportedMessage(paths: readonly string[]): string {
  const names = paths.slice(0, 3).map((p) => `"${fileName(p)}"`).join(", ");
  const more = paths.length > 3 ? ` and ${paths.length - 3} more` : "";
  return `Can't import ${names}${more}. tricky3D reads ${SUPPORTED_TEXT} files; see File › Supported formats.`;
}

/** "1 part", "3 parts". */
export function countParts(n: number): string {
  return `${n} ${n === 1 ? "part" : "parts"}`;
}

/**
 * Text for an error toast. Rust errors are already full sentences naming the file, so they are
 * shown as they are; anything else (a JS exception) is put after `context`.
 */
export function errorMessage(err: unknown, context: string): string {
  if (typeof err === "string" && err.trim()) return err;
  const detail = err instanceof Error ? err.message : String(err);
  return `${context}: ${detail}`;
}

// ---------------------------------------------------------------------------------------------
// Placement and project fields (pure)

/** Translation that rests `box` on the platform (min Z = 0) with its centre over the platform centre. The platform spans 0..size. */
export function placementOffset(box: Bounds, platform: Vec3): Vec3 {
  return [
    platform[0] / 2 - (box.min[0] + box.max[0]) / 2,
    platform[1] / 2 - (box.min[1] + box.max[1]) / 2,
    -box.min[2],
  ];
}

/** Translation × m for a column-major matrix: moves in world space and keeps m's own rotation and scale. */
export function preTranslate(m: readonly number[], [x, y, z]: Vec3): number[] {
  const out = [...m];
  for (let c = 0; c < 4; c++) {
    const w = m[c * 4 + 3];
    out[c * 4] += x * w;
    out[c * 4 + 1] += y * w;
    out[c * 4 + 2] += z * w;
  }
  return out;
}

/**
 * Matrices that place a file's meshes on the platform as one group: one translation for all of
 * them, from their combined world box. Null when the meshes have no vertices.
 */
export function placedMatrices(meshes: readonly { mesh: MeshData; matrix: readonly number[] | null }[], platform: Vec3): number[][] | null {
  const box = unionBounds(meshes.map((m) => boundsOf(m.mesh, m.matrix ?? IDENTITY)));
  if (!box) return null;
  const offset = placementOffset(box, platform);
  return meshes.map((m) => preTranslate(m.matrix ?? IDENTITY, offset));
}

/** Formats without units, which tricky3D reads as millimetres. */
const UNITLESS: ReadonlySet<ImportFormat> = new Set(["stl", "obj", "ply", "off"]);

/** Below this size across (mm), a file without units was probably modelled in metres. */
export const METRES_BELOW = 1;

/**
 * The size across (the longest side of the box round all its meshes) of a file without units that
 * is small enough to have been modelled in metres, or null.
 */
export function metresSize(result: Pick<ImportResult, "format" | "meshes">): number | null {
  if (!UNITLESS.has(result.format)) return null;
  const box = unionBounds(result.meshes.map((m) => boundsOf(m.mesh, m.matrix ?? IDENTITY)));
  if (!box) return null;
  const size = Math.max(box.max[0] - box.min[0], box.max[1] - box.min[1], box.max[2] - box.min[2]);
  return size > 0 && size < METRES_BELOW ? size : null;
}

/** The meshes with every vertex scaled by `factor` about the origin. Files without units have no matrices. */
export function scaledMeshes(meshes: readonly LoadedMesh[], factor: number): LoadedMesh[] {
  return meshes.map((m) => ({
    ...m,
    mesh: { positions: m.mesh.positions.map((v) => v * factor), indices: m.mesh.indices },
    stats: null,
    edges: null,
  }));
}

/** The platform a project file asks for, if it is a usable size; otherwise the current one. */
export function projectPlatform(fromFile: Vec3 | null, current: Vec3): Vec3 {
  const ok = fromFile?.length === 3 && fromFile.every((v) => Number.isFinite(v) && v > 0);
  return ok ? [fromFile[0], fromFile[1], fromFile[2]] : current;
}

/** A part colour read from a file, if it is a plain hex colour; otherwise none (the palette picks one). */
export function fileColor(color: unknown): string | undefined {
  return typeof color === "string" && /^#[0-9a-f]{6}$/i.test(color) ? color : undefined;
}

/** A project mesh as a new part, with the display fields the file stored. */
export function projectPart(m: LoadedMesh): NewPart {
  return {
    name: m.name,
    mesh: m.mesh,
    matrix: m.matrix ?? undefined,
    color: fileColor(m.color),
    visible: m.visible,
    transparent: m.transparent,
    locked: m.locked,
    measurements: readMeasurements(m.measurements),
    analyses: readAnalyses(m.analyses, m.mesh.indices.length / 3),
    colors: readColors(m.colors, m.mesh.indices.length),
    ...(m.priority !== undefined || m.rotation !== undefined
      ? { packing: { priority: m.priority ?? DEFAULT_PACKING.priority, rotation: m.rotation ?? DEFAULT_PACKING.rotation } }
      : {}),
    ...(typeof m.source === "string" && m.source ? { source: m.source } : {}),
  };
}

/** A part as a project file keeps it: projectPart() turns it back into the same part. */
export function projectMesh(p: Part): MeshOut {
  return {
    name: p.name,
    matrix: p.matrix,
    mesh: p.mesh,
    color: p.color,
    visible: p.visible,
    transparent: p.transparent,
    locked: p.locked,
    ...(p.packing ? { priority: p.packing.priority, rotation: p.packing.rotation } : {}),
    ...(p.measurements && p.measurements.length > 0 ? { measurements: JSON.stringify(p.measurements) } : {}),
    ...(p.analyses && p.analyses.length > 0 ? { analyses: writeAnalyses(p.analyses) } : {}),
    ...(p.colors ? { colors: toBase64(p.colors) } : {}),
    ...(p.source ? { source: p.source } : {}),
  };
}

/** What a placement setting means for the next file: place, keep the file's position, or ask. */
export function placementRule(setting: ImportPlacement): boolean | "ask" {
  return setting === "ask" ? "ask" : setting === "always";
}

/** The setting a "Remember my answer" reply stores, or null when the reply is not to be remembered. */
export function rememberedPlacement(answer: PlaceAnswer): ImportPlacement | null {
  if (!answer.remember) return null;
  return answer.place ? "always" : "never";
}

export type DiscardChoice = "save" | "discard" | "cancel";

const SAVE_LABEL = "Save";
const DONT_SAVE_LABEL = "Don't save";

/**
 * The unsaved-changes message box's answer. With custom buttons the dialog plugin returns the
 * pressed button's label; "Yes"/"No" are accepted too in case a platform reports the plain button.
 * Closing the box any other way is Cancel.
 */
export function discardChoice(result: string): DiscardChoice {
  if (result === SAVE_LABEL || result === "Yes") return "save";
  if (result === DONT_SAVE_LABEL || result === "No") return "discard";
  return "cancel";
}

// ---------------------------------------------------------------------------------------------
// Document and save order (pure)

export interface SaveQueue {
  /** Runs `save` after every save asked for before it. `behindAnother` is true when one was still running at the time of asking. */
  add(save: (behindAnother: boolean) => Promise<boolean>): Promise<boolean>;
  /** True while a save is running or waiting for its turn. */
  busy(): boolean;
  /** Resolves (never rejects) once every save asked for so far has finished. */
  idle(): Promise<unknown>;
}

/**
 * Saves run one after another, never sharing a result: each reads the parts when its turn comes,
 * so a save asked for during another one also writes the edits made in between.
 */
export function createSaveQueue(): SaveQueue {
  let tail: Promise<unknown> = Promise.resolve();
  let pending = 0;
  return {
    add(save) {
      const behindAnother = pending > 0;
      pending++;
      const turn = tail
        .then(() => save(behindAnother))
        .finally(() => {
          pending--;
        });
      tail = turn.catch(() => undefined);
      return turn;
    },
    busy: () => pending > 0,
    idle: () => tail,
  };
}

// ---------------------------------------------------------------------------------------------
// Import flow

/** The side effects of importing one file. The app's own talk to Rust, the prompts and the store; tests pass fakes. */
export interface ImportEnv {
  /** Reads and welds the file in Rust; `merge` joins an OBJ's objects into one mesh. */
  read(path: string, merge: boolean): Promise<ImportResult>;
  /** "One part" or "One part per object"; null when cancelled. */
  askObjects(fileName: string, count: number): Promise<ObjectsAnswer | null>;
  /** Whether to scale a file `size` mm across by 1000 (modelled in metres); null when cancelled. */
  askScale(fileName: string, size: number): Promise<boolean | null>;
  /** Whether to place the file on the platform; null when cancelled. */
  askPlace(fileName: string): Promise<boolean | null>;
  platform(): Vec3;
  /** False once New project or Open project has replaced the project the import started in. */
  current(): boolean;
  /** Adds the file's parts as one undo step labelled `label`. */
  add(parts: NewPart[], label: string): void;
  warn(message: string): void;
}

/**
 * Imports one file: asks how to split a multi-object OBJ, asks whether to scale a file without
 * units by 1000 when it is under METRES_BELOW mm across, asks (or applies the setting) whether to
 * place it, then adds its parts as one undo step. Cancelling either prompt skips the file, and so
 * does closing its project while the file is read. Rejects with the reader's error. Main-thread
 * work stays O(vertices), and only when placing.
 */
export async function importFile(path: string, env: ImportEnv): Promise<"added" | "skipped"> {
  const name = fileName(path);
  // Reading a big file takes seconds; its parts must not land in a project opened meanwhile.
  const closed = () => {
    if (env.current()) return false;
    env.warn(`Stopped importing "${name}" because its project was closed.`);
    return true;
  };

  let result = await env.read(path, false);
  if (closed()) return "skipped";
  if (result.splittable) {
    const answer = await env.askObjects(name, result.objectCount);
    if (answer === null) return "skipped";
    if (answer === "single") {
      result = await env.read(path, true);
      if (closed()) return "skipped";
    }
  }
  if (result.meshes.length === 0) {
    for (const w of result.warnings) env.warn(w);
    env.warn(`"${name}" has no triangles to import.`);
    return "skipped";
  }

  const size = metresSize(result);
  if (size !== null) {
    const scale = await env.askScale(name, size);
    if (scale === null || closed()) return "skipped";
    if (scale) result = { ...result, meshes: scaledMeshes(result.meshes, 1000) };
  }

  const place = await env.askPlace(name);
  if (place === null || closed()) return "skipped";
  const placed = place ? placedMatrices(result.meshes, env.platform()) : null;
  const parts = result.meshes.map(
    (m, i): NewPart => ({ name: m.name, mesh: m.mesh, matrix: placed?.[i] ?? m.matrix ?? undefined, source: path }),
  );
  env.add(parts, `Import ${name}`);
  for (const w of result.warnings) env.warn(w);
  return "added";
}

/** Progress callback for a status-bar task, updating it only when the whole percentage changes. */
function progressTo(task: TaskHandle): (p: ImportProgress) => void {
  let shown = -1;
  return (p) => {
    const percent = Math.floor(Math.min(Math.max(p.fraction, 0), 1) * 100);
    if (percent === shown) return;
    shown = percent;
    task.progress(percent / 100);
  };
}

let placementTurn: Promise<unknown> = Promise.resolve();

/**
 * "Place on platform?" for one file. Files take turns, and each reads the setting when its turn
 * comes, so "Remember my answer" on one file also answers the files queued behind it.
 */
export function askPlacement(name: string): Promise<boolean | null> {
  const turn = placementTurn.then(async () => {
    const rule = placementRule(useStore.getState().importPlacement);
    if (rule !== "ask") return rule;
    const answer = await prompt<PlaceAnswer>((resolve) => ({ kind: "place-on-import", fileName: name, resolve }));
    if (!answer) return null;
    const remembered = rememberedPlacement(answer);
    if (remembered) useStore.getState().setImportPlacement(remembered);
    return answer.place;
  });
  placementTurn = turn.catch(() => undefined);
  return turn;
}

/** Counts New project / Open project, so work that outlives its project (imports, saves) can tell. */
const documentGeneration = () => useStore.getState().generation;

/** The app's import side effects, for files of the project open right now. */
function appImportEnv(): ImportEnv {
  const generation = documentGeneration();
  return {
    async read(path, merge) {
      const task = startTask(`Importing ${fileName(path)}`);
      try {
        const result = await importMesh(path, merge, progressTo(task));
        // Rust measured the meshes while welding; keep that so the info panel and viewport do not redo it.
        for (const m of result.meshes) primeMeshCaches(m.mesh, { stats: m.stats, edges: m.edges });
        return result;
      } finally {
        task.end();
      }
    },
    askObjects: (name, count) => prompt<ObjectsAnswer>((resolve) => ({ kind: "import-objects", fileName: name, count, resolve })),
    askScale: (name, size) => prompt<boolean>((resolve) => ({ kind: "scale-on-import", fileName: name, size, resolve })),
    askPlace: askPlacement,
    platform: () => useStore.getState().platform,
    current: () => documentGeneration() === generation,
    add: (parts, label) => void useStore.getState().addParts(parts, label),
    warn: (text) => toast(text, "warn"),
  };
}

/** Toasts that `what` needs the desktop app and returns true when running in a plain browser (development). */
export function browserOnly(what: string): boolean {
  if (isTauri()) return false;
  toast(`${what} needs the tricky3D desktop app.`);
  return true;
}

/** File › Import part…: pick mesh files and import them. */
export async function importPart(): Promise<void> {
  if (browserOnly("Importing")) return;
  let picked: string[];
  try {
    picked = await pickFiles({ title: "Import part", multiple: true });
  } catch (err) {
    toast(errorMessage(err, "Could not show the file dialog"), "error");
    return;
  }
  if (picked.length) {
    usedFile(picked[0]);
    await importPaths(picked);
  }
}

/**
 * Imports files by path (file dialog or drag-and-drop). Each file imports on its own: Rust reads
 * them in parallel, prompts queue, and one file failing does not stop the others.
 */
export async function importPaths(paths: string[]): Promise<void> {
  if (browserOnly("Importing")) return;
  const unsupported = paths.filter((p) => importFormatOf(p) === null);
  if (unsupported.length) toast(unsupportedMessage(unsupported), "error");
  const supported = paths.filter((p) => importFormatOf(p) !== null);
  // A drop is a use of its folder too.
  if (supported.length) usedFile(supported[0]);
  const env = appImportEnv();
  await Promise.all(
    supported.map(async (path) => {
      try {
        await importFile(path, env);
      } catch (err) {
        toast(errorMessage(err, `Could not import "${fileName(path)}"`), "error");
      }
    }),
  );
}

// ---------------------------------------------------------------------------------------------
// Export

/** The parts Export writes, in tree order: the selection, or every visible part when nothing is selected. */
function partsToExport(): Part[] | null {
  const s = useStore.getState();
  const ids = new Set(targetIds(s));
  const parts = s.parts.filter((p) => ids.has(p.id));
  if (parts.length === 0) {
    toast("Nothing to export. Add or show a part first.");
    return null;
  }
  return parts;
}

async function pickSavePath(options: { title: string; filters: FileFilter[] } & DialogDefault): Promise<string | null> {
  try {
    const picked = await pickSaveFile(options);
    if (picked) usedFile(picked);
    return picked;
  } catch (err) {
    toast(errorMessage(err, "Could not show the save dialog"), "error");
    return null;
  }
}

/** The side effects of an export: the app's own dialogs, the store and the talk to Rust. Tests pass fakes. */
export interface ExportEnv {
  /** Export's options for `count` parts, opened on `last`; null when cancelled. */
  askOptions(count: number, last: ExportOptions): Promise<ExportOptions | null>;
  /** Keeps the options for the next export, also after a restart. */
  remember(options: ExportOptions): void;
  /** The Export dialog (pickExportFile); null when cancelled. */
  pick(request: { title: string; format: ExportType; each: boolean } & DialogDefault): Promise<ExportPick | null>;
  /** Writes every part into one file. */
  writeOne(path: string, format: ExportFormat, parts: Part[]): Promise<void>;
  /** Writes one file per part next to `path` (exportEach). */
  writeEach(path: string, format: ExportFormat, replace: boolean, parts: Part[]): Promise<EachResult>;
  /** Whether to replace the `existing` files in `folder`. */
  askReplace(existing: string[], folder: string): Promise<boolean>;
}

/** What an export wrote: the files' names and their folder (with its separator; "" when the path had none). */
export interface Exported {
  folder: string;
  files: string[];
}

/**
 * Exports `parts`, the same from every entry point: the options step (on `options`, the last ones used), then the
 * Export dialog with every type tricky3D writes and the parts' own format selected, else STL. One file is named and
 * placed as exportDialogDefault says, and the system asks before it replaces a file. One file per part (several parts
 * and that option) names each file after its part in the folder picked, and asks before replacing any of them.
 * Null when cancelled at any step; rejects with a writer's error.
 */
export async function exportParts(
  parts: Part[],
  context: { options: ExportOptions; projectName: string; lastFolder: string | null },
  env: ExportEnv,
): Promise<Exported | null> {
  const options = await env.askOptions(parts.length, context.options);
  if (!options) return null;
  env.remember(options);
  const each = parts.length > 1 && options.parts === "per-part";
  const type = exportType(parts);
  const picked = await env.pick({
    title: each ? `Export ${countParts(parts.length)}, one file each` : `Export ${countParts(parts.length)}`,
    format: type,
    each,
    ...(each ? eachDialogDefault(parts, type, context.lastFolder) : exportDialogDefault(parts, type, context.projectName, context.lastFolder)),
  });
  if (!picked) return null;
  const target = exportTarget(picked.path, picked.format, options.stl);
  if (!each) {
    await env.writeOne(target.path, target.format, parts);
    return { folder: folderOf(target.path), files: [fileName(target.path)] };
  }
  // Rust takes the folder of the path exactly as picked.
  const folder = folderOf(picked.path);
  let result = await env.writeEach(picked.path, target.format, false, parts);
  if (result.existing.length > 0) {
    if (!(await env.askReplace(result.existing, folder))) return null;
    result = await env.writeEach(picked.path, target.format, true, parts);
  }
  return { folder, files: result.written };
}

/** The toast after an export of `count` parts. */
export function exportedMessage(count: number, { folder, files }: Exported): string {
  if (files.length === 1) return `Exported ${countParts(count)} to ${files[0]}`;
  return `Exported ${countParts(count)} to ${files.length} files in ${shownFolder(folder)}`;
}

const meshesOut = (parts: Part[]) => parts.map((p) => ({ name: p.name, matrix: p.matrix, mesh: p.mesh }));

/** Runs `write` as a status-bar task named after the file. */
async function exportTask<T>(path: string, write: () => Promise<T>): Promise<T> {
  const task = startTask(`Exporting ${fileName(path)}`);
  try {
    return await write();
  } finally {
    task.end();
  }
}

/** The app's export side effects. */
function appExportEnv(): ExportEnv {
  return {
    askOptions: (count, last) => prompt<ExportOptions>((resolve) => ({ kind: "export-options", count, options: last, resolve })),
    remember: (options) => useStore.getState().setExportOptions(options),
    async pick(request) {
      const picked = await pickExportFile(request);
      if (picked) usedFile(picked.path);
      return picked;
    },
    writeOne: (path, format, parts) => exportTask(path, () => exportMeshes(path, format, meshesOut(parts))),
    writeEach: (path, format, replace, parts) => exportTask(path, () => exportEach(path, format, replace, meshesOut(parts))),
    askReplace: (existing, folder) =>
      ask(replaceQuestion(existing, folder), { title: "Replace files?", kind: "warning", okLabel: "Replace", cancelLabel: "Cancel" }),
  };
}

/**
 * Export (ribbon, File menu, right-click and Ctrl+E): the selected parts, or every visible part when none is
 * selected, through exportParts.
 */
export async function exportSelection(): Promise<void> {
  if (browserOnly("Exporting")) return;
  const parts = partsToExport();
  if (!parts) return;
  const { exportOptions, projectName, lastFolder } = useStore.getState();
  try {
    const done = await exportParts(parts, { options: exportOptions, projectName, lastFolder }, appExportEnv());
    if (done) toast(exportedMessage(parts.length, done));
  } catch (err) {
    toast(errorMessage(err, "Could not export"), "error");
  }
}

// ---------------------------------------------------------------------------------------------
// Project

/**
 * File › Open…: asks about unsaved changes, then replaces the scene with the file picked. A 3MF opens
 * as a project; any other mesh file starts a new project holding its parts.
 */
export async function openFile(): Promise<void> {
  if (browserOnly("Opening a file")) return;
  if (!(await confirmDiscard())) return;
  let path: string | null;
  try {
    path = await pickOpenFile();
  } catch (err) {
    toast(errorMessage(err, "Could not show the file dialog"), "error");
    return;
  }
  if (!path) return;
  usedFile(path);
  const format = importFormatOf(path);
  if (format === "3mf") await openProject(path);
  else if (format) await openMesh(path);
  else toast(unsupportedMessage([path]), "error");
}

/**
 * After File › Open has read `name`: true when it may replace the open project. Reading takes a while.
 * New project or another Open that finished first wins, and edits made meanwhile are asked about
 * again rather than thrown away. So is repair work: a part can go into repair mode while the file is
 * read, and repair mode never changes the parts, so only closeRepair notices its unapplied changes.
 * It asks last, so no repair can start between its answer and the replacement.
 */
async function mayReplace(name: string, generation: number, before: AppState): Promise<boolean> {
  const stale = () => documentGeneration() !== generation;
  if (stale()) {
    toast(`Stopped opening "${name}" because another project was opened or started meanwhile.`, "warn");
    return false;
  }
  const now = useStore.getState();
  const edited = now.parts !== before.parts || now.platform !== before.platform;
  if (edited && !((await confirmDiscard()) && !stale())) return false;
  return (await closeRepair()) && !stale();
}

/**
 * A mesh file as a new project: asks what Import part asks (OBJ objects, placement), keeps the
 * platform, and has no file yet, so Save asks for one. The open project stays if reading fails.
 */
async function openMesh(path: string): Promise<void> {
  const name = fileName(path);
  const generation = documentGeneration();
  const before = useStore.getState();
  const parts: NewPart[] = [];
  const env: ImportEnv = { ...appImportEnv(), add: (read) => void parts.push(...read) };
  try {
    if ((await importFile(path, env)) !== "added") return;
  } catch (err) {
    toast(errorMessage(err, `Could not open "${name}"`), "error");
    return;
  }
  if (!(await mayReplace(name, generation, before))) return;
  const s = useStore.getState();
  s.loadProject({ name: fileStem(path), path: null, platform: s.platform, parts });
  viewport.zoomTo("everything");
}

/** A 3MF as the project: its parts with their display fields, and the platform it asks for. */
async function openProject(path: string): Promise<void> {
  const name = fileName(path);
  const generation = documentGeneration();
  const before = useStore.getState();
  const task = startTask(`Opening ${name}`);
  let result: ProjectResult;
  try {
    result = await openProjectFile(path, progressTo(task));
  } catch (err) {
    toast(errorMessage(err, `Could not open "${name}"`), "error");
    return;
  } finally {
    task.end();
  }
  if (!(await mayReplace(name, generation, before))) return;

  try {
    for (const m of result.meshes) primeMeshCaches(m.mesh, { stats: m.stats, edges: m.edges });
    const s = useStore.getState();
    s.loadProject({
      name: fileStem(path),
      path,
      platform: projectPlatform(result.platform, s.platform),
      parts: result.meshes.map(projectPart),
    });
    for (const w of result.warnings) toast(w, "warn");
    viewport.zoomTo("everything");
  } catch (err) {
    toast(errorMessage(err, `Could not open "${name}"`), "error");
  }
}

/** Writes every part and the platform to `path`. Resolves true once the file is written and the project is marked saved. */
async function writeProject(path: string): Promise<boolean> {
  const { parts, platform } = useStore.getState();
  const generation = documentGeneration();
  const name = fileName(path);
  const task = startTask(`Saving ${name}`);
  try {
    await saveProjectFile(path, platform, parts.map(projectMesh));
  } catch (err) {
    toast(errorMessage(err, `Could not save "${name}"`), "error");
    return false;
  } finally {
    task.end();
  }

  toast(`Saved ${name}`);
  // Another project was opened (or a new one started) while the file was written: the file holds
  // the old project, so the open one keeps its own name, path and saved state.
  if (documentGeneration() !== generation) return true;
  // Edits made while the file was being written are not in it, so they stay unsaved.
  useStore.getState().markSaved(path, fileStem(path), { parts, platform });
  return true;
}

async function saveWithDialog(): Promise<boolean> {
  const s = useStore.getState();
  const picked = await pickSavePath({
    title: "Save project",
    ...saveDialogDefault(s.parts, s.projectName, s.projectPath, s.lastFolder),
    filters: PROJECT_FILTERS,
  });
  return picked ? writeProject(withExtension(picked, "3mf")) : false;
}

const saves = createSaveQueue();

/** File › Save project: writes to the project's file, or asks for one first. Resolves true when saved. */
export function saveProject(): Promise<boolean> {
  if (browserOnly("Saving")) return Promise.resolve(false);
  return saves.add((behindAnother) => {
    const s = useStore.getState();
    // Pressed again (or key repeat) while a save was writing, and that save wrote everything.
    if (behindAnother && !isDirty(s)) return Promise.resolve(true);
    return s.projectPath ? writeProject(s.projectPath) : saveWithDialog();
  });
}

/** File › Save project as (.3mf)…: asks for a file, then saves. Resolves true when saved. */
export function saveProjectAs(): Promise<boolean> {
  if (browserOnly("Saving")) return Promise.resolve(false);
  return saves.add(saveWithDialog);
}

/** The unsaved-changes question: Save, Don't save or Cancel. Cancel when the box cannot be shown. */
async function askDiscard(projectName: string): Promise<DiscardChoice> {
  const question = `Do you want to save the changes to "${projectName}"? Your changes will be lost if you don't save them.`;
  // Plain browser (development): no native dialog.
  if (!isTauri()) return window.confirm(`${question}\n\nOK discards them.`) ? "discard" : "cancel";
  try {
    const result = await message(question, {
      title: "Unsaved changes",
      kind: "warning",
      buttons: { yes: SAVE_LABEL, no: DONT_SAVE_LABEL, cancel: "Cancel" },
    });
    return discardChoice(result);
  } catch (err) {
    toast(errorMessage(err, "Could not ask about unsaved changes"), "error");
    return "cancel";
  }
}

/**
 * Asks before unsaved changes are thrown away (New project, Open project, closing the window).
 * Resolves true when it is fine to go on: nothing unsaved, "Don't save", or "Save" and everything
 * is now in the file. Never rejects; if the question cannot be asked, it resolves false.
 */
export async function confirmDiscard(): Promise<boolean> {
  for (;;) {
    // A save that is still writing may hold the changes, and quitting mid-write would leave a
    // broken file, so wait for it before asking. Say so, or a click on Close seems to do nothing.
    if (saves.busy()) toast("Waiting for the save to finish…");
    await saves.idle();
    const s = useStore.getState();
    if (!isDirty(s)) return true;
    const choice = await askDiscard(s.projectName);
    if (choice !== "save") return choice === "discard";
    if (!(await saveProject())) return false;
    // Saved. Edits made while the file was written are not in it, so they are asked about again.
  }
}
