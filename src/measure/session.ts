// Measure mode: entering and leaving, the tool, the clicks that make a measurement, and keeping every measurement's
// result up to date. UI code changes measure state only through these functions; the state is in
// src/measure/store.ts, and the measurements themselves live with their parts in the main store, so adding or deleting
// one is an undo step and Save keeps them. Rust does every bit of geometry (src/measure/ipc.ts).
//
// The flow follows the reference tool: Home or Analyze › Measure opens the "Measurement" tab and the panel; a tool from
// the tab's dropdowns says what to click; each click becomes a feature in Rust (a snapped point, an edge, a circle, a
// surface, a wall); with all of them in, the measurement is added to the part clicked first and its value shown in the
// view and in the list. The measurements stay with the part (they move with it) and show again whenever the mode is
// open.
import { CLOSE_BOOLEAN_FIRST, isBooleanOpen } from "../boolean/session";
import { activePlanes, anyEnabled, sceneBounds, useClip } from "../clip";
import type { TabId } from "../features";
import { browserOnly, errorMessage } from "../io/files";
import { CLOSE_LIBRARY_FIRST, isLibraryOpen } from "../library/session";
import { meshOperationRunning, WAIT_MESSAGE } from "../ops/meshOps";
import { CLOSE_REPAIR_FIRST, isRepairing } from "../repair/session";
import { useStore, type Part, type Vec3 } from "../store";
import { toast, useUi } from "../ui";
import { forgetMeasureMeshes, measureEvaluate, measureFit, measurePick } from "./ipc";
import { emptySession, useMeasure, type MeasurePanelTab, type MeasureSession, type PendingFeature, type ResultEntry } from "./store";
import { DEFAULT_TOOL, clicksFor, toolById, type MeasureTool } from "./tools";
import type { Measurement } from "./types";

/** Main-document commands that would move or change the parts refuse with this while the mode is open. */
export const CLOSE_MEASURE_FIRST = "Close Measure first.";

const state = () => useMeasure.getState();
const session = () => useMeasure.getState().session;
const store = () => useStore.getState();

let tabBefore: TabId | null = null;
let measureSeq = 0;

function setSession(patch: Partial<MeasureSession>) {
  const current = session();
  if (current) useMeasure.setState({ session: { ...current, ...patch } });
}

/** True while Measure mode is open. */
export function isMeasureOpen(): boolean {
  return session() !== null;
}

/** The tool picked now. */
export function currentTool(): MeasureTool {
  return toolById(session()?.tool ?? DEFAULT_TOOL) ?? (toolById(DEFAULT_TOOL) as MeasureTool);
}

// ---------------------------------------------------------------------------------------------
// Entering and leaving

let watching = false;

/** Results follow the parts (a move, an undo), and New or Open project ends the mode (subscribes once). */
function watchParts() {
  if (watching) return;
  watching = true;
  useStore.subscribe((s, prev) => {
    if (!isMeasureOpen()) return;
    if (s.generation !== prev.generation) {
      closeMeasure();
      return;
    }
    if (s.parts === prev.parts) return;
    const current = session();
    // The part a measurement in progress belongs to went away (undo, delete): start it again.
    if (current?.owner && !s.parts.some((p) => p.id === current.owner)) setSession({ picks: [], points: [], owner: null });
    void refreshResults();
  });
}

/**
 * Home › Measure and Analyze › Measure: opens the Measurement tab and panel with `tool` picked (Distance point to point
 * the first time). Refused while repair mode, Boolean operations, the Part library or a mesh operation is busy with the
 * parts; a second call picks the tool.
 */
export function openMeasure(tool: string = state().session?.tool ?? DEFAULT_TOOL): void {
  const ui = useUi.getState();
  if (isMeasureOpen()) {
    pickMeasureTool(tool);
    ui.setRibbonTab("measure");
    return;
  }
  if (isRepairing()) return toast(CLOSE_REPAIR_FIRST);
  if (isBooleanOpen()) return toast(CLOSE_BOOLEAN_FIRST);
  if (isLibraryOpen()) return toast(CLOSE_LIBRARY_FIRST);
  if (meshOperationRunning()) return toast(WAIT_MESSAGE);
  if (browserOnly("Measuring")) return;
  useMeasure.setState({ session: emptySession(toolById(tool) ? tool : DEFAULT_TOOL), busy: false });
  watchParts();
  tabBefore = ui.ribbonTab === "measure" ? null : ui.ribbonTab;
  ui.closeContextMenu();
  ui.setRibbonTab("measure");
  const s = store();
  if (s.gizmoMode) s.setGizmoMode(null);
  void refreshResults();
}

/** Cancel: leaves the mode. The measurements stay with their parts; a measurement half made is dropped. */
export function closeMeasure(): void {
  if (!isMeasureOpen()) return;
  useMeasure.setState({ session: null, busy: false, results: {} });
  forgetMeasureMeshes().catch(() => {});
  const ui = useUi.getState();
  if (ui.ribbonTab === "measure") ui.setRibbonTab(tabBefore ?? "home");
  tabBefore = null;
}

// ---------------------------------------------------------------------------------------------
// Tools and the panel

/** A tool from the ribbon or the Tools tab: it starts a new measurement (the clicks so far are dropped). */
export function pickMeasureTool(id: string): void {
  if (!toolById(id)) return;
  if (!isMeasureOpen()) {
    openMeasure(id);
    return;
  }
  setSession({ tool: id, picks: [], points: [], owner: null });
}

/** Esc: drops the clicks of the measurement in progress. True when there were any. */
export function dropPending(): boolean {
  const current = session();
  if (!current || (current.picks.length === 0 && current.points.length === 0)) return false;
  setSession({ picks: [], points: [], owner: null });
  return true;
}

export function setPanelTab(panelTab: MeasurePanelTab): void {
  setSession({ panelTab });
}

/** A row of the list or of the parts tree: that measurement is highlighted (null: none). */
export function selectMeasurement(id: string | null): void {
  setSession({ selected: id });
}

/** Every measurement of the project, part by part in list order. */
export function allMeasurements(parts: readonly Part[] = store().parts): { part: Part; measurement: Measurement }[] {
  return parts.flatMap((part) => (part.measurements ?? []).map((measurement) => ({ part, measurement })));
}

/** Deletes one measurement, as one undo step. */
export function deleteMeasurement(id: string): void {
  const owner = store().parts.find((p) => p.measurements?.some((m) => m.id === id));
  if (!owner) return;
  store().setMeasurements(new Map([[owner.id, (owner.measurements ?? []).filter((m) => m.id !== id)]]), "Delete measurement");
  if (session()?.selected === id) setSession({ selected: null });
}

/** Reset: deletes every measurement of the project as one undo step, and the one in progress. */
export function resetMeasurements(): void {
  const changes = new Map(store().parts.filter((p) => (p.measurements ?? []).length > 0).map((p) => [p.id, [] as Measurement[]]));
  if (changes.size > 0) store().setMeasurements(changes, "Delete measurements");
  setSession({ picks: [], points: [], owner: null, selected: null });
}

// ---------------------------------------------------------------------------------------------
// Clicks

/** A click on a part in the view (src/viewport/picking.ts), with the world point and the snap reach there. */
export interface MeasureHit {
  partId: string;
  triangle: number;
  point: Vec3;
  /** World mm: about eight pixels at the point. */
  tolerance: number;
}

/** The clip planes in force, world space, for snapping to where they cut a part. */
function clipPlanes() {
  const clip = useClip.getState();
  const s = store();
  return anyEnabled(clip) ? activePlanes(clip, sceneBounds(s.platform, s.parts)) : [];
}

/** True while `current` is still the session a click started in: same tool, same clicks so far. */
const unchanged = (current: MeasureSession) => {
  const now = session();
  return now !== null && now.tool === current.tool && now.picks === current.picks && now.points === current.points;
};

/**
 * A click in the view: Rust turns it into the feature the tool needs next. With every feature in, the measurement is
 * added to the part clicked first, as one undo step "Measure", and picked in the list.
 */
export async function measureClick(hit: MeasureHit): Promise<void> {
  const current = session();
  if (!current || state().busy) return;
  const tool = currentTool();
  const need = tool.needs[current.picks.length];
  const part = store().parts.find((p) => p.id === hit.partId);
  const owner = store().parts.find((p) => p.id === (current.owner ?? hit.partId));
  if (!need || !part || !owner) return;
  const fitted = need === "circle3" || need === "sphere4";
  useMeasure.setState({ busy: true });
  try {
    const reply = await measurePick({
      kind: fitted ? "point" : need,
      mesh: part.mesh,
      matrix: part.matrix,
      owner: owner.matrix,
      triangle: hit.triangle,
      hit: hit.point,
      snap: need === "wall" ? "free" : state().options.snap,
      tolerance: hit.tolerance,
      clip: clipPlanes(),
    });
    if (!unchanged(current)) return;
    const pending: PendingFeature = { feature: reply.feature, world: reply.world, outline: reply.outline };
    let picks = current.picks;
    let points = current.points;
    if (fitted) {
      points = [...points, pending];
      if (points.length === clicksFor(need)) {
        const [feature, world] = await Promise.all([measureFit(need, points.map((p) => p.feature)), measureFit(need, points.map((p) => p.world))]);
        if (!unchanged(current)) return;
        picks = [...picks, { feature, world, outline: [] }];
        points = [];
      }
    } else {
      picks = [...picks, pending];
    }
    if (picks.length < tool.needs.length) {
      setSession({ picks, points, owner: owner.id });
      return;
    }
    finish(tool, owner, picks);
  } catch (err) {
    if (unchanged(current)) toast(errorMessage(err, "Could not measure there"), "warn");
  } finally {
    useMeasure.setState({ busy: false });
  }
}

/** The measurement made of `picks`, added to `owner` as one undo step, and picked in the list. */
function finish(tool: MeasureTool, owner: Part, picks: PendingFeature[]) {
  const { axes, diameter } = state().options;
  const measurement: Measurement = { id: `m${Date.now().toString(36)}${++measureSeq}`, tool: tool.id, features: picks.map((p) => p.feature), axes: [...axes], diameter };
  const now = store().parts.find((p) => p.id === owner.id) ?? owner;
  store().setMeasurements(new Map([[owner.id, [...(now.measurements ?? []), measurement]]]), "Measure");
  setSession({ picks: [], points: [], owner: null, selected: measurement.id });
}

// ---------------------------------------------------------------------------------------------
// Results

/**
 * Works out, in Rust, the result of every measurement whose part moved since (or that has none yet), and forgets those
 * of measurements that are gone. Results land one by one; a part moved again meanwhile is worked out again.
 */
export async function refreshResults(): Promise<void> {
  if (!isMeasureOpen()) return;
  const list = allMeasurements();
  const live = new Set(list.map((e) => e.measurement.id));
  const results = state().results;
  if (Object.keys(results).some((id) => !live.has(id))) {
    useMeasure.setState({ results: Object.fromEntries(Object.entries(results).filter(([id]) => live.has(id))) });
  }
  await Promise.all(
    list.map(async ({ part, measurement }) => {
      if (state().results[measurement.id]?.matrix === part.matrix) return;
      const tool = toolById(measurement.tool);
      if (!tool) return;
      let entry: ResultEntry;
      try {
        entry = { matrix: part.matrix, result: await measureEvaluate(tool.op, measurement.features, part.matrix), error: null };
      } catch (err) {
        entry = { matrix: part.matrix, result: null, error: errorMessage(err, "Could not work it out") };
      }
      // The part may have moved again, or the mode closed, while Rust worked.
      const now = store().parts.find((p) => p.id === part.id);
      if (!isMeasureOpen() || now?.matrix !== part.matrix || !now.measurements?.some((m) => m.id === measurement.id)) return;
      useMeasure.setState((s) => ({ results: { ...s.results, [measurement.id]: entry } }));
    }),
  );
}
