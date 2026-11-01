// Modify › Manipulate mesh: a panel on the right with a tab per tool (Triangle reduction, Smoothing, Refine). It works
// on the selected part, or, opened from repair mode, on the working copy and its selected triangles. Calculate runs the
// tool in Rust (src-tauri/src/modify, with progress and Cancel in the busy box) and shows the result in place of the
// part; Show original, held, shows the part as it was; OK puts the result in as one undo step (in repair mode, one
// inner step). Rust refuses a result that would open, turn or cross a sound part, and nothing changes then. Invert and
// Invert negative shells run at once on the selected parts.
import { create } from "zustand";
import { closeAnalysisView } from "../analysis/session";
import { CLOSE_BOOLEAN_FIRST, isBooleanOpen } from "../boolean/session";
import { triangleCount } from "../info";
import { browserOnly } from "../io/files";
import { nextJob } from "../jobs";
import { CLOSE_LABEL_FIRST, isLabelOpen } from "../label/session";
import { boundsOf, IDENTITY } from "../mesh";
import { bakeMesh, weldMesh } from "../ops/bake";
import { meshOperationRunning, runMeshOperation, WAIT_MESSAGE } from "../ops/meshOps";
import { CANCELLED, cancelRepair } from "../repair/ipc";
import { maskToIds } from "../repair/selection";
import { applyToolResult, enterRepair } from "../repair/session";
import { useRepair } from "../repair/store";
import { loadSetting, saveSetting, useStore, type MeshData, type Part, type Replacement } from "../store";
import { toast } from "../ui";
import { runModify, type ModifyReply } from "./ipc";
import { DEFAULT_SETTINGS, NEEDS_SELECTION, TAB_LABELS, TOOL_LABELS, type ModifyTab, type TabSettings, type ToolSettings } from "./types";

/** What the panel works on: a part, or the working copy of the part in repair mode. */
export interface ModifyTarget {
  kind: "part" | "repair";
  partId: string;
}

/** A calculated result, shown in place of the part until OK, Close or another Calculate. */
export interface ModifyPreview {
  /** The settings it was calculated with, as sent; a change makes it out of date. */
  key: string;
  label: string;
  mesh: MeshData;
  origin: Uint32Array | null;
  message: string;
  warnings: string[];
  before: number;
  after: number;
}

export interface ModifySession {
  target: ModifyTarget;
  tab: ModifyTab;
  /** The world-space mesh the tools run on: the part baked (as repair bakes it), or the working copy. */
  base: MeshData;
  preview: ModifyPreview | null;
  /** Show original is held down. */
  showOriginal: boolean;
}

export interface ModifyBusy {
  job: number;
  label: string;
  fraction: number;
  stage: string | null;
  cancelling: boolean;
}

export interface ModifyState {
  session: ModifySession | null;
  settings: TabSettings;
  /** Draw the triangles' edges on the part and the result (Display and settings › Highlight triangles). */
  highlight: boolean;
  busy: ModifyBusy | null;
}

const SETTINGS_KEY = "tricky3d.modifySettings";

/** The settings used last, over the defaults: only known keys of the right type. */
export function loadSettings(): TabSettings {
  const out: TabSettings = structuredClone(DEFAULT_SETTINGS);
  try {
    const raw = JSON.parse(loadSetting(SETTINGS_KEY) ?? "null") as Record<string, Record<string, unknown>> | null;
    if (!raw || typeof raw !== "object") return out;
    for (const tab of Object.keys(out) as ModifyTab[]) {
      const saved = raw[tab];
      if (!saved || typeof saved !== "object") continue;
      const into = out[tab] as unknown as Record<string, unknown>;
      for (const key of Object.keys(into)) {
        const v = saved[key];
        if (typeof v === typeof into[key] && (typeof v !== "number" || Number.isFinite(v))) into[key] = v;
      }
    }
  } catch {
    // Damaged settings: the defaults.
  }
  return out;
}

export const useModify = create<ModifyState>(() => ({ session: null, settings: loadSettings(), highlight: false, busy: null }));

const m = () => useModify.getState();
const s = () => useStore.getState();

export const PICK_ONE_PART = "Select the part to change first.";
export const ONE_PART_ONLY = "The mesh tools work on one part at a time. Select just one part.";
export const LOCKED_PART = "The part is locked. Unlock it first.";

/** The part the panel would open on (the selected one), or null after saying why there is none. */
function onePart(): Part | null {
  const ids = s().selection;
  if (ids.length > 1) {
    toast(ONE_PART_ONLY);
    return null;
  }
  const part = s().parts.find((p) => p.id === ids[0]);
  if (!part) {
    toast(PICK_ONE_PART);
    return null;
  }
  if (part.locked) {
    toast(LOCKED_PART);
    return null;
  }
  return part;
}

/** Says why a mesh tool cannot start now, or returns false when it can. */
function refusedNow(what: string): boolean {
  if (browserOnly(what)) return true;
  if (isBooleanOpen()) {
    toast(CLOSE_BOOLEAN_FIRST);
    return true;
  }
  if (isLabelOpen()) {
    toast(CLOSE_LABEL_FIRST);
    return true;
  }
  if (meshOperationRunning() || m().busy || useRepair.getState().busy) {
    toast(WAIT_MESSAGE);
    return true;
  }
  return false;
}

/** A round number of millimetres near `x` (1, 2 or 5 times a power of ten). */
export function niceMm(x: number): number {
  if (!(x > 0) || !Number.isFinite(x)) return 1;
  const p = 10 ** Math.floor(Math.log10(x));
  const f = x / p;
  return (f < 1.5 ? 1 : f < 3.5 ? 2 : f < 7.5 ? 5 : 10) * p;
}

/** The settings that depend on the part, for a panel opened on `base`: half its triangles, edges up to its size. */
export function partSettings(settings: TabSettings, base: MeshData): TabSettings {
  const box = boundsOf(base, IDENTITY);
  const diagonal = box ? Math.hypot(box.max[0] - box.min[0], box.max[1] - box.min[1], box.max[2] - box.min[2]) : 10;
  return {
    ...settings,
    reduce: { ...settings.reduce, target: Math.max(4, Math.floor(triangleCount(base) / 2)), maxEdge: Math.ceil(diagonal), selectedOnly: false },
    smooth: { ...settings.smooth, selectedOnly: false },
    refine: { ...settings.refine, maxEdge: niceMm(diagonal / 50), selectedOnly: false },
    remesh: { ...settings.remesh, edge: niceMm(diagonal / 50), largeEdge: niceMm(diagonal / 10), selectedOnly: false },
    holes: { ...settings.holes, selectedOnly: false },
  };
}

/** The working copy of a part: its matrix baked in, coincident points joined (as repair mode makes it). */
export function bakedCopy(part: Part): MeshData {
  const baked = bakeMesh(part.mesh, part.matrix);
  return baked === part.mesh ? baked : weldMesh(baked);
}

/**
 * Modify › Manipulate mesh ▸ a tool (and the repair menus' entries): opens the panel on that tool's tab, on the
 * working copy in repair mode, else on the selected part. With the panel open it only switches tabs.
 */
export function openManipulate(tab: ModifyTab): void {
  const open = m().session;
  if (open) {
    useModify.setState({ session: { ...open, tab } });
    return;
  }
  if (refusedNow(TAB_LABELS[tab])) return;
  let repair = useRepair.getState().session;
  // Extrude and Offset move selected triangles, which are picked in repair mode: open it on the part first.
  if (!repair && NEEDS_SELECTION.has(tab)) {
    const part = onePart();
    if (!part) return;
    enterRepair(part.id);
    repair = useRepair.getState().session;
    if (!repair) return;
  }
  let target: ModifyTarget;
  let base: MeshData;
  if (repair) {
    target = { kind: "repair", partId: repair.partId };
    base = repair.mesh;
  } else {
    const part = onePart();
    if (!part) return;
    target = { kind: "part", partId: part.id };
    base = bakedCopy(part);
  }
  closeAnalysisView();
  useModify.setState({
    session: { target, tab, base, preview: null, showOriginal: false },
    settings: partSettings(m().settings, base),
  });
}

/** Leaves the panel; a result not applied with OK is dropped, and a run going is cancelled. */
export function closeManipulate(): void {
  const busy = m().busy;
  if (busy) cancelRepair(busy.job).catch(() => {});
  useModify.setState({ session: null, busy: null });
}

export function setTab(tab: ModifyTab): void {
  const open = m().session;
  if (open) useModify.setState({ session: { ...open, tab } });
}

/** Changes a tool's settings; they are remembered for the next time. */
export function editSettings<T extends ModifyTab>(tab: T, patch: Partial<TabSettings[T]>): void {
  const settings = { ...m().settings, [tab]: { ...m().settings[tab], ...patch } } as TabSettings;
  saveSetting(SETTINGS_KEY, JSON.stringify(settings));
  useModify.setState({ settings });
}

export function setHighlight(highlight: boolean): void {
  useModify.setState({ highlight });
}

/** Show original: true while the button is held. */
export function setShowOriginal(showOriginal: boolean): void {
  const open = m().session;
  if (open && open.showOriginal !== showOriginal) useModify.setState({ session: { ...open, showOriginal } });
}

/** The triangles selected in repair mode (sorted ids); none outside it. */
export function selectedTriangles(state: ModifyState = m()): Uint32Array {
  const repair = useRepair.getState().session;
  if (state.session?.target.kind !== "repair" || !repair || repair.mesh !== state.session.base) return new Uint32Array(0);
  return maskToIds(repair.selection);
}

/** What Rust runs for the tab with these settings; "Selected triangles only" counts only with a selection. */
export function toolOf(tab: ModifyTab, settings: TabSettings, selected: boolean): ToolSettings {
  switch (tab) {
    case "reduce":
      return { tool: "reduce", ...settings.reduce, selectedOnly: selected && settings.reduce.selectedOnly };
    case "smooth":
      return { tool: "smooth", ...settings.smooth, selectedOnly: selected && settings.smooth.selectedOnly };
    case "refine":
      return { tool: "refine", ...settings.refine, selectedOnly: selected && settings.refine.selectedOnly };
    case "remesh": {
      const { slimColored: _colored, slimAngle: _angle, ...rest } = settings.remesh;
      const whole = rest.method === "volumetric" || rest.method === "manifold";
      return { tool: "remesh", ...rest, selectedOnly: selected && !whole && rest.selectedOnly };
    }
    case "zcomp":
      return { tool: "z-compensation", ...settings.zcomp };
    case "extrude":
      return { tool: "extrude", ...settings.extrude };
    case "offset":
      return { tool: "offset-selection", ...settings.offset };
    case "holes":
      return { tool: "offset-holes", ...settings.holes, selectedOnly: selected && settings.holes.selectedOnly };
  }
}

/** The tool the open panel would run now, and its key. */
function current(): { tool: ToolSettings; key: string; selection: Uint32Array } | null {
  const open = m().session;
  if (!open) return null;
  const selection = selectedTriangles();
  const tool = toolOf(open.tab, m().settings, selection.length > 0);
  return { tool, key: JSON.stringify(tool), selection };
}

/** True when the result shown was calculated with the settings the panel has now. */
export function previewIsCurrent(state: ModifyState = m()): boolean {
  const open = state.session;
  if (!open?.preview) return false;
  const tool = toolOf(open.tab, state.settings, selectedTriangles(state).length > 0);
  return open.preview.key === JSON.stringify(tool);
}

/**
 * Runs `tool` on `mesh` in Rust as the one mesh operation, with the busy box. Resolves to Rust's answer, or null when
 * it was cancelled (said in a toast) or failed (Rust's sentence in a toast).
 */
async function runTool(label: string, mesh: MeshData, tool: ToolSettings, selection: Uint32Array): Promise<ModifyReply | null> {
  const job = nextJob();
  let reply: ModifyReply | null = null;
  let cancelled = false;
  useModify.setState({ busy: { job, label, fraction: 0, stage: null, cancelling: false } });
  try {
    await runMeshOperation(label, `Could not finish ${label}`, async () => {
      try {
        const answer = await runModify(mesh, tool, selection, job, (p) => {
          const busy = m().busy;
          if (busy?.job === job) useModify.setState({ busy: { ...busy, fraction: p.fraction, stage: p.stage } });
        });
        if (m().busy?.cancelling) cancelled = true;
        else reply = answer;
      } catch (err) {
        if (err !== CANCELLED) throw err;
        cancelled = true;
      }
    });
  } finally {
    if (m().busy?.job === job) useModify.setState({ busy: null });
  }
  if (cancelled) toast(`${label} was cancelled. Nothing changed.`);
  return reply;
}

/** Calculate: runs the open tab's tool and shows the result in place of the part. Resolves to true when it did. */
export async function calculate(): Promise<boolean> {
  const open = m().session;
  const now = current();
  if (!open || !now) return false;
  if (refusedNow(TOOL_LABELS[now.tool.tool])) return false;
  const label = TOOL_LABELS[now.tool.tool];
  const reply = await runTool(label, open.base, now.tool, now.selection);
  const after = m().session;
  if (!reply || !after || after.base !== open.base) return false;
  if (!reply.changed) {
    useModify.setState({ session: { ...after, preview: null } });
    toast(reply.message);
    return false;
  }
  const preview: ModifyPreview = {
    key: now.key,
    label,
    mesh: reply.mesh,
    origin: reply.origin,
    message: reply.message,
    warnings: reply.warnings,
    before: reply.before,
    after: reply.after,
  };
  useModify.setState({ session: { ...after, preview, showOriginal: false } });
  for (const w of reply.warnings) toast(w, "warn");
  return true;
}

/** OK: calculates first when the result shown is missing or out of date, then puts it in as one undo step and closes. */
export async function applyManipulate(): Promise<boolean> {
  if (!m().session) return false;
  if (!previewIsCurrent() && !(await calculate())) return false;
  const open = m().session;
  const preview = open?.preview;
  if (!open || !preview) return false;
  if (open.target.kind === "repair") {
    if (!applyToolResult(preview.label, open.base, preview.mesh, preview.origin)) {
      toast("The part changed while the tool ran, so the result was dropped.", "warn");
      return false;
    }
  } else {
    const part = s().parts.find((p) => p.id === open.target.partId);
    if (!part || bakedCopyIs(part, open.base) === false) {
      toast("The part changed while the tool ran, so the result was dropped.", "warn");
      return false;
    }
    s().replaceParts([replacement(part, preview.mesh)], preview.label);
  }
  toast(preview.message);
  useModify.setState({ session: null });
  return true;
}

/** Whether `base` is still the baked copy of `part`: the part kept its mesh and matrix since the panel opened. */
const bases = new WeakMap<MeshData, { mesh: MeshData; matrix: string }>();
function bakedCopyIs(part: Part, base: MeshData): boolean | null {
  const from = bases.get(base);
  if (!from) return null;
  return from.mesh === part.mesh && from.matrix === JSON.stringify(part.matrix);
}

// Remember which part state each baked copy came from, so OK can tell a part that changed meanwhile.
useModify.subscribe((state, prev) => {
  const open = state.session;
  if (!open || open === prev.session || open.target.kind !== "part" || bases.has(open.base)) return;
  const part = s().parts.find((p) => p.id === open.target.partId);
  if (part) bases.set(open.base, { mesh: part.mesh, matrix: JSON.stringify(part.matrix) });
});

// The panel closes when New project or Open project replaces the document, and, opened in repair mode, when repair mode
// closes: what it works on is gone.
useStore.subscribe((state, prev) => {
  if (state.generation !== prev.generation && m().session) closeManipulate();
});
useRepair.subscribe((r, prev) => {
  if (prev.session && !r.session && m().session?.target.kind === "repair") closeManipulate();
});

/** The part with a new world-space mesh in its place: same name, colour and settings, the identity matrix. */
function replacement(part: Part, mesh: MeshData): Replacement {
  return {
    remove: [part.id],
    add: [
      {
        name: part.name,
        mesh,
        matrix: [...IDENTITY],
        color: part.color,
        visible: part.visible,
        transparent: part.transparent,
        locked: part.locked,
        packing: part.packing,
        source: part.source,
      },
    ],
  };
}

/** Stops the run in progress: Rust stops at once and nothing changes. */
export function cancelModify(): void {
  const busy = m().busy;
  if (!busy || busy.cancelling) return;
  useModify.setState({ busy: { ...busy, cancelling: true } });
  cancelRepair(busy.job).catch(() => {});
}

/** The result drawn in place of the part, while one is shown and Show original is not held. */
export const PREVIEW_ID = "modify-preview";

/** Show slim faces' angle while the Remesh tab shows it, else null. */
export function slimAngle(state: ModifyState = m()): number | null {
  const r = state.settings.remesh;
  return state.session?.tab === "remesh" && r.slimColored ? r.slimAngle : null;
}

/**
 * What stands in for the part in the view: the result while one is shown (and Show original is not held), or the part
 * as it is while Show slim faces colours it; null otherwise.
 */
export function previewPart(state: ModifyState = m(), parts: readonly Part[] = s().parts): Part | null {
  const open = state.session;
  if (!open) return null;
  const mesh = open.preview && !open.showOriginal ? open.preview.mesh : slimAngle(state) !== null ? open.base : null;
  if (!mesh) return null;
  const part = parts.find((p) => p.id === open.target.partId);
  return {
    id: PREVIEW_ID,
    name: part?.name ?? "Result",
    mesh,
    matrix: [...IDENTITY],
    visible: true,
    transparent: false,
    locked: true,
    color: part?.color ?? "#a9b1bc",
  };
}

// ---------------------------------------------------------------------------------------------
// Invert: at once, on the selected parts or the working copy.

/**
 * Modify › Invert ▸ Turn the part inside out, or Turn inside-out shells back: on every selected (unlocked) part as one
 * undo step, or in repair mode on the working copy as one inner step.
 */
export async function invertParts(kind: "invert" | "invert-negative"): Promise<void> {
  const label = TOOL_LABELS[kind];
  if (m().session) {
    toast("Close Manipulate mesh first.");
    return;
  }
  if (refusedNow(label)) return;
  const repair = useRepair.getState().session;
  if (repair) {
    const from = repair.mesh;
    const reply = await runTool(label, from, { tool: kind }, new Uint32Array(0));
    if (!reply) return;
    if (!reply.changed) toast(reply.message);
    else if (applyToolResult(label, from, reply.mesh, reply.origin)) toast(reply.message);
    return;
  }
  const chosen = new Set(s().selection);
  const parts = s().parts.filter((p) => chosen.has(p.id) && !p.locked);
  if (parts.length === 0) {
    toast(s().parts.some((p) => chosen.has(p.id)) ? "Locked parts cannot be changed. Unlock them first." : "Select the parts to invert first.");
    return;
  }
  const before = s().parts;
  const replacements: Replacement[] = [];
  let last = "";
  for (const part of parts) {
    const reply = await runTool(label, bakedCopy(part), { tool: kind }, new Uint32Array(0));
    if (!reply) return;
    last = reply.message;
    if (reply.changed) replacements.push(replacement(part, reply.mesh));
  }
  if (s().parts !== before) {
    toast("The parts changed while the tool ran, so nothing was changed.", "warn");
    return;
  }
  if (replacements.length === 0) {
    toast(last || "Nothing to invert.");
    return;
  }
  s().replaceParts(replacements, label);
  toast(replacements.length === 1 ? last : `${label}: ${replacements.length} parts changed.`);
}
