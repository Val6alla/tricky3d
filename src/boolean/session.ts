// Boolean operations mode: entering and leaving, the two lists, the checks, the runs (Union, Difference,
// Intersection), Cancel, OK and Repair first. UI code changes Boolean state only through these functions; the state
// itself is in src/boolean/store.ts. The decisions are plain exported functions so tests run them without Tauri.
//
// The flow follows the reference tool: the selected parts start in the green list; the user moves the parts to
// subtract or intersect with to the red list and picks an operation. Every input is checked first (closed and
// oriented); a part that fails is named in the log with "Repair first", which runs Extended repair on it. The result
// shows in the viewport in place of the inputs, and OK adds it as one undo step ("Union", "Difference",
// "Intersection"), removing the originals unless that setting is off. Runs follow the rules of src/ops/meshOps.ts: one
// at a time, a cancelled or outdated run changes nothing, and nothing partial is ever added.
import { isTauri } from "@tauri-apps/api/core";
import { message } from "@tauri-apps/plugin-dialog";
import type { TabId } from "../features";
import { formatCount, triangleCount } from "../info";
import { browserOnly, errorMessage } from "../io/files";
import type { AnalyzedMesh } from "../io/ipc";
import { IDENTITY, boundsOf, unionBounds, type Bounds } from "../mesh";
import { primeMeshCaches } from "../meshCache";
import { bakeMesh, weldMesh } from "../ops/bake";
import { cancelManifold, manifoldBoolean } from "../ops/manifoldClient";
import type { BooleanOp } from "../ops/manifoldOps";
import {
  badIndices,
  commit,
  freshColors,
  fromWorker,
  leftOutMessage,
  measure,
  meshOperationRunning,
  scopeTargets,
  staleMessage,
  targetProblem,
  unionShellMessage,
  WAIT_MESSAGE,
  type OpWords,
} from "../ops/meshOps";
import { badgeText, cachedCheck, checkFromStatus, primeCheck } from "../repair/badges";
import { cm3 } from "../repair/format";
import { CANCELLED, cancelRepair, checkMeshes, runRepair } from "../repair/ipc";
import { loadScripts, scriptById } from "../repair/scripts";
import { CLOSE_REPAIR_FIRST, isRepairing } from "../repair/session";
import type { RepairCheck, ScriptStep } from "../repair/types";
import { useStore, type MeshData, type Part } from "../store";
import { toast, useUi } from "../ui";
import { viewport } from "../viewport/api";
import { useBoolean, type BooleanBusy, type BooleanList, type BooleanResult, type BooleanSession, type InputSnapshot, type LogKind } from "./store";
import { CLOSE_LABEL_FIRST, isLabelOpen } from "../label/session";

// ---------------------------------------------------------------------------------------------
// Words and checks (pure)

/** Main-document commands that would change the parts in the lists refuse with this while the mode is open. */
export const CLOSE_BOOLEAN_FIRST = "Close Boolean operations first.";
export const BOOLEAN_WORDS: OpWords = { verb: "combine", past: "combined" };
export const CHECK_SUCCESSFUL = "Check successful: every part is closed and oriented.";
export const NO_RESULT = "Run Union, Difference or Intersection first.";
export const REPAIR_FIRST_HINT = "Repair these parts first (Repair first runs Extended repair), then run the operation again.";
export const DISCARD_QUESTION = "Discard the Boolean result?";
/** The id the result is drawn with: never a part id (those are "part-1", "part-2"…). */
export const RESULT_ID = "boolean-result";
/** The log keeps this many lines; older ones scroll away. */
export const LOG_LINES = 200;
/** Extended repair, run by Repair first (src-tauri/src/repair/defaults.rs EXTENDED_ID). */
export const EXTENDED_SCRIPT = "builtin.extended";

export const OP_NAMES: Record<BooleanOp, string> = { union: "Union", difference: "Difference", intersection: "Intersection" };

/** Why `op` cannot run on lists of these sizes, or null when it can. */
export function opProblem(op: BooleanOp, main: number, tool: number): string | null {
  if (op === "union") return main + tool >= 2 ? null : "Union needs at least 2 parts in the lists.";
  if (main === 0) return `${OP_NAMES[op]} needs at least one part in the green list.`;
  if (tool === 0) return `${OP_NAMES[op]} needs at least one part in the red list. Move a part there with the arrow button.`;
  return null;
}

/** The new part's name: "Union of A, B" for two parts, "Difference of 3 parts" for more (main list first). */
export function booleanName(op: BooleanOp, names: readonly string[]): string {
  const of = names.length === 2 ? `${names[0]}, ${names[1]}` : names.length === 1 ? names[0] : `${names.length} parts`;
  return `${OP_NAMES[op]} of ${of}`;
}

/** The log line for a part that fails the check: "Bracket is not closed: 4 holes." */
export function checkProblem(name: string, check: RepairCheck): string {
  const what = [!check.closed && "not closed", !check.oriented && "not oriented"].filter(Boolean).join(" and ");
  const detail = badgeText(check)?.replace(/^Needs repair: /, "") ?? "";
  return detail && !detail.startsWith("the mesh is") ? `${name} is ${what}: ${detail}.` : `${name} is ${what}.`;
}

/** The log line when the result is empty. */
export function emptyMessage(op: BooleanOp): string {
  if (op === "intersection") return "The result is empty: the green and red parts do not overlap.";
  if (op === "difference") return "The result is empty: the red parts cover the green parts completely.";
  return "The result is empty.";
}

/** "Union ready: 12.000 cm³, 24 triangles, 1 shell. Click OK to add it as a part." */
export function resultMessage(op: BooleanOp, measured: AnalyzedMesh): string {
  const { volume, triangles, shells } = measured.stats;
  const tris = `${formatCount(triangles)} ${triangles === 1 ? "triangle" : "triangles"}`;
  const shellText = `${formatCount(shells)} ${shells === 1 ? "shell" : "shells"}`;
  return `${OP_NAMES[op]} ready: ${cm3(volume)} cm³, ${tris}, ${shellText}. Click OK to add it as a part.`;
}

export function noiseMessage(count: number, volume: number): string {
  return `Removed ${formatCount(count)} noise ${count === 1 ? "shell" : "shells"} smaller than ${volume} cm³.`;
}

/** The session with the selected part moved to the end of `to`; unchanged when it is already there or none is selected. */
export function movedTo(s: BooleanSession, to: BooleanList): BooleanSession {
  const id = s.selected;
  const from: BooleanList = to === "main" ? "tool" : "main";
  if (id === null || !s[from].includes(id)) return s;
  return { ...s, [from]: s[from].filter((x) => x !== id), [to]: [...s[to], id], result: null };
}

/** Which list `id` is in, or null. */
export function listOf(s: Pick<BooleanSession, "main" | "tool">, id: string): BooleanList | null {
  return s.main.includes(id) ? "main" : s.tool.includes(id) ? "tool" : null;
}

/** The session without the ids that name no part any more (deleted, or replaced by Repair first). */
export function pruned(s: BooleanSession, parts: readonly Part[]): BooleanSession {
  const live = new Set(parts.map((p) => p.id));
  const main = s.main.filter((id) => live.has(id));
  const tool = s.tool.filter((id) => live.has(id));
  if (main.length === s.main.length && tool.length === s.tool.length) return s;
  return { ...s, main, tool, selected: s.selected !== null && live.has(s.selected) ? s.selected : null };
}

/** True when the parts no longer match the inputs a result was computed from (moved, repaired, deleted). */
export function inputsChanged(inputs: readonly InputSnapshot[], parts: readonly Part[]): boolean {
  const byId = new Map(parts.map((p) => [p.id, p]));
  return inputs.some((i) => {
    const p = byId.get(i.id);
    return !p || p.mesh !== i.mesh || p.matrix !== i.matrix;
  });
}

const snapshot = (p: Part): InputSnapshot => ({ id: p.id, mesh: p.mesh, matrix: p.matrix });

/** The step Remove degenerate faces runs on a result (src-tauri/src/repair/degenerate.rs). */
export function degenerateStep(tolerance: number): ScriptStep {
  return { type: "remove_degenerate_faces", enabled: true, params: { calc_tolerance: false, tolerance } };
}

// ---------------------------------------------------------------------------------------------
// State helpers

const state = () => useBoolean.getState();
const session = () => useBoolean.getState().session;

let logSeq = 0;
/** Job ids of Boolean runs, apart from repair mode's own (both go to repair_run). */
let jobSeq = 2 ** 32;
let tabBefore: TabId | null = null;

function setSession(next: BooleanSession) {
  useBoolean.setState({ session: next });
}

/** Adds a line to the log of the open session. */
export function log(kind: LogKind, text: string, repair?: string): void {
  const current = session();
  if (!current) return;
  const line = { id: ++logSeq, kind, text, ...(repair ? { repair } : {}) };
  setSession({ ...current, log: [...current.log, line].slice(-LOG_LINES) });
}

/** True while Boolean operations is open. */
export function isBooleanOpen(): boolean {
  return session() !== null;
}

const partsOf = (ids: readonly string[], parts: readonly Part[] = useStore.getState().parts): Part[] => {
  const byId = new Map(parts.map((p) => [p.id, p]));
  return ids.flatMap((id) => byId.get(id) ?? []);
};

// ---------------------------------------------------------------------------------------------
// Entering and leaving

let watching = false;

/**
 * Keeps the session in line with the parts: ids of parts that are gone leave the lists, a result whose inputs changed
 * is dropped, and New or Open project ends the mode (subscribes once).
 */
function watchParts() {
  if (watching) return;
  watching = true;
  useStore.subscribe((s, prev) => {
    const current = session();
    if (!current) return;
    if (s.generation !== prev.generation) {
      leave();
      return;
    }
    if (s.parts === prev.parts) return;
    let next = pruned(current, s.parts);
    if (next.result && inputsChanged(next.result.inputs, s.parts)) {
      next = { ...next, result: null };
      setSession(next);
      log("warn", "The parts changed, so the result was dropped. Run the operation again.");
      return;
    }
    if (next !== current) setSession(next);
  });
}

/**
 * Modify › Boolean: opens the Boolean operations tab and panel with the selected shown, unlocked parts (at least 2)
 * in the green list. Refused while repair mode is open or another mesh operation runs.
 */
export function openBooleanMode(): void {
  const ui = useUi.getState();
  if (isBooleanOpen()) {
    ui.setRibbonTab("boolean");
    return;
  }
  if (isRepairing()) {
    toast(CLOSE_REPAIR_FIRST);
    return;
  }
  if (isLabelOpen()) {
    toast(CLOSE_LABEL_FIRST);
    return;
  }
  if (meshOperationRunning()) {
    toast(WAIT_MESSAGE);
    return;
  }
  const store = useStore.getState();
  const t = scopeTargets(store, "selected");
  const problem = targetProblem(t, BOOLEAN_WORDS, "selected", 2);
  if (problem) {
    toast(problem.message, problem.kind);
    return;
  }
  const leftOut = leftOutMessage(t, "added to the lists");
  if (leftOut) toast(leftOut, "warn");
  const main = t.parts.map((p) => p.id);
  setSession({ main, tool: [], selected: null, log: [], result: null });
  useBoolean.setState({ busy: null, tool: "select" });
  log("info", `${formatCount(main.length)} parts in the green list. Move the parts to subtract or intersect with to the red list.`);
  watchParts();
  tabBefore = ui.ribbonTab === "boolean" ? null : ui.ribbonTab;
  ui.closeContextMenu();
  ui.setRibbonTab("boolean");
  if (store.gizmoMode) store.setGizmoMode(null);
}

/** Ends the mode without touching the parts: drops the session and stops a run. */
function leave() {
  const busy = state().busy;
  if (busy) stopRun(busy);
  useBoolean.setState({ session: null, busy: null, tool: "select" });
  const ui = useUi.getState();
  if (ui.ribbonTab === "boolean") ui.setRibbonTab(tabBefore ?? "modify");
  tabBefore = null;
}

/** "Discard the Boolean result?": true for Discard. */
async function askDiscard(): Promise<boolean> {
  // Plain browser (development): no native dialog.
  if (!isTauri()) return window.confirm(DISCARD_QUESTION);
  try {
    const answer = await message(DISCARD_QUESTION, {
      title: "Close Boolean operations",
      kind: "warning",
      buttons: { ok: "Discard", cancel: "Cancel" },
    });
    return answer === "Discard" || answer === "Ok";
  } catch (err) {
    toast(errorMessage(err, "Could not ask about the result"), "error");
    return false;
  }
}

/**
 * Close Boolean operations: leaves without adding anything. Asks first when a result waits for OK. Resolves true when
 * the mode is closed (or was not open), false when it stays open.
 */
export async function closeBooleanMode(): Promise<boolean> {
  const current = session();
  if (!current) return true;
  if (state().busy) {
    toast("Wait for the operation to finish, or cancel it.");
    return false;
  }
  if (current.result && !(await askDiscard())) return false;
  if (session() === null) return true;
  leave();
  return true;
}

/**
 * OK: adds the result as a new part, one undo step named after the operation, and leaves the mode. The originals go
 * unless Remove original parts is off. Without a result, says what to do first.
 */
export function acceptBoolean(): void {
  const current = session();
  if (!current) return;
  if (state().busy) {
    toast(WAIT_MESSAGE);
    return;
  }
  const result = current.result;
  if (!result) {
    toast(NO_RESULT);
    return;
  }
  const parts = useStore.getState().parts;
  if (inputsChanged(result.inputs, parts)) {
    setSession({ ...current, result: null });
    log("warn", "The parts changed, so the result was dropped. Run the operation again.");
    return;
  }
  const inputs = partsOf(result.inputs.map((i) => i.id), parts);
  const removeIds = state().settings.removeOriginals ? inputs.map((p) => p.id) : [];
  const from = inputs.map((p) => p.id);
  commit({ groups: [{ removeIds, from, made: [{ name: result.name, result: result.measured }] }] }, inputs, OP_NAMES[result.op]);
  leave();
}

// ---------------------------------------------------------------------------------------------
// Lists

/** A click in a list or on a part in the viewport: that part becomes the selected one (null clears). */
export function selectInList(id: string | null): void {
  const current = session();
  if (!current || current.selected === id) return;
  if (id !== null && listOf(current, id) === null) return;
  setSession({ ...current, selected: id });
}

/** The arrow buttons: moves the selected part to the green (`main`) or red (`tool`) list. Drops a shown result. */
export function moveSelected(to: BooleanList): void {
  const current = session();
  if (!current || state().busy) return;
  const next = movedTo(current, to);
  if (next !== current) setSession(next);
}

/** Zoom to part: the part selected in the lists, or every part in them when none is. */
export function zoomToBooleanPart(): void {
  const current = session();
  if (!current) return;
  const ids = current.selected ? [current.selected] : [...current.main, ...current.tool];
  const box: Bounds | null = unionBounds(partsOf(ids).map((p) => boundsOf(p.mesh, p.matrix)));
  if (box) viewport.zoomToBounds(box);
}

// ---------------------------------------------------------------------------------------------
// Runs

function startBusy(label: string): number {
  const job = ++jobSeq;
  useBoolean.setState({ busy: { job, label, progress: null, step: null, cancelling: false } });
  return job;
}

function setStep(job: number, progress: number | null, step: string) {
  useBoolean.setState((s) => (s.busy?.job === job ? { busy: { ...s.busy, progress, step } } : s));
}

function endBusy(job: number) {
  if (state().busy?.job === job) useBoolean.setState({ busy: null });
}

/** The run `job` is still the one going, and nobody asked it to stop. */
function live(job: number): boolean {
  const busy = state().busy;
  return session() !== null && busy?.job === job && !busy.cancelling;
}

/** Stops a run in the worker and in Rust; whichever it is in rejects with CANCELLED. */
function stopRun(busy: BooleanBusy) {
  cancelManifold();
  cancelRepair(busy.job).catch((err: unknown) => console.error("repair_cancel failed", err));
}

/**
 * Cancel: stops the run in progress (it then changes nothing), or when none runs, drops the result shown, so the
 * lists can be changed and run again.
 */
export function cancelBoolean(): void {
  const current = session();
  if (!current) return;
  const busy = state().busy;
  if (busy) {
    if (busy.cancelling) return;
    useBoolean.setState({ busy: { ...busy, cancelling: true, step: "Cancelling" } });
    stopRun(busy);
    return;
  }
  if (current.result) {
    setSession({ ...current, result: null });
    log("info", "Result discarded.");
  }
}

/** Closed and oriented, per part: the badge's cached check, else asked of Rust now. */
async function checksOf(parts: readonly Part[]): Promise<RepairCheck[]> {
  const known = parts.map((p) => cachedCheck(p.mesh));
  const missing = parts.filter((_, i) => !known[i]);
  if (missing.length === 0) return known as RepairCheck[];
  const fresh = await checkMeshes(missing.map((p) => p.mesh));
  missing.forEach((p, k) => {
    if (fresh[k]) primeCheck(p.mesh, fresh[k]);
  });
  let next = 0;
  return known.map((c) => c ?? fresh[next++]);
}

/**
 * Union, Difference or Intersection: checks the parts, computes the result in the worker, then (when set) removes
 * degenerate faces in Rust and measures it, and shows it for OK. Every step logs; nothing changes in the parts.
 */
export async function runBoolean(op: BooleanOp): Promise<void> {
  const current = session();
  if (!current) return;
  if (state().busy) {
    toast(WAIT_MESSAGE);
    return;
  }
  if (browserOnly("Boolean operations")) return;
  const problem = opProblem(op, current.main.length, current.tool.length);
  if (problem) {
    log("warn", problem);
    return;
  }
  const main = partsOf(current.main);
  const tool = partsOf(current.tool);
  const inputs = [...main, ...tool];
  const settings = state().settings;
  const name = OP_NAMES[op];
  if (current.result) setSession({ ...current, result: null });
  const job = startBusy(name);
  try {
    setStep(job, null, "Checking the parts");
    const checks = await checksOf(inputs);
    if (!live(job)) return cancelled(job, name);
    const failing = inputs.flatMap((p, i) => (checks[i] && !(checks[i].closed && checks[i].oriented) ? [{ p, check: checks[i] }] : []));
    if (failing.length) {
      for (const { p, check } of failing) log("error", checkProblem(p.name, check), p.id);
      log("info", REPAIR_FIRST_HINT);
      return;
    }
    log("ok", CHECK_SUCCESSFUL);
    const noiseVolume = settings.filterNoise ? settings.noiseVolume : 0;
    const outcome = await fromWorker(
      manifoldBoolean({ kind: op, main, tool, noiseVolume: noiseVolume * 1000 }, (fraction, step) => setStep(job, 0.05 + 0.8 * fraction, step)),
    );
    if (!live(job)) return cancelled(job, name);
    if (outcome.kind === "invalid") {
      for (const i of badIndices(outcome.indices, inputs.length)) {
        log("error", `${inputs[i].name} is not a solid the Boolean can use (it may touch itself or have stray triangles).`, inputs[i].id);
      }
      log("info", REPAIR_FIRST_HINT);
      return;
    }
    if (outcome.noise > 0) log("info", noiseMessage(outcome.noise, noiseVolume));
    if (triangleCount(outcome.mesh) === 0) {
      log("warn", emptyMessage(op));
      return;
    }
    let measured: AnalyzedMesh | null = null;
    if (settings.removeDegenerate) {
      setStep(job, 0.85, "Removing degenerate faces");
      const run = await runRepair(outcome.mesh, [degenerateStep(settings.degenerateTolerance)], {
        job,
        label: "Remove degenerate faces",
        selection: new Uint32Array(0),
        withStatus: false,
      });
      if (!live(job)) return cancelled(job, name);
      if (run.reports[0]?.message) log("info", run.reports[0].message);
      if (run.changed && run.stats && run.edges) measured = { mesh: run.mesh, stats: run.stats, edges: run.edges };
      else if (run.changed) [measured] = await measure([run.mesh]);
    }
    setStep(job, 0.95, "Measuring the result");
    if (!measured) [measured] = await measure([outcome.mesh]);
    if (!live(job)) return cancelled(job, name);
    const parts = useStore.getState().parts;
    const snapshots = inputs.map(snapshot);
    if (inputsChanged(snapshots, parts)) {
      log("warn", staleMessage(name));
      return;
    }
    primeMeshCaches(measured.mesh, { stats: measured.stats, edges: measured.edges });
    const color = freshColors(inputs.map((p) => p.color), 1)[0];
    const part: Part = { id: RESULT_ID, name: booleanName(op, inputs.map((p) => p.name)), mesh: measured.mesh, matrix: [...IDENTITY], visible: true, transparent: false, locked: false, color };
    const result: BooleanResult = { op, name: part.name, part, measured, inputs: snapshots };
    const now = session();
    if (now) setSession({ ...now, result });
    log("ok", resultMessage(op, measured));
    const shells = op === "union" ? unionShellMessage(measured.stats.shells, outcome.bodies) : null;
    if (shells) log("info", shells);
  } catch (err) {
    if (err === CANCELLED) cancelled(job, name);
    else if (session()) log("error", errorMessage(err, `Could not compute the ${name.toLowerCase()}`));
  } finally {
    endBusy(job);
  }
}

function cancelled(job: number, name: string) {
  if (session() && state().busy?.job === job) log("info", `${name} was cancelled. Nothing changed.`);
}

/**
 * Repair first, from a log line: runs Extended repair on the part (world space, as repair mode does) and puts the
 * repaired part in its place, as one undo step "Repair", keeping its name, colour, display flags and place in the
 * lists. Logs whether it is closed and oriented now.
 */
export async function repairFirst(partId: string): Promise<void> {
  const current = session();
  if (!current) return;
  if (state().busy) {
    toast(WAIT_MESSAGE);
    return;
  }
  if (browserOnly("Repair")) return;
  const part = useStore.getState().parts.find((p) => p.id === partId);
  if (!part) {
    log("warn", "That part no longer exists.");
    return;
  }
  const job = startBusy(`Repairing ${part.name}`);
  try {
    setStep(job, null, "Loading Extended repair");
    const script = (await loadScripts()) ? scriptById(EXTENDED_SCRIPT) : undefined;
    if (!live(job)) return cancelled(job, "Repair");
    if (!script) {
      log("error", "Extended repair could not be loaded.");
      return;
    }
    const baked = bakeMesh(part.mesh, part.matrix);
    // The actions take two vertices at one position for an open seam; import never leaves any, but a bake can.
    const mesh = baked === part.mesh ? baked : weldMesh(baked);
    const run = await runRepair(mesh, script.actions, {
      job,
      label: script.name,
      selection: new Uint32Array(0),
      withStatus: true,
      onProgress: (p) => setStep(job, p.fraction, p.steps > 1 ? `${p.label} (${p.step + 1} of ${p.steps})` : p.label),
    });
    if (!live(job)) return cancelled(job, "Repair");
    const now = useStore.getState().parts.find((p) => p.id === partId);
    if (!now || now.mesh !== part.mesh || now.matrix !== part.matrix) {
      log("warn", `${part.name} changed while it was repaired, so the repair was dropped.`);
      return;
    }
    const check = run.status ? checkFromStatus(run.status) : null;
    if (run.changed) replaceRepaired(now, run.mesh, check);
    else if (check) primeCheck(now.mesh, check);
    if (!check) log("info", `${part.name}: Extended repair finished.`);
    else if (check.closed && check.oriented) log("ok", `${part.name} is closed and oriented now.`);
    else log("warn", `${checkProblem(part.name, check)} Extended repair could not fix it. Close Boolean operations and repair it in repair mode.`);
  } catch (err) {
    if (err === CANCELLED) cancelled(job, "Repair");
    else if (session()) log("error", errorMessage(err, `Could not repair ${part.name}`));
  } finally {
    endBusy(job);
  }
}

/** The repaired mesh in the part's place (one undo step "Repair"), and in its place in the lists. */
function replaceRepaired(part: Part, mesh: MeshData, check: RepairCheck | null) {
  if (check) primeCheck(mesh, check);
  const before = session();
  const list = before ? listOf(before, part.id) : null;
  const at = list && before ? before[list].indexOf(part.id) : -1;
  const wasSelected = before?.selected === part.id;
  const { name, color, visible, transparent, locked } = part;
  // The store subscription takes the old id out of the lists; the new one goes back where it was.
  const [id] = useStore.getState().replaceParts([{ remove: [part.id], add: [{ name, mesh, color, visible, transparent, locked, source: part.source }] }], "Repair");
  const now = session();
  if (!now || !list || id === undefined) return;
  const ids = [...now[list]];
  ids.splice(Math.min(at, ids.length), 0, id);
  setSession({ ...now, [list]: ids, selected: wasSelected ? id : now.selected });
}
