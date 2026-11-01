// Repair mode's session: entering and leaving, Apply repair, runs (one action, a script, a status update), the inner
// undo history and the triangle selection. UI code changes repair state only through these functions; the state
// itself is in src/repair/store.ts. The working copy is an immutable world-space MeshData kept here; each run sends
// it to Rust (src/repair/ipc.ts) and takes the result back, so nothing about a session lives in Rust.
//
// Runs follow the rules of src/ops/meshOps.ts:
// - one at a time; a second request is refused with a toast, not queued;
// - the progress dialog opens only once a run has taken BUSY_DELAY_MS and no other dialog is open (it waits for one
//   to close rather than replace it), and always closes when the run ends;
// - a cancelled run (Rust rejects with CANCELLED) leaves the working copy, selection and history exactly as they were;
// - a run that changed the mesh is one inner undo step, labelled with the action's or script's name; a run that
//   changed nothing adds none.
import { isTauri } from "@tauri-apps/api/core";
import { message } from "@tauri-apps/plugin-dialog";
import type { TabId } from "../features";
import { formatCount, triangleCount } from "../info";
import { browserOnly, errorMessage } from "../io/files";
import { boundsOf, IDENTITY, type Bounds } from "../mesh";
import { bakeMesh, weldMesh } from "../ops/bake";
import { BUSY_DELAY_MS, freshColors, meshOperationRunning, WAIT_MESSAGE } from "../ops/meshOps";
import { useStore, type MeshData, type Part } from "../store";
import { dialogSlotFree, toast, useUi } from "../ui";
import { clearTriangleBvh } from "../viewport/triPick";
import { checkFromStatus, primeCheck } from "./badges";
import { runSummary, runWarnings, selectionInfoText } from "./format";
import { redoStep, pushStep, undoStep, type HistoryMove } from "./history";
import { CANCELLED, cancelRepair, runRepair } from "./ipc";
import { actionSpec, enabledSteps, loadScripts, scriptProblems } from "./scripts";
import {
  applySelection,
  clearSelectionCaches,
  countSelected,
  expandMask,
  extractTriangles,
  idsToMask,
  invertMask,
  maskToIds,
  selectionBounds,
  selectionInfoOf,
  shellRegion,
  shrinkMask,
  surfaceRegion,
  type SelectionInfo,
  type SelectMode,
} from "./selection";
import { useRepair } from "./store";
import type {
  ActionId,
  ApplyMode,
  Highlights,
  ParamValue,
  RepairProgress,
  RepairScript,
  RepairSession,
  RepairStatus,
  RunResult,
  ScriptStep,
  SelectTool,
} from "./types";
import { CLOSE_LABEL_FIRST, isLabelOpen } from "../label/session";
import { nextJob } from "../jobs";

export type { SelectMode } from "./selection";

/** Main-document commands that would change the part being repaired refuse with this. */
export const CLOSE_REPAIR_FIRST = "Close repair mode first.";
export const SELECT_PART_FIRST = "Select a part to repair.";
export const SELECT_TRIANGLES_FIRST = "Select triangles first.";
/** The question closeRepair asks when the working copy changed, and its buttons. */
export const DISCARD_QUESTION = "Discard repair changes?";
export const DISCARD_LABEL = "Discard";

const repair = () => useRepair.getState();
const session = () => useRepair.getState().session;
const setSession = (next: RepairSession) => useRepair.setState({ session: next });

/** The ribbon tab shown before repair mode, shown again after it. */
let tabBefore: TabId | null = null;
/** Statuses computed for working copies, so undo and redo show them again without a run. */
const statuses = new WeakMap<MeshData, { status: RepairStatus; highlights: Highlights | null }>();

// ---------------------------------------------------------------------------------------------
// Entering and leaving

/** True while a part is in repair mode. */
export function isRepairing(): boolean {
  return session() !== null;
}

/**
 * The name of the part being repaired as it is now (`parts`: the store's). Renaming stays open in repair mode, so the
 * session's `partName`, the name on entering, only stands in for a part that was removed meanwhile.
 */
export function repairedPartName(current: Pick<RepairSession, "partId" | "partName">, parts: readonly Part[] = useStore.getState().parts): string {
  return parts.find((p) => p.id === current.partId)?.name ?? current.partName;
}

/** Home › Repair, the quick strip and the part context menu: the selected part, or a choice when several are selected. */
export function startRepair(): void {
  if (isRepairing()) {
    toast(CLOSE_REPAIR_FIRST);
    return;
  }
  const s = useStore.getState();
  const known = new Set(s.parts.map((p) => p.id));
  const ids = s.selection.filter((id) => known.has(id));
  if (ids.length === 0) toast(SELECT_PART_FIRST);
  else if (ids.length === 1) enterRepair(ids[0]);
  else useUi.getState().openDialog({ kind: "repair-choose-part", ids });
}

let watching = false;

/** Leaves repair mode whenever New project or Open project replaces the document under it (subscribes once). */
function watchProject() {
  if (watching) return;
  watching = true;
  useStore.subscribe((s, prev) => {
    if (s.generation !== prev.generation && isRepairing()) leave();
  });
}

/**
 * Starts repairing a part: its matrix is baked into a world-space working copy, the Repair ribbon tab opens, and
 * the status is computed. Refused (with a toast) while a repair is open, a run is going, or a Cut, Boolean, Split or
 * Merge is still working (it could replace the part).
 */
export function enterRepair(partId: string): void {
  if (isRepairing()) {
    toast(CLOSE_REPAIR_FIRST);
    return;
  }
  if (isLabelOpen()) {
    toast(CLOSE_LABEL_FIRST);
    return;
  }
  if (repair().busy || meshOperationRunning()) {
    toast(WAIT_MESSAGE);
    return;
  }
  const store = useStore.getState();
  const part = store.parts.find((p) => p.id === partId);
  if (!part) {
    toast("That part no longer exists.");
    return;
  }
  if (triangleCount(part.mesh) === 0) {
    toast(`"${part.name}" has no triangles to repair.`);
    return;
  }
  // The actions take two vertices at one position for an open seam; import never leaves any, but a bake can.
  const baked = bakeMesh(part.mesh, part.matrix);
  const mesh = baked === part.mesh ? baked : weldMesh(baked);
  const known = statuses.get(mesh);
  setSession({
    partId,
    partName: part.name,
    initialMesh: mesh,
    mesh,
    selection: new Uint8Array(triangleCount(mesh)),
    selectedCount: 0,
    past: [],
    future: [],
    status: known?.status ?? null,
    highlights: known?.highlights ?? null,
    intersecting: null,
    lastRun: null,
    hiddenTriangles: null,
  });
  useRepair.setState({ panelTab: "status" });
  watchProject();
  const ui = useUi.getState();
  tabBefore = ui.ribbonTab === "repair" ? null : ui.ribbonTab;
  ui.closeContextMenu();
  ui.setRibbonTab("repair");
  if (store.gizmoMode) store.setGizmoMode(null);
  store.select([partId]);
  void loadScripts();
  if (!known) void refreshStatus();
}

/**
 * Ends repair mode without touching the parts: drops the session, cancels a run, closes repair dialogs, and lets go of
 * the picking and selection caches, which would otherwise keep the last working copy alive.
 */
function leave() {
  const busy = repair().busy;
  if (busy) cancelRepair(busy.job).catch((err: unknown) => console.error("repair_cancel failed", err));
  useRepair.setState({ session: null, busy: null, panelTab: "status" });
  clearSelectionCaches();
  clearTriangleBvh();
  const ui = useUi.getState();
  if (ui.dialog?.kind.startsWith("repair-")) ui.closeDialog();
  if (ui.ribbonTab === "repair") ui.setRibbonTab(tabBefore ?? "modify");
  tabBefore = null;
}

/**
 * True when `a` and `b` are the same triangles over the same vertices, bit for bit: the same object or equal arrays.
 * Apply and Close repair compare the working copy with the part this way, never by what the runs reported.
 */
export function sameMesh(a: MeshData, b: MeshData): boolean {
  return a === b || (sameBits(a.indices, b.indices) && sameBits(a.positions, b.positions));
}

function sameBits(a: Float32Array | Uint32Array, b: Float32Array | Uint32Array): boolean {
  if (a.length !== b.length) return false;
  const x = new Uint32Array(a.buffer, a.byteOffset, a.length);
  const y = new Uint32Array(b.buffer, b.byteOffset, b.length);
  for (let i = 0; i < x.length; i++) if (x[i] !== y[i]) return false;
  return true;
}

/** True when the working copy differs from the part as it was on entering repair mode. */
export function workingCopyChanged(current: Pick<RepairSession, "mesh" | "initialMesh">): boolean {
  return !sameMesh(current.mesh, current.initialMesh);
}

/** Apply repair (ribbon and panel button): opens the Apply dialog. Does nothing while a run is going. */
export function requestApply(): void {
  if (!isRepairing() || repair().busy) return;
  useUi.getState().openDialog({ kind: "repair-apply" });
}

/**
 * The Apply dialog's choice, as one main undo step "Repair". remove: the repaired part takes the old one's place,
 * name, colour and display flags; keep: it goes right after the old part as "<name> (repaired)" in a fresh colour;
 * compare: like keep, with the old part made transparent in the same step. The new part has the identity matrix
 * (the working copy is world space). Leaves repair mode. A working copy equal to the part changes nothing.
 */
export function applyRepair(mode: ApplyMode): void {
  const current = session();
  if (!current || repair().busy) return;
  if (!workingCopyChanged(current)) {
    leave();
    toast("Nothing was changed, so the part stays as it was.");
    return;
  }
  const mesh = current.mesh;
  if (triangleCount(mesh) === 0) {
    toast("The repaired part has no triangles left. Undo the last step, or close repair mode.", "warn");
    return;
  }
  // The badge needs no check of its own when the status of this exact mesh is known.
  if (current.status) primeCheck(mesh, checkFromStatus(current.status));
  const store = useStore.getState();
  const part = store.parts.find((p) => p.id === current.partId);
  if (!part) {
    store.addParts([{ name: current.partName, mesh }], "Repair");
    toast(`The original part was removed meanwhile, so "${current.partName}" was added as a new part.`, "warn");
  } else if (mode === "remove") {
    const { name, color, visible, transparent, locked, packing } = part;
    store.replaceParts([{ remove: [part.id], add: [{ name, mesh, color, visible, transparent, locked, packing, source: part.source }] }], "Repair");
  } else {
    const repaired = { name: `${part.name} (repaired)`, mesh, color: freshColors([part.color], 1)[0], source: part.source };
    const patch = mode === "compare" ? { ids: [part.id], patch: { transparent: true, visible: true } } : undefined;
    store.replaceParts([{ remove: [], add: [repaired], after: part.id }], "Repair", patch);
  }
  leave();
}

/** True when the message box's answer means Discard (the custom button's label, or a platform's plain Ok). */
export function isDiscard(answer: string): boolean {
  return answer === DISCARD_LABEL || answer === "Ok";
}

async function askDiscard(): Promise<boolean> {
  // Plain browser (development): no native dialog.
  if (!isTauri()) return window.confirm(DISCARD_QUESTION);
  try {
    const answer = await message(DISCARD_QUESTION, {
      title: "Close repair",
      kind: "warning",
      buttons: { ok: DISCARD_LABEL, cancel: "Cancel" },
    });
    return isDiscard(answer);
  } catch (err) {
    toast(errorMessage(err, "Could not ask about the repair changes"), "error");
    return false;
  }
}

/**
 * Close repair: leaves repair mode without applying. Asks "Discard repair changes?" first when the working copy
 * differs from the one repair mode started with. Resolves true when repair mode is closed (or was not open), false
 * when the user kept it open or a run is going.
 */
export async function closeRepair(): Promise<boolean> {
  const current = session();
  if (!current) return true;
  if (repair().busy) {
    toast("Wait for the repair to finish, or cancel it.");
    return false;
  }
  if (workingCopyChanged(current) && !(await askDiscard())) return false;
  if (session()?.initialMesh === current.initialMesh) leave();
  return true;
}

// ---------------------------------------------------------------------------------------------
// Runs

interface RunRequest {
  /** The action's or script's name: progress, undo label and messages. */
  label: string;
  steps: ScriptStep[];
  selection: Uint32Array;
  withStatus: boolean;
  /** Scripts run without a selection and leave none. */
  clearSelection: boolean;
  /** Toast the run's summary (not for a status update). */
  announce: boolean;
  /** Keep the reports for the Repair scripts tab's Last run section. */
  lastRun: boolean;
}

function onProgress(job: number, p: RepairProgress) {
  useRepair.setState((s) =>
    s.busy?.job === job
      ? {
          busy: {
            ...s.busy,
            progress: p.fraction,
            step: p.steps > 1 ? `${p.label} (${p.step + 1} of ${p.steps})` : p.label,
            detail: p.detail ?? null,
            local: p.local ?? null,
          },
        }
      : s,
  );
}

/**
 * The session after a run that started from `now.mesh` with the selection `started` (the mask when the run began).
 * The undo step goes back to that selection, as every step does, even when it was changed while the run went on;
 * the result's selection is Rust's remap of it, or the one an action set (`selected`: Select by criteria, an edit
 * that leaves nothing selected), which applies even when the mesh did not change. Hidden triangles stay unselected.
 */
function withResult(now: RepairSession, started: Uint8Array, result: RunResult, request: RunRequest): RepairSession {
  const next: RepairSession = { ...now };
  if (request.lastRun) next.lastRun = result.reports;
  if (result.status) statuses.set(result.mesh, { status: result.status, highlights: result.highlights });
  const triangles = triangleCount(result.mesh);
  if (result.changed) {
    next.past = pushStep(now.past, { label: request.label, mesh: now.mesh, selection: started }, [result.mesh, now.initialMesh]);
    next.future = [];
    next.mesh = result.mesh;
    next.selection = request.clearSelection ? new Uint8Array(triangles) : idsToMask(result.selection, triangles);
    next.selectedCount = request.clearSelection ? 0 : countSelected(next.selection);
    next.status = result.status;
    next.highlights = result.highlights;
    next.intersecting = result.intersecting;
    next.hiddenTriangles = null;
  } else {
    if (request.clearSelection && now.selectedCount > 0) {
      next.selection = new Uint8Array(triangles);
      next.selectedCount = 0;
    } else if (result.selected) {
      next.selection = idsToMask(withoutHidden(result.selection, now.hiddenTriangles), triangles);
      next.selectedCount = countSelected(next.selection);
    }
    if (result.status) {
      next.status = result.status;
      next.highlights = result.highlights;
    }
    if (result.intersecting) next.intersecting = result.intersecting;
  }
  return next;
}

/**
 * Puts a mesh tool's result (src/modify: Triangle reduction, Smoothing… run on the working copy) into the session as
 * one inner undo step labelled `label`, if the working copy is still `from`, the mesh the tool ran on. The selection
 * follows its triangles through `origin` (the input triangle of each result triangle; null when the triangles are the
 * input's in order). Returns false, changing nothing, when the working copy changed meanwhile or a run is going.
 */
export function applyToolResult(label: string, from: MeshData, mesh: MeshData, origin: Uint32Array | null): boolean {
  const now = session();
  if (!now || now.mesh !== from || repair().busy) return false;
  const count = triangleCount(mesh);
  const picked: number[] = [];
  for (let t = 0; t < count; t++) {
    const was = origin ? origin[t] : t;
    if (was < now.selection.length && now.selection[was]) picked.push(t);
  }
  const result: RunResult = {
    changed: true,
    mesh,
    stats: null,
    edges: null,
    selection: Uint32Array.from(picked),
    selected: false,
    reports: [],
    status: null,
    highlights: null,
    intersecting: null,
    warnings: [],
  };
  const request: RunRequest = { label, steps: [], selection: result.selection, withStatus: false, clearSelection: false, announce: false, lastRun: false };
  setSession(withResult(now, now.selection, result, request));
  if (repair().autoUpdate) void refreshStatus();
  return true;
}

/**
 * Opens the progress dialog of run `job` as soon as no other dialog is open, if the run is still going then. It never
 * takes another dialog's place: a prompt replaced counts as cancelled (src/ui.ts), so a file dropped on the window
 * during the run would silently not be imported.
 */
async function showProgress(job: number) {
  while (useUi.getState().dialog !== null) await dialogSlotFree();
  if (repair().busy?.job === job) useUi.getState().openDialog({ kind: "repair-progress" });
}

/**
 * Runs `request` on the working copy as the one repair run, and puts the result into the session. Resolves to the
 * result, or null when the run was refused, cancelled, failed (after a toast), or repair mode closed meanwhile.
 */
async function run(request: RunRequest): Promise<RunResult | null> {
  const started = session();
  if (!started) return null;
  if (repair().busy) {
    toast(WAIT_MESSAGE);
    return null;
  }
  if (browserOnly("Repair")) return null;
  const job = nextJob();
  useRepair.setState({ busy: { job, label: request.label, progress: null, step: null, detail: null, local: null, cancelling: false } });
  const timer = setTimeout(() => void showProgress(job), BUSY_DELAY_MS);
  try {
    const result = await runRepair(started.mesh, request.steps, {
      job,
      label: request.label,
      selection: request.selection,
      withStatus: request.withStatus,
      onProgress: (p) => onProgress(job, p),
    });
    const now = session();
    // Repair mode closed, or the run was dropped, while Rust worked: the result belongs to nothing on screen. A
    // result that beat the cancel request is dropped too: Cancel always leaves the working copy as it was.
    if (!now || now.mesh !== started.mesh || repair().busy?.job !== job || repair().busy?.cancelling) return null;
    setSession(withResult(now, started.selection, result, request));
    if (request.announce) toast(runSummary(request.label, result.reports));
    for (const w of runWarnings(result.reports, result.warnings)) toast(w, "warn");
    return result;
  } catch (err) {
    if (err !== CANCELLED && repair().busy?.job === job) toast(errorMessage(err, `Could not run ${request.label}`), "error");
    return null;
  } finally {
    clearTimeout(timer);
    if (repair().busy?.job === job) useRepair.setState({ busy: null });
    const ui = useUi.getState();
    if (ui.dialog?.kind === "repair-progress" && !repair().busy) ui.closeDialog();
  }
}

/**
 * Runs one action on the working copy (Actions tab, ribbon, context menu), on the selection when the action uses
 * one. The status comes along when Auto-update is on. One inner undo step when the mesh changed.
 */
export async function runAction(id: ActionId, params?: Record<string, ParamValue>, selection?: Uint32Array): Promise<void> {
  const { session: current, busy, catalog, autoUpdate } = repair();
  if (!current) return;
  if (busy) {
    toast(WAIT_MESSAGE);
    return;
  }
  const spec = catalog ? actionSpec(catalog, id) : undefined;
  if (spec && !spec.implemented) {
    toast(`${spec.label} is coming in a future version.`);
    return;
  }
  if (spec?.needsSelection && current.selectedCount === 0) {
    toast(SELECT_TRIANGLES_FIRST);
    return;
  }
  await run({
    label: spec?.label ?? id,
    steps: [{ type: id, enabled: true, params: params ?? {} }],
    selection: selection ?? maskToIds(current.selection),
    withStatus: autoUpdate,
    clearSelection: false,
    announce: true,
    lastRun: false,
  });
}

/**
 * Runs a script (saved or as edited) on the working copy, without a selection, with the progress dialog after
 * BUSY_DELAY_MS and Cancel. The status is always refreshed. One inner undo step labelled with the script's name.
 */
export async function runScript(script: RepairScript): Promise<void> {
  const { session: current, busy, catalog } = repair();
  if (!current) return;
  if (busy) {
    toast(WAIT_MESSAGE);
    return;
  }
  const problems = catalog ? scriptProblems(script, catalog) : [];
  if (problems.length) {
    toast(`"${script.name}" cannot run. ${problems[0]}`, "error");
    return;
  }
  if (enabledSteps(script).length === 0) {
    toast(`"${script.name}" has no enabled actions to run.`);
    return;
  }
  await run({
    label: script.name,
    steps: script.actions,
    selection: new Uint32Array(0),
    withStatus: true,
    clearSelection: true,
    announce: true,
    lastRun: true,
  });
}

/**
 * Runs Move nodes or Cut surfaces: one action whose parameters (a vertex and where it goes, the points of a line) come
 * from the viewport rather than the catalog, on the selection as it is. One inner undo step when the mesh changed.
 * `withStatus` asks for the status with the result (Cut surfaces always does, so its new open edges show as holes);
 * `announce` toasts the summary (warnings always show). Resolves to the result, or null when the run was refused,
 * cancelled or failed.
 */
export async function runTool(
  id: ActionId,
  params: Record<string, unknown>,
  { withStatus, announce }: { withStatus: boolean; announce: boolean },
): Promise<RunResult | null> {
  const { session: current, busy, catalog } = repair();
  if (!current) return null;
  if (busy) {
    toast(WAIT_MESSAGE);
    return null;
  }
  const spec = catalog ? actionSpec(catalog, id) : undefined;
  return run({
    label: spec?.label ?? id,
    // A point list is no catalog parameter; Rust reads these tools' parameters itself (scripts.rs).
    steps: [{ type: id, enabled: true, params: params as Record<string, ParamValue> }],
    selection: maskToIds(current.selection),
    withStatus: withStatus || repair().autoUpdate,
    clearSelection: false,
    announce,
    lastRun: false,
  });
}

/** Several actions as one undo step, so Close holes does not leave the trivial fill behind one Undo. */
export async function runActions(label: string, ids: ActionId[]): Promise<void> {
  const { session: current, busy, catalog } = repair();
  if (!current) return;
  if (busy) {
    toast(WAIT_MESSAGE);
    return;
  }
  for (const id of ids) {
    const spec = catalog ? actionSpec(catalog, id) : undefined;
    if (spec && !spec.implemented) {
      toast(`${spec.label} is coming in a future version.`);
      return;
    }
  }
  await run({
    label,
    steps: ids.map((id) => ({ type: id, enabled: true, params: {} })),
    selection: maskToIds(current.selection),
    withStatus: repair().autoUpdate,
    clearSelection: false,
    announce: true,
    lastRun: false,
  });
}

/** The progress dialog's Cancel: asks Rust to stop the run; it then ends as cancelled and changes nothing. */
export function cancelRun(): void {
  const busy = repair().busy;
  if (!busy || busy.cancelling) return;
  useRepair.setState({ busy: { ...busy, cancelling: true } });
  cancelRepair(busy.job).catch((err: unknown) => console.error("repair_cancel failed", err));
}

/** The Status tab's Update: computes the status and highlights of the working copy as it is. */
export async function refreshStatus(): Promise<void> {
  if (!isRepairing()) return;
  await run({
    label: "Update status",
    steps: [],
    selection: new Uint32Array(0),
    withStatus: true,
    clearSelection: false,
    announce: false,
    lastRun: false,
  });
}

// ---------------------------------------------------------------------------------------------
// Inner undo

function show(current: RepairSession, move: HistoryMove) {
  const known = statuses.get(move.target.mesh);
  setSession({
    ...current,
    mesh: move.target.mesh,
    selection: move.target.selection,
    selectedCount: countSelected(move.target.selection),
    past: move.past,
    future: move.future,
    status: known?.status ?? null,
    highlights: known?.highlights ?? null,
    intersecting: null,
    hiddenTriangles: move.target.mesh === current.mesh ? current.hiddenTriangles : null,
  });
  if (!known && repair().autoUpdate) void refreshStatus();
}

function step(move: (current: RepairSession) => HistoryMove | null) {
  const { session: current, busy } = repair();
  if (!current) return;
  if (busy) {
    toast(WAIT_MESSAGE);
    return;
  }
  const target = move(current);
  if (target) show(current, target);
}

/** Ctrl/Cmd+Z in repair mode: the working copy and selection as they were before the last step. */
export function undoRepair(): void {
  step((current) => undoStep(current.past, current.future, current));
}

/** Ctrl+Shift+Z / Ctrl+Y in repair mode. */
export function redoRepair(): void {
  step((current) => redoStep(current.past, current.future, current));
}

export function canUndoRepair(): boolean {
  return (session()?.past.length ?? 0) > 0;
}

export function canRedoRepair(): boolean {
  return (session()?.future.length ?? 0) > 0;
}

// ---------------------------------------------------------------------------------------------
// Selection. Selection changes are not undo steps; an undo restores the selection a step started with.

function setMask(current: RepairSession, selection: Uint8Array, selectedCount: number) {
  if (selection !== current.selection) setSession({ ...current, selection, selectedCount });
}

/** Selects triangles: replace, add, toggle (by the first id: see applySelection) or remove. */
function withoutHidden(ids: ArrayLike<number>, hidden: Uint8Array | null): ArrayLike<number> {
  if (!hidden) return ids;
  const kept: number[] = [];
  for (let i = 0; i < ids.length; i++) if (!hidden[ids[i]]) kept.push(ids[i]);
  return kept;
}

function dropHidden(mask: Uint8Array, hidden: Uint8Array | null): void {
  if (!hidden) return;
  const n = Math.min(mask.length, hidden.length);
  for (let t = 0; t < n; t++) if (hidden[t]) mask[t] = 0;
}

export function selectTriangles(ids: ArrayLike<number>, mode: SelectMode): void {
  const current = session();
  if (!current) return;
  const next = applySelection(current.selection, current.selectedCount, withoutHidden(ids, current.hiddenTriangles), mode);
  setMask(current, next.mask, next.count);
}

export function selectAt(triangle: number, mode: SelectMode, tool: SelectTool = repair().selectTool): void {
  const { session: current, view } = repair();
  if (!current) return;
  const ids =
    tool === "surface" ? surfaceRegion(current.mesh, triangle, view.surfaceTolerance) : tool === "shell" ? shellRegion(current.mesh, triangle) : [triangle];
  selectTriangles(ids, mode);
}

export function selectAll(): void {
  const current = session();
  if (!current) return;
  const n = current.selection.length;
  if (!current.hiddenTriangles) {
    if (current.selectedCount !== n) setMask(current, new Uint8Array(n).fill(1), n);
    return;
  }
  const mask = new Uint8Array(n).fill(1);
  dropHidden(mask, current.hiddenTriangles);
  setMask(current, mask, countSelected(mask));
}

export function selectNone(): void {
  const current = session();
  if (current && current.selectedCount > 0) setMask(current, new Uint8Array(current.selection.length), 0);
}

/** Select › Toggle: every unselected triangle selected and every selected one not. */
export function invertSelection(): void {
  const current = session();
  if (!current) return;
  const mask = invertMask(current.selection);
  dropHidden(mask, current.hiddenTriangles);
  setMask(current, mask, countSelected(mask));
}

/** `=`: adds every triangle that shares a corner with the selection. */
export function expandSelection(): void {
  const current = session();
  if (!current || current.selectedCount === 0) return;
  const mask = expandMask(current.mesh, current.selection);
  dropHidden(mask, current.hiddenTriangles);
  setMask(current, mask, countSelected(mask));
}

/** `-`: drops the selected triangles that share a corner with an unselected one. */
export function shrinkSelection(): void {
  const current = session();
  if (!current || current.selectedCount === 0) return;
  const mask = shrinkMask(current.mesh, current.selection);
  dropHidden(mask, current.hiddenTriangles);
  setMask(current, mask, countSelected(mask));
}

/** Selected triangles, their area (mm²) and the shells they touch. */
export function selectionInfo(): SelectionInfo {
  const current = session();
  return current ? selectionInfoOf(current.mesh, current.selection) : { count: 0, area: 0, shells: 0 };
}

/** Info › Show selection info: the selection's count, area and shells as a toast. */
export function showSelectionInfo(): void {
  if (isRepairing()) toast(selectionInfoText(selectionInfo()));
}

/** Zoom to › Part: the working copy's box (world space), or null outside repair mode. */
export function workingCopyBounds(): Bounds | null {
  const current = session();
  return current ? boundsOf(current.mesh, IDENTITY) : null;
}

/** Zoom to › Selected: the selected triangles' box, or null when none is selected. */
export function selectedBounds(): Bounds | null {
  const current = session();
  return current && current.selectedCount > 0 ? selectionBounds(current.mesh, current.selection) : null;
}

/**
 * Extract parts: the selected triangles as a new part (main undo step "Extract part", world space, fresh colour).
 * With `separate` they also leave the working copy, as an inner undo step that runs first; the part is added only
 * when that worked.
 */
export async function extractSelection(separate: boolean): Promise<void> {
  const { session: current, busy, autoUpdate } = repair();
  if (!current) return;
  if (busy) {
    toast(WAIT_MESSAGE);
    return;
  }
  if (current.selectedCount === 0) {
    toast(SELECT_TRIANGLES_FIRST);
    return;
  }
  const piece = extractTriangles(current.mesh, current.selection);
  if (separate) {
    const result = await run({
      label: "Separate selection as part",
      steps: [{ type: "remove_selected_triangles", enabled: true, params: {} }],
      selection: maskToIds(current.selection),
      withStatus: autoUpdate,
      clearSelection: false,
      announce: false,
      lastRun: false,
    });
    if (!result?.changed) return;
  }
  const store = useStore.getState();
  const part = store.parts.find((p) => p.id === current.partId);
  const name = `${repairedPartName(current, store.parts)} (extracted)`;
  store.addParts([{ name, mesh: piece, color: freshColors(part ? [part.color] : [], 1)[0], source: part?.source }], "Extract part");
  // The tree keeps pointing at the part being repaired.
  if (part) store.select([part.id]);
  toast(`Added "${name}" with ${formatCount(triangleCount(piece))} ${triangleCount(piece) === 1 ? "triangle" : "triangles"}.`);
}

export async function withRepairSession(fn: () => void | Promise<void>): Promise<void> {
  if (!isRepairing()) {
    const store = useStore.getState();
    const known = new Set(store.parts.map((p) => p.id));
    const ids = store.selection.filter((id) => known.has(id));
    if (ids.length === 0) {
      toast(SELECT_PART_FIRST);
      return;
    }
    if (ids.length > 1) {
      useUi.getState().openDialog({ kind: "repair-choose-part", ids });
      return;
    }
    enterRepair(ids[0]);
    if (!isRepairing()) return;
    while (isRepairing() && repair().busy) await new Promise((resolve) => setTimeout(resolve, 0));
    if (!isRepairing()) return;
  }
  await fn();
}

export function hideSelectedTriangles(): { kind: "hidden"; count: number } | { kind: "shown" } | { kind: "none" } {
  const current = session();
  if (!current) return { kind: "none" };
  if (current.selectedCount > 0) {
    const hidden =
      current.hiddenTriangles && current.hiddenTriangles.length === current.selection.length ? current.hiddenTriangles.slice() : new Uint8Array(current.selection.length);
    for (let t = 0; t < current.selection.length; t++) if (current.selection[t]) hidden[t] = 1;
    setSession({ ...current, hiddenTriangles: hidden, selection: new Uint8Array(current.selection.length), selectedCount: 0 });
    return { kind: "hidden", count: current.selectedCount };
  }
  if (current.hiddenTriangles && countSelected(current.hiddenTriangles) > 0) {
    setSession({ ...current, hiddenTriangles: null });
    return { kind: "shown" };
  }
  return { kind: "none" };
}

export async function extractShells(): Promise<void> {
  const current = session();
  if (!current || current.selectedCount === 0) {
    toast(SELECT_TRIANGLES_FIRST);
    return;
  }
  const ids = new Set<number>();
  for (let t = 0; t < current.selection.length; t++) {
    if (!current.selection[t]) continue;
    for (const id of shellRegion(current.mesh, t)) ids.add(id);
  }
  selectTriangles([...ids], "replace");
  await extractSelection(false);
}
