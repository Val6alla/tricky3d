// The Part library mode: entering and leaving, the category and item picked, the form, the picture, the tile pictures,
// the Triangles read-out, and Create part. UI code changes library state only through these functions; the state is in
// src/library/store.ts, the meshes come from Rust (src/library/ipc.ts). The decisions are plain exported functions, so
// tests run them without Tauri.
//
// The flow follows the reference tool: Home › Part library opens a "Part library" tab, four categories with a gallery of
// items over the view and the picked item's parameters in the right panel; Create part builds the mesh in Rust, adds it
// as a part standing on the middle of the platform (one undo step) and leaves the mode; Cancel leaves without adding
// anything. Items made from the selected parts (Convex hull, Base plate) are added where those parts are.
import { CLOSE_BOOLEAN_FIRST, isBooleanOpen } from "../boolean/session";
import type { TabId } from "../features";
import { browserOnly, errorMessage, sourceOf } from "../io/files";
import { CLOSE_MEASURE_FIRST, isMeasureOpen } from "../measure/session";
import { IDENTITY, translation } from "../mesh";
import { primeMeshCaches } from "../meshCache";
import { parseNumber } from "../numbers";
import { meshOperationRunning, WAIT_MESSAGE } from "../ops/meshOps";
import { CANCELLED, cancelRepair } from "../repair/ipc";
import { CLOSE_REPAIR_FIRST, isRepairing } from "../repair/session";
import { useStore, type Part } from "../store";
import { startTask, toast, useUi } from "../ui";
import { buildItem, fetchLibraryCatalog, pickPicture, type LibraryItem, type LibraryParam, type NumberParam } from "./ipc";
import { CATEGORIES, useLibrary, type FieldValue, type LibraryCategory, type LibrarySession } from "./store";
import { releaseThumbnails, thumbnail } from "./thumbnails";
import { CLOSE_LABEL_FIRST, isLabelOpen } from "../label/session";
import { nextJob } from "../jobs";

/** Main-document commands that would add or change parts refuse with this while the library is open. */
export const CLOSE_LIBRARY_FIRST = "Close the part library first.";

// ---------------------------------------------------------------------------------------------
// The form (pure)

/** The number a field shows for a value: plain, without trailing zeros. */
export function valueText(v: number): string {
  return String(Math.round(v * 1e6) / 1e6);
}

/** A parameter's starting value in the form. */
export function defaultValue(param: LibraryParam): FieldValue {
  switch (param.kind) {
    case "number":
      return valueText(param.default);
    case "choice":
    case "text":
      return param.default;
    case "toggle":
      return param.default;
    case "image":
      return "";
  }
}

/** The form's starting values: every parameter's default. */
export function defaultValues(item: LibraryItem): Record<string, FieldValue> {
  return Object.fromEntries(item.params.map((p) => [p.key, defaultValue(p)]));
}

/** Why a number field's text cannot be used, or null. */
export function fieldProblem(param: NumberParam, text: string): string | null {
  const v = parseNumber(text);
  if (v === null) return "Enter a number.";
  if (param.integer && !Number.isInteger(v)) return "Enter a whole number.";
  if (v < param.min || v > param.max) {
    const unit = param.unit === "%" || param.unit === "°" ? param.unit : param.unit ? ` ${param.unit}` : "";
    return `Enter ${valueText(param.min)}${unit} to ${valueText(param.max)}${unit}.`;
  }
  return null;
}

/** Why a parameter's value cannot be used, or null. */
export function valueProblem(param: LibraryParam, value: FieldValue | undefined): string | null {
  const v = value ?? defaultValue(param);
  switch (param.kind) {
    case "number":
      return fieldProblem(param, String(v));
    case "text": {
      const length = [...String(v)].length;
      return length > param.maxLength ? `Enter at most ${param.maxLength} characters.` : null;
    }
    case "choice": {
      const option = param.options.find((o) => o.value === v);
      if (!option) return "Pick one of the choices.";
      return option.soon ? `${option.label} is not available yet.` : null;
    }
    default:
      return null;
  }
}

/** Whether a parameter shows: always, or while the toggle its `when` names is on ("!key": off). */
export function shows(param: LibraryParam, values: Record<string, FieldValue>, item: LibraryItem): boolean {
  if (!param.when) return true;
  const off = param.when.startsWith("!");
  const key = off ? param.when.slice(1) : param.when;
  const toggle = item.params.find((p) => p.key === key);
  const on = values[key] ?? (toggle ? defaultValue(toggle) : false);
  return off ? on !== true : on === true;
}

/** The values to send to Rust, and the shown fields that are wrong (by key). Hidden fields keep their defaults. */
export function parseValues(
  item: LibraryItem,
  values: Record<string, FieldValue>,
): { params: Record<string, number | string | boolean>; errors: Record<string, string> } {
  const params: Record<string, number | string | boolean> = {};
  const errors: Record<string, string> = {};
  for (const p of item.params) {
    if (p.kind === "image") continue;
    const value = shows(p, values, item) ? (values[p.key] ?? defaultValue(p)) : defaultValue(p);
    const problem = valueProblem(p, value);
    if (problem) {
      errors[p.key] = problem;
      continue;
    }
    params[p.key] = p.kind === "number" ? (parseNumber(String(value)) as number) : value;
  }
  return { params, errors };
}

/** "Box", or "Box 2" when a part is called "Box" already (the next number free). */
export function defaultName(label: string, names: readonly string[]): string {
  const taken = new Set(names);
  if (!taken.has(label)) return label;
  let n = 2;
  while (taken.has(`${label} ${n}`)) n++;
  return `${label} ${n}`;
}

/** Whether an item takes a picture. */
export const usesPicture = (item: LibraryItem) => item.params.some((p) => p.kind === "image");

/** "12,345 triangles" for the read-out. */
export function triangleText(n: number): string {
  return n.toLocaleString("en-US");
}

// ---------------------------------------------------------------------------------------------
// State helpers

const state = () => useLibrary.getState();
const session = () => useLibrary.getState().session;
const itemOf = (id: string) => state().catalog?.find((p) => p.id === id);

let tabBefore: TabId | null = null;

function setSession(patch: Partial<LibrarySession>) {
  const current = session();
  if (current) useLibrary.setState({ session: { ...current, ...patch } });
}

/** True while the Part library is open. */
export function isLibraryOpen(): boolean {
  return session() !== null;
}

/** The selected parts an item made from the selection is made from: the selection's visible parts. */
function selectedParts(): Part[] {
  const s = useStore.getState();
  const ids = new Set(s.selection);
  return s.parts.filter((p) => ids.has(p.id) && p.visible);
}

// ---------------------------------------------------------------------------------------------
// Entering and leaving

/**
 * Home › Part library: opens the "Part library" tab, the gallery and the form, with the first primitive picked.
 * Refused while repair mode, Boolean operations, Measure or a mesh operation is busy with the parts.
 */
export async function openPartLibrary(): Promise<void> {
  const ui = useUi.getState();
  if (isLibraryOpen()) {
    ui.setRibbonTab("library");
    return;
  }
  if (isRepairing()) return toast(CLOSE_REPAIR_FIRST);
  if (isBooleanOpen()) return toast(CLOSE_BOOLEAN_FIRST);
  if (isMeasureOpen()) return toast(CLOSE_MEASURE_FIRST);
  if (isLabelOpen()) return toast(CLOSE_LABEL_FIRST);
  if (meshOperationRunning()) return toast(WAIT_MESSAGE);
  if (browserOnly("The part library")) return;
  if (!state().catalog) {
    try {
      useLibrary.setState({ catalog: await fetchLibraryCatalog() });
    } catch (err) {
      toast(errorMessage(err, "Could not load the part library"), "error");
      return;
    }
    // A second click while the catalog loaded opened it already.
    if (isLibraryOpen()) return;
  }
  const first = state().catalog?.find((p) => p.category === "primitive" && !p.soon);
  if (!first) return;
  useLibrary.setState({
    session: { category: "primitive", primitive: first.id, name: "", nameEdited: false, values: {}, triangles: null },
    busy: null,
  });
  pickPrimitive(first.id);
  tabBefore = ui.ribbonTab === "library" ? null : ui.ribbonTab;
  ui.closeContextMenu();
  ui.setRibbonTab("library");
  watchProject();
  const store = useStore.getState();
  if (store.gizmoMode) store.setGizmoMode(null);
  void drawTiles("primitive");
}

/** Cancel: leaves the mode without adding anything; a part being made is dropped. */
export function closePartLibrary(): void {
  if (!isLibraryOpen()) return;
  const busy = state().busy;
  if (busy) cancelRepair(busy.job).catch(() => {});
  stopPreview();
  useLibrary.setState({ session: null, busy: null });
  releaseThumbnails();
  const ui = useUi.getState();
  if (ui.ribbonTab === "library") ui.setRibbonTab(tabBefore ?? "home");
  tabBefore = null;
}

let watching = false;

/** New or Open project ends the mode: the part would land in the project that replaced this one (subscribes once). */
function watchProject() {
  if (watching) return;
  watching = true;
  useStore.subscribe((s, prev) => {
    if (s.generation !== prev.generation) closePartLibrary();
  });
}

// ---------------------------------------------------------------------------------------------
// The gallery and the form

/** A category: its tiles, and its first item picked unless the picked one belongs to it. */
export function pickCategory(category: LibraryCategory): void {
  const current = session();
  if (!current) return;
  setSession({ category });
  const picked = itemOf(current.primitive);
  if (picked?.category !== category) {
    const first = state().catalog?.find((p) => p.category === category && !p.soon);
    if (first) pickPrimitive(first.id);
  }
  void drawTiles(category);
}

/** A tile: that item with its default values; the name follows unless the user typed one. Soon items say why. */
export function pickPrimitive(id: string): void {
  const current = session();
  const item = itemOf(id);
  if (!current || !item) return;
  if (item.soon) {
    toast(`${item.label} is not available yet: ${item.soon}`);
    return;
  }
  const name = current.nameEdited ? current.name : defaultName(item.label, useStore.getState().parts.map((p) => p.name));
  setSession({ primitive: id, values: defaultValues(item), name, triangles: null });
  redrawPickedTile(0);
}

export function setName(name: string): void {
  setSession({ name, nameEdited: true });
}

export function setValue(key: string, value: FieldValue): void {
  const current = session();
  if (!current) return;
  setSession({ values: { ...current.values, [key]: value }, triangles: null });
  redrawPickedTile();
}

/** Back to the picked item's defaults. */
export function resetValues(): void {
  const current = session();
  const item = current && itemOf(current.primitive);
  if (!item) return;
  setSession({ values: defaultValues(item), triangles: null });
  redrawPickedTile();
}

/** Choose picture…: the picture dialog; the picture then shapes every item that takes one. */
export async function choosePicture(): Promise<void> {
  try {
    const picture = await pickPicture();
    if (!picture || !isLibraryOpen()) return;
    useLibrary.setState({ picture });
    setSession({ triangles: null });
    redrawPickedTile(0);
  } catch (err) {
    toast(errorMessage(err, "Could not read the picture"), "error");
  }
}

/** Back to the built-in sample picture. */
export function clearPicture(): void {
  useLibrary.setState({ picture: null });
  setSession({ triangles: null });
  redrawPickedTile(0);
}

// ---------------------------------------------------------------------------------------------
// Tile pictures and the Triangles read-out

/** The tile pictures of a category's items at their defaults, and the four category pictures, one after another. */
async function drawTiles(category: LibraryCategory): Promise<void> {
  const catalog = state().catalog ?? [];
  const pictures = CATEGORIES.flatMap((c) => catalog.filter((p) => p.id === c.picture));
  const wanted = [...pictures, ...catalog.filter((p) => p.category === category && !p.soon)];
  for (const item of wanted) {
    if (!isLibraryOpen() || session()?.category !== category) return;
    if (state().thumbs[item.id]) continue;
    try {
      const built = await buildItem(item.id, parseValues(item, defaultValues(item)).params, nextJob(), { preview: true });
      if (!isLibraryOpen()) return;
      useLibrary.setState((s) => ({ thumbs: { ...s.thumbs, [item.id]: thumbnail(built.mesh) } }));
    } catch (err) {
      console.error(`library thumbnail ${item.id}`, err);
    }
  }
}

let previewTimer: ReturnType<typeof setTimeout> | undefined;
let previewJob: number | null = null;

function stopPreview() {
  clearTimeout(previewTimer);
  if (previewJob !== null) cancelRepair(previewJob).catch(() => {});
  previewJob = null;
}

/**
 * The picked tile and the Triangles read-out follow the form, a moment after the typing stops: Rust builds the item
 * with the form's values (a build still running for older values is cancelled).
 */
function redrawPickedTile(delay = 300): void {
  stopPreview();
  previewTimer = setTimeout(async () => {
    const current = session();
    const item = current && itemOf(current.primitive);
    if (!current || !item) return;
    const { params, errors } = parseValues(item, current.values);
    if (Object.keys(errors).length > 0) return;
    const job = nextJob();
    previewJob = job;
    try {
      const parts = item.input === "parts" ? selectedParts() : [];
      const built = await buildItem(item.id, params, job, {
        preview: parts.length === 0,
        parts: parts.map(partMesh),
        picture: usesPicture(item) ? state().picture : null,
      });
      if (previewJob !== job || session()?.values !== current.values || session()?.primitive !== item.id) return;
      previewJob = null;
      useLibrary.setState((s) => ({ thumbs: { ...s.thumbs, [item.id]: thumbnail(built.mesh) } }));
      setSession({ triangles: built.stats.triangles });
    } catch {
      // A value Rust refuses: Create part says why.
      if (previewJob === job) previewJob = null;
    }
  }, delay);
}

const partMesh = (p: Part) => ({ name: p.name, mesh: p.mesh, matrix: p.matrix });

// ---------------------------------------------------------------------------------------------
// Create part

/**
 * Create part: Rust builds the mesh from the form, which is added as one part, named as the form says, standing on the
 * middle of the platform, or for an item made from the selection, where the selection is; one undo step
 * "Create <name>". Then the mode closes. A wrong field or a value Rust refuses keeps it open with the reason.
 */
export async function createPart(): Promise<void> {
  const current = session();
  if (!current || state().busy) return;
  const item = itemOf(current.primitive);
  if (!item || item.soon) return;
  const { params, errors } = parseValues(item, current.values);
  const wrong = item.params.find((p) => errors[p.key]);
  if (wrong) {
    toast(`${wrong.label}: ${errors[wrong.key]}`, "warn");
    return;
  }
  const parts = item.input === "parts" ? selectedParts() : [];
  if (item.input === "parts" && parts.length === 0) {
    toast(`Select the part to make the ${item.label.toLowerCase()} from first: click it in the parts list.`, "warn");
    return;
  }
  stopPreview();
  const name = current.name.trim() || item.label;
  const job = nextJob();
  useLibrary.setState({ busy: { job, cancelling: false, fraction: 0 } });
  const task = startTask(`Creating ${name}`);
  try {
    const built = await buildItem(item.id, params, job, {
      parts: parts.map(partMesh),
      picture: usesPicture(item) ? state().picture : null,
      onProgress: (fraction) => {
        const busy = state().busy;
        if (busy?.job === job) useLibrary.setState({ busy: { ...busy, fraction } });
        task.progress(fraction);
      },
    });
    if (state().busy?.job !== job || state().busy?.cancelling || !isLibraryOpen()) return;
    primeMeshCaches(built.mesh, { stats: built.stats, edges: built.edges });
    const store = useStore.getState();
    const [x, y] = store.platform;
    const matrix = built.world ? [...IDENTITY] : translation(x / 2, y / 2, 0);
    const source = sourceOf(parts) ?? undefined;
    store.addParts([{ name, mesh: built.mesh, matrix, source }], `Create ${name}`);
    useLibrary.setState({ busy: null });
    closePartLibrary();
  } catch (err) {
    if (err !== CANCELLED && isLibraryOpen()) toast(errorMessage(err, `Could not create ${name}`), "error");
  } finally {
    task.end();
    if (state().busy?.job === job) useLibrary.setState({ busy: null });
  }
}

/** Stops a Create part in progress; nothing is added. */
export function cancelCreate(): void {
  const busy = state().busy;
  if (!busy || busy.cancelling) return;
  useLibrary.setState({ busy: { ...busy, cancelling: true } });
  cancelRepair(busy.job).catch(() => {});
}
