// Part analyses: Home › New analysis and the Analyze tab start one on the selected part (one part at a time). Rust
// runs it (src/analysis/ipc.ts) with progress and Cancel in the busy box; the result is kept on the part as a node in
// the parts tree, saved with the project, and replaces an earlier result of the same kind. Picking the node shows the
// results in the right panel and the colours in the view, with the Part analysis tab in the ribbon.
import { create } from "zustand";
import { useBoolean } from "../boolean/store";
import { browserOnly, errorMessage, withExtension } from "../io/files";
import { copyText } from "../io/clipboard";
import { pickSaveFile } from "../io/ipc";
import { nextJob } from "../jobs";
import { useLabel } from "../label/store";
import { useLibrary } from "../library/store";
import { closeMeasure } from "../measure/session";
import { useMeasure } from "../measure/store";
import { isStale, meshOperationRunning, runMeshOperation, WAIT_MESSAGE, staleMessage } from "../ops/meshOps";
import { CANCELLED, cancelRepair } from "../repair/ipc";
import { CLOSE_REPAIR_FIRST } from "../repair/session";
import { useRepair } from "../repair/store";
import { exportText } from "../stats/ipc";
import { toCsv, toTsv } from "../stats/format";
import { useStore, type Part, type Vec3 } from "../store";
import type { TabId } from "../features";
import { toast, useUi } from "../ui";
import { viewport } from "../viewport/api";
import { runAnalysis } from "./ipc";
import { resultCells } from "./rows";
import { wallTriangleColors } from "./colors";
import { KIND_LABELS, type AnalysisKind, type AnalysisSettings, type CompareView, type PartAnalysis, type Probe } from "./types";

/** A run in progress: what the busy box shows, and what Cancel stops. */
export interface AnalysisBusy {
  job: number;
  label: string;
  fraction: number;
  cancelling: boolean;
}

export interface AnalysisState {
  busy: AnalysisBusy | null;
  /** The analysis on show (its part and id): the right panel shows its results and the view its colours. */
  shown: { partId: string; id: string } | null;
  /** How a comparison on show is drawn (the panel's controls). */
  compare: CompareView;
}

export const DEFAULT_COMPARE_VIEW: CompareView = {
  tolerance: 0.1,
  gradient: true,
  triangleEdges: false,
  toleranceEdges: false,
  transparent: false,
  bars: null,
  tab: "distances",
  probe: "select",
};

export const useAnalysis = create<AnalysisState>(() => ({ busy: null, shown: null, compare: DEFAULT_COMPARE_VIEW }));

const s = () => useStore.getState();

/** The kinds that ask for settings before they run. */
const HAS_SETTINGS: ReadonlySet<AnalysisKind> = new Set(["support", "skin", "wall", "compare"]);

export const ONE_PART = "Analyses run on one part at a time. Select just one part.";
export const PICK_PART = "Select the part to analyze first.";

let lastId = 0;
const nextAnalysisId = () => `analysis-${Date.now().toString(36)}-${++lastId}`;

/** True while a mode with a panel of its own is open; the analysis view waits behind it. */
function otherModeOpen(): boolean {
  return (
    useRepair.getState().session !== null ||
    useBoolean.getState().session !== null ||
    useLibrary.getState().session !== null ||
    useLabel.getState().session !== null ||
    useMeasure.getState().session !== null
  );
}

/** The part and analysis on show, or null when none is (or its part or the analysis is gone). */
export function shownAnalysis(shown = useAnalysis.getState().shown, parts = s().parts): { part: Part; analysis: PartAnalysis } | null {
  if (!shown) return null;
  const part = parts.find((p) => p.id === shown.partId);
  const analysis = part?.analyses?.find((a) => a.id === shown.id);
  return part && analysis ? { part, analysis } : null;
}

/** True while the analysis view shows: an analysis is picked and no other mode is open. */
export function isAnalysisOpen(): boolean {
  return shownAnalysis() !== null && !otherModeOpen();
}

const sameMatrix = (a: readonly number[], b: readonly number[]) => a.length === b.length && a.every((v, i) => v === b[i]);

/**
 * True when the part has moved, turned or been scaled since the analysis ran; for a comparison also when the other
 * part has, or is gone.
 */
export function isOutOfDate(part: Part, analysis: PartAnalysis, parts: readonly Part[] = s().parts): boolean {
  if (!sameMatrix(part.matrix, analysis.matrix)) return true;
  if (analysis.kind !== "compare") return false;
  const other = parts.find((p) => p.id === analysis.settings.comparisonId);
  return !other || !sameMatrix(other.matrix, analysis.settings.comparisonMatrix);
}

/** The one part an analysis starts on, or null after saying why there is none. */
function onePart(): Part | null {
  const selection = s().selection;
  if (selection.length > 1) {
    toast(ONE_PART);
    return null;
  }
  const part = s().parts.find((p) => p.id === selection[0]);
  if (!part) {
    toast(PICK_PART);
    return null;
  }
  return part;
}

/** Says why an analysis cannot start now, or returns false when it can. */
function refusedNow(what: string): boolean {
  if (browserOnly(what)) return true;
  if (useRepair.getState().session !== null) {
    toast(CLOSE_REPAIR_FIRST);
    return true;
  }
  if (meshOperationRunning()) {
    toast(WAIT_MESSAGE);
    return true;
  }
  return false;
}

/** Home › New analysis ▸ a kind, or the Analyze tab: asks for the settings when the kind has any, else runs. */
export function newAnalysis(kind: AnalysisKind): void {
  if (refusedNow(KIND_LABELS[kind])) return;
  if (kind === "compare") {
    // The reference is the part picked first; a second picked part is the comparison.
    const [first, second, ...more] = s().selection;
    if (!first || more.length > 0) {
      toast(first ? COMPARE_PARTS : COMPARE_PICK);
      return;
    }
    if (s().parts.length < 2) {
      toast("Mesh compare needs two parts.");
      return;
    }
    useUi.getState().openDialog({ kind: "analysis-settings", analysis: kind, partId: first, ...(second ? { otherId: second } : {}) });
    return;
  }
  const part = onePart();
  if (!part) return;
  if (HAS_SETTINGS.has(kind)) useUi.getState().openDialog({ kind: "analysis-settings", analysis: kind, partId: part.id });
  else void runPartAnalysis(kind, part.id, null);
}

export const COMPARE_PICK = "Select the reference part first, and the comparison part second if you like.";
export const COMPARE_PARTS = "Mesh compare works on two parts. Select just the reference and the comparison.";

/** Stops the run in progress: Rust stops at once and nothing changes. */
export function cancelAnalysis(): void {
  const busy = useAnalysis.getState().busy;
  if (!busy || busy.cancelling) return;
  useAnalysis.setState({ busy: { ...busy, cancelling: true } });
  cancelRepair(busy.job).catch(() => {});
}

/**
 * Runs the analysis `kind` on the part `partId` in Rust, with the busy box's progress and Cancel. On success the part
 * keeps it (in place of an earlier one of the same kind) as one undo step and the view shows it. Returns its id, or
 * null when it was cancelled, failed (said in a toast) or the part changed meanwhile.
 */
export async function runPartAnalysis(kind: AnalysisKind, partId: string, settings: AnalysisSettings): Promise<string | null> {
  const label = KIND_LABELS[kind];
  if (refusedNow(label)) return null;
  const start = s();
  const part = start.parts.find((p) => p.id === partId);
  if (!part) return null;
  // Mesh compare runs on the reference and the comparison part; the result stays with the reference.
  const comparisonId = kind === "compare" && settings && "comparisonId" in settings ? settings.comparisonId : null;
  const other = comparisonId === null ? null : start.parts.find((p) => p.id === comparisonId);
  if (comparisonId !== null && (!other || other.id === part.id)) {
    toast("Pick a comparison part other than the reference.");
    return null;
  }
  const inputs = other ? [part, other] : [part];
  const used: AnalysisSettings =
    other && settings && "comparisonId" in settings ? { ...settings, comparisonName: other.name, comparisonMatrix: [...other.matrix] } : settings;
  const generation = start.generation;
  const job = nextJob();
  let added: string | null = null;
  let cancelled = false;
  useAnalysis.setState({ busy: { job, label, fraction: 0, cancelling: false } });
  try {
    await runMeshOperation(label, `Could not finish ${label}`, async () => {
      try {
        const meshes = inputs.map((p) => ({ name: p.name, mesh: p.mesh, matrix: p.matrix }));
        const reply = await runAnalysis(kind, used, meshes, job, (fraction) => {
          const busy = useAnalysis.getState().busy;
          if (busy?.job === job) useAnalysis.setState({ busy: { ...busy, fraction } });
        });
        if (useAnalysis.getState().busy?.cancelling) {
          cancelled = true;
          return;
        }
        if (isStale(inputs, generation, s())) {
          toast(staleMessage(label), "warn");
          return;
        }
        const analysis = { id: nextAnalysisId(), kind, settings: used, result: reply.result, matrix: [...part.matrix], drawing: reply.drawing } as PartAnalysis;
        const list = [...(part.analyses ?? [])];
        const at = list.findIndex((a) => a.kind === kind);
        if (at >= 0) list[at] = analysis;
        else list.push(analysis);
        s().setAnalyses(new Map([[part.id, list]]), label);
        added = analysis.id;
      } catch (err) {
        if (err !== CANCELLED) throw err;
        cancelled = true;
      }
    });
  } finally {
    if (useAnalysis.getState().busy?.job === job) useAnalysis.setState({ busy: null });
  }
  if (cancelled) toast(`${label} was cancelled. Nothing changed.`);
  if (added) showAnalysis(partId, added);
  return added;
}

/** Runs the analysis again with its own settings, on the part as it is now. */
export function rerunAnalysis(partId: string, id: string): void {
  const found = shownAnalysis({ partId, id });
  if (found) void runPartAnalysis(found.analysis.kind, partId, found.analysis.settings);
}

/** Shows an analysis: its results in the right panel, its colours in the view, the Part analysis tab in the ribbon. */
export function showAnalysis(partId: string, id: string): void {
  if (useRepair.getState().session !== null) {
    toast(CLOSE_REPAIR_FIRST);
    return;
  }
  if (otherModeOpen()) closeMeasure();
  if (otherModeOpen()) {
    toast("Finish what is open in the right panel first.");
    return;
  }
  s().select([partId], "replace");
  const ui = useUi.getState();
  if (ui.ribbonTab !== "analysis") tabBefore = ui.ribbonTab;
  const same = useAnalysis.getState().shown?.id === id;
  useAnalysis.setState({ shown: { partId, id }, ...(same ? {} : { compare: { ...useAnalysis.getState().compare, bars: null, probe: "select" as const } }) });
  ui.setRibbonTab("analysis");
}

/** The tab that was on show before the analysis view opened, to go back to. */
let tabBefore: TabId | null = null;

/** Part analysis › OK: leaves the analysis view; the analysis stays in the tree. */
export function closeAnalysisView(): void {
  if (!useAnalysis.getState().shown) return;
  useAnalysis.setState({ shown: null });
  const ui = useUi.getState();
  if (ui.ribbonTab === "analysis") ui.setRibbonTab(tabBefore ?? "analyze");
  tabBefore = null;
}

/** The tree row's cross, and Part analysis › Close part analysis: takes the analysis off its part as one undo step. */
export function deleteAnalysis(partId: string, id: string): void {
  const part = s().parts.find((p) => p.id === partId);
  const analysis = part?.analyses?.find((a) => a.id === id);
  if (!part || !analysis) return;
  const shown = useAnalysis.getState().shown;
  if (shown?.partId === partId && shown.id === id) closeAnalysisView();
  s().setAnalyses(new Map([[partId, (part.analyses ?? []).filter((a) => a.id !== id)]]), `Delete ${KIND_LABELS[analysis.kind]}`);
  // Wall thickness with "make a coloured part when closing": a copy of the part in the result's colours.
  if (analysis.kind === "wall" && analysis.settings.colouredPart && analysis.drawing.values) {
    const colors = wallTriangleColors(analysis.drawing.values, analysis.settings);
    const mesh = { positions: part.mesh.positions.slice(), indices: part.mesh.indices.slice() };
    s().addParts([{ name: `${part.name} (wall thickness)`, mesh, matrix: [...part.matrix], colors, source: part.source }], "Coloured part from wall thickness");
  }
}

/**
 * A click in the view while a comparison shows with Add or Remove measure point on: true when the click was taken.
 */
export function probeClick(clientX: number, clientY: number): boolean {
  const found = isAnalysisOpen() ? shownAnalysis() : null;
  const mode = useAnalysis.getState().compare.probe;
  if (!found || found.analysis.kind !== "compare" || mode === "select") return false;
  const hit = viewport.probeAt(clientX, clientY);
  if (mode === "add") probeAt(hit);
  else if (hit) probeAt(null, hit.point);
  return true;
}

/** Changes how the comparison on show is drawn. */
export function setCompareView(patch: Partial<CompareView>): void {
  useAnalysis.setState({ compare: { ...useAnalysis.getState().compare, ...patch } });
}

/**
 * A click on a compared surface while Add measure point or Remove measure point is on: adds a point there with its
 * distance, or removes the point nearest the click. One undo step each.
 */
export function probeAt(hit: { point: Vec3; value: number } | null, removeNear?: Vec3): void {
  const found = shownAnalysis();
  if (!found || found.analysis.kind !== "compare") return;
  const { part, analysis } = found;
  const probes = analysis.probes ?? [];
  let next: Probe[];
  if (removeNear) {
    if (probes.length === 0) return;
    const d = (p: Probe) => Math.hypot(p.point[0] - removeNear[0], p.point[1] - removeNear[1], p.point[2] - removeNear[2]);
    const nearest = probes.reduce((a, b) => (d(b) < d(a) ? b : a));
    next = probes.filter((p) => p !== nearest);
  } else {
    if (!hit) return;
    next = [...probes, { id: `probe-${Date.now().toString(36)}-${++lastId}`, point: hit.point, value: hit.value }];
  }
  const list = (part.analyses ?? []).map((a) => (a.id === analysis.id ? { ...analysis, probes: next } : a));
  s().setAnalyses(new Map([[part.id, list]]), removeNear ? "Remove measure point" : "Add measure point");
}

/** Part analysis › Close part analysis on the analysis on show. */
export function closeShownAnalysis(): void {
  const shown = useAnalysis.getState().shown;
  if (shown) deleteAnalysis(shown.partId, shown.id);
}

/** Copies the shown analysis's results as tab-separated text. */
export async function copyResults(part: Part, analysis: PartAnalysis): Promise<void> {
  try {
    await copyText(toTsv(resultCells(analysis, part.name)));
    toast(`Copied the ${KIND_LABELS[analysis.kind].toLowerCase()} results to the clipboard.`);
  } catch (err) {
    toast(errorMessage(err, "Could not copy to the clipboard"), "error");
  }
}

/** Saves the shown analysis's results to a CSV file the user picks. */
export async function exportResults(part: Part, analysis: PartAnalysis): Promise<void> {
  if (browserOnly("Export to CSV")) return;
  let path: string | null;
  try {
    path = await pickSaveFile({
      title: `Export ${KIND_LABELS[analysis.kind].toLowerCase()}`,
      defaultPath: `${part.name} ${KIND_LABELS[analysis.kind].toLowerCase()}.csv`,
      filters: [{ name: "CSV (*.csv)", extensions: ["csv"] }],
    });
  } catch (err) {
    toast(errorMessage(err, "Could not show the save dialog"), "error");
    return;
  }
  if (!path) return;
  const target = withExtension(path, "csv");
  try {
    await exportText(target, toCsv(resultCells(analysis, part.name)));
    toast(`Saved ${target.split(/[\\/]/).pop()}.`);
  } catch (err) {
    toast(errorMessage(err, "Could not write the CSV file"), "error");
  }
}

// The analysis on show goes when its part or the analysis does (undo, delete, a new mesh).
useStore.subscribe((state, prev) => {
  if (state.parts !== prev.parts && useAnalysis.getState().shown && !shownAnalysis(useAnalysis.getState().shown, state.parts)) {
    useAnalysis.setState({ shown: null });
  }
});
