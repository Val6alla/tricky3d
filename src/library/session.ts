// The Part library mode: entering and leaving, the category and primitive picked, the form, the tile pictures, and
// Create part. UI code changes library state only through these functions; the state is in src/library/store.ts, the
// meshes come from Rust (src/library/ipc.ts). The decisions are plain exported functions, so tests run them without
// Tauri.
//
// The flow follows the reference tool: Home › Part library opens a "Part library" tab, a gallery of primitives over the
// view and their parameters in the right panel; Create part builds the mesh in Rust, adds it as a part standing on the
// middle of the platform (one undo step) and leaves the mode; Cancel leaves without adding anything.
import { CLOSE_BOOLEAN_FIRST, isBooleanOpen } from "../boolean/session";
import type { TabId } from "../features";
import { browserOnly, errorMessage } from "../io/files";
import { CLOSE_MEASURE_FIRST, isMeasureOpen } from "../measure/session";
import { translation } from "../mesh";
import { primeMeshCaches } from "../meshCache";
import { parseNumber } from "../numbers";
import { meshOperationRunning, WAIT_MESSAGE } from "../ops/meshOps";
import { CANCELLED, cancelRepair } from "../repair/ipc";
import { CLOSE_REPAIR_FIRST, isRepairing } from "../repair/session";
import { useStore } from "../store";
import { startTask, toast, useUi } from "../ui";
import { buildPrimitive, fetchLibraryCatalog, type LibraryParam, type LibraryPrimitive } from "./ipc";
import { useLibrary, type LibraryCategory, type LibrarySession } from "./store";
import { releaseThumbnails, thumbnail } from "./thumbnails";

/** Main-document commands that would add or change parts refuse with this while the library is open. */
export const CLOSE_LIBRARY_FIRST = "Close the part library first.";

// ---------------------------------------------------------------------------------------------
// The form (pure)

/** The number a field shows for a value: plain, without trailing zeros. */
export function valueText(v: number): string {
  return String(Math.round(v * 1e6) / 1e6);
}

/** The form's starting text: every parameter's default. */
export function defaultValues(primitive: LibraryPrimitive): Record<string, string> {
  return Object.fromEntries(primitive.params.map((p) => [p.key, valueText(p.default)]));
}

/** Why a field's text cannot be used, or null. */
export function fieldProblem(param: LibraryParam, text: string): string | null {
  const v = parseNumber(text);
  if (v === null) return "Enter a number.";
  if (param.integer && !Number.isInteger(v)) return "Enter a whole number.";
  if (v < param.min || v > param.max) {
    const unit = param.unit ? ` ${param.unit}` : "";
    return `Enter ${valueText(param.min)}${unit} to ${valueText(param.max)}${unit}.`;
  }
  return null;
}

/** The numbers to send to Rust, and the fields that are wrong (by key). */
export function parseValues(primitive: LibraryPrimitive, values: Record<string, string>): { params: Record<string, number>; errors: Record<string, string> } {
  const params: Record<string, number> = {};
  const errors: Record<string, string> = {};
  for (const p of primitive.params) {
    const text = values[p.key] ?? valueText(p.default);
    const problem = fieldProblem(p, text);
    if (problem) errors[p.key] = problem;
    else params[p.key] = parseNumber(text) as number;
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

// ---------------------------------------------------------------------------------------------
// State helpers

const state = () => useLibrary.getState();
const session = () => useLibrary.getState().session;
const primitiveOf = (id: string) => state().catalog?.find((p) => p.id === id);

/** Job numbers of library builds, apart from the other modes' (all cancel through repair_cancel). */
let jobSeq = 2 ** 41;
let tabBefore: TabId | null = null;

function setSession(patch: Partial<LibrarySession>) {
  const current = session();
  if (current) useLibrary.setState({ session: { ...current, ...patch } });
}

/** True while the Part library is open. */
export function isLibraryOpen(): boolean {
  return session() !== null;
}

// ---------------------------------------------------------------------------------------------
// Entering and leaving

/**
 * Home › Part library: opens the "Part library" tab, the gallery and the form, with the Box picked. Refused while repair
 * mode, Boolean operations, Measure or a mesh operation is busy with the parts.
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
  const first = state().catalog?.[0];
  if (!first) return;
  useLibrary.setState({ session: { category: "primitive", primitive: first.id, name: "", nameEdited: false, values: {} }, busy: null });
  pickPrimitive(first.id);
  tabBefore = ui.ribbonTab === "library" ? null : ui.ribbonTab;
  ui.closeContextMenu();
  ui.setRibbonTab("library");
  watchProject();
  const store = useStore.getState();
  if (store.gizmoMode) store.setGizmoMode(null);
  void drawTiles();
}

/** Cancel: leaves the mode without adding anything; a part being made is dropped. */
export function closePartLibrary(): void {
  if (!isLibraryOpen()) return;
  const busy = state().busy;
  if (busy) cancelRepair(busy.job).catch(() => {});
  clearTimeout(thumbTimer);
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

export function pickCategory(category: LibraryCategory): void {
  setSession({ category });
}

/** A tile: that primitive with its default values; the name follows unless the user typed one. */
export function pickPrimitive(id: string): void {
  const current = session();
  const primitive = primitiveOf(id);
  if (!current || !primitive) return;
  const name = current.nameEdited ? current.name : defaultName(primitive.label, useStore.getState().parts.map((p) => p.name));
  setSession({ primitive: id, values: defaultValues(primitive), name });
}

export function setName(name: string): void {
  setSession({ name, nameEdited: true });
}

export function setValue(key: string, text: string): void {
  const current = session();
  if (!current) return;
  setSession({ values: { ...current.values, [key]: text } });
  redrawPickedTile();
}

/** Back to the picked primitive's defaults. */
export function resetValues(): void {
  const current = session();
  const primitive = current && primitiveOf(current.primitive);
  if (!primitive) return;
  setSession({ values: defaultValues(primitive) });
  redrawPickedTile();
}

// ---------------------------------------------------------------------------------------------
// Tile pictures

/** The tile pictures of every primitive at its defaults, one after another. */
async function drawTiles(): Promise<void> {
  for (const p of state().catalog ?? []) {
    if (!isLibraryOpen()) return;
    if (state().thumbs[p.id]) continue;
    try {
      const built = await buildPrimitive(p.id, parseValues(p, defaultValues(p)).params, ++jobSeq);
      if (!isLibraryOpen()) return;
      useLibrary.setState((s) => ({ thumbs: { ...s.thumbs, [p.id]: thumbnail(built.mesh) } }));
    } catch (err) {
      console.error(`library thumbnail ${p.id}`, err);
    }
  }
}

let thumbTimer: ReturnType<typeof setTimeout> | undefined;

/** The picked tile follows the form, a moment after the typing stops. */
function redrawPickedTile(): void {
  clearTimeout(thumbTimer);
  thumbTimer = setTimeout(async () => {
    const current = session();
    const primitive = current && primitiveOf(current.primitive);
    if (!current || !primitive) return;
    const { params, errors } = parseValues(primitive, current.values);
    if (Object.keys(errors).length > 0) return;
    try {
      const built = await buildPrimitive(primitive.id, params, ++jobSeq);
      if (session()?.values !== current.values) return;
      useLibrary.setState((s) => ({ thumbs: { ...s.thumbs, [primitive.id]: thumbnail(built.mesh) } }));
    } catch {
      // A value Rust refuses (turns that touch): the form says so on Create part.
    }
  }, 300);
}

// ---------------------------------------------------------------------------------------------
// Create part

/**
 * Create part: Rust builds the mesh from the form, which is added as one part, named as the form says, standing on the
 * middle of the platform; one undo step "Create <name>". Then the mode closes. A wrong field or a value Rust refuses
 * keeps it open with the reason.
 */
export async function createPart(): Promise<void> {
  const current = session();
  if (!current || state().busy) return;
  const primitive = primitiveOf(current.primitive);
  if (!primitive) return;
  const { params, errors } = parseValues(primitive, current.values);
  const wrong = primitive.params.find((p) => errors[p.key]);
  if (wrong) {
    toast(`${wrong.label}: ${errors[wrong.key]}`, "warn");
    return;
  }
  const name = current.name.trim() || primitive.label;
  const job = ++jobSeq;
  useLibrary.setState({ busy: { job, cancelling: false } });
  const task = startTask(`Creating ${name}`);
  try {
    const built = await buildPrimitive(primitive.id, params, job);
    if (state().busy?.job !== job || state().busy?.cancelling || !isLibraryOpen()) return;
    primeMeshCaches(built.mesh, { stats: built.stats, edges: built.edges });
    const store = useStore.getState();
    const [x, y] = store.platform;
    store.addParts([{ name, mesh: built.mesh, matrix: translation(x / 2, y / 2, 0) }], `Create ${name}`);
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
