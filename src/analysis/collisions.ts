// Collision detection (Analyze › Collision detection, Arrange › Collision check, the quick strip) and the interlocking
// test. Collision detection is a panel that stays open: Rust checks the parts' meshes as Packing does (src-tauri
// src/arrange/collide.rs) and also finds parts closer than the minimum distance; while the panel is open each part
// shows a red cross or a green tick in the tree, the view draws where parts cross and a status, and with Auto-update
// the check runs again after every move, turn or scale (also during a drag with Update while dragging). A check only
// reads the parts, so it does not take the one-mesh-operation slot; a newer check cancels an older one.
import { create } from "zustand";
import { browserOnly } from "../io/files";
import { nextJob } from "../jobs";
import { meshOperationRunning, runMeshOperation, WAIT_MESSAGE } from "../ops/meshOps";
import { CANCELLED, cancelRepair } from "../repair/ipc";
import { CLOSE_REPAIR_FIRST } from "../repair/session";
import { useRepair } from "../repair/store";
import { loadSetting, saveSetting, useStore, type Part } from "../store";
import { toast, useUi } from "../ui";
import { runAnalysis } from "./ipc";
import { closeAnalysisView, useAnalysis } from "./session";
import type { AnalysisDrawing, InterlockResult } from "./types";

export interface CollisionSettings {
  autoUpdate: boolean;
  whileDragging: boolean;
  /** Draw where parts cross, and the closest points of parts too close. */
  display: boolean;
  /** Check hidden parts too. */
  checkHidden: boolean;
  /** mm: parts nearer than this count as too close; 0 checks only for collisions. */
  minDistance: number;
  /** mm: how far Move by moves the selected parts per click, 0 to 1000. */
  moveStep: number;
}

/** One pair found: by part id. */
export interface CollisionPair {
  a: string;
  b: string;
  kind: "collide" | "near";
  distance: number;
}

export interface CollisionState {
  open: boolean;
  settings: CollisionSettings;
  tab: "settings" | "tolerance";
  /** The last finished check, and the parts it checked. */
  result: { pairs: CollisionPair[]; checked: string[]; drawing: AnalysisDrawing } | null;
  /** The check running now. */
  running: number | null;
  /** The parts changed since the result: it waits for the next check. */
  outdated: boolean;
}

const SETTINGS_KEY = "tricky3d.collisionSettings";

export const DEFAULT_COLLISIONS: CollisionSettings = { autoUpdate: true, whileDragging: false, display: true, checkHidden: true, minDistance: 0, moveStep: 1 };

/** Largest Move by step, mm. */
export const MAX_MOVE_STEP = 1000;

function loadSettings(): CollisionSettings {
  try {
    const raw = JSON.parse(loadSetting(SETTINGS_KEY) ?? "null") as Partial<CollisionSettings> | null;
    if (!raw || typeof raw !== "object") return DEFAULT_COLLISIONS;
    const out = { ...DEFAULT_COLLISIONS };
    for (const key of Object.keys(out) as (keyof CollisionSettings)[]) {
      if (typeof raw[key] === typeof out[key]) (out as Record<string, unknown>)[key] = raw[key];
    }
    out.minDistance = Math.min(100, Math.max(0, Number.isFinite(out.minDistance) ? out.minDistance : 0));
    out.moveStep = Math.min(MAX_MOVE_STEP, Math.max(0, Number.isFinite(out.moveStep) ? out.moveStep : DEFAULT_COLLISIONS.moveStep));
    return out;
  } catch {
    return DEFAULT_COLLISIONS;
  }
}

export const useCollisions = create<CollisionState>(() => ({
  open: false,
  settings: loadSettings(),
  tab: "settings",
  result: null,
  running: null,
  outdated: false,
}));

const s = () => useStore.getState();

/** The parts a check looks at: every part, or every shown one. */
function checkedParts(settings: CollisionSettings): Part[] {
  return s().parts.filter((p) => settings.checkHidden || p.visible);
}

/** Opens the panel (leaving the analysis view) and checks at once. */
export function openCollisions(): void {
  if (browserOnly("Collision detection")) return;
  if (useRepair.getState().session !== null) {
    toast(CLOSE_REPAIR_FIRST);
    return;
  }
  closeAnalysisView();
  useCollisions.setState({ open: true });
  void updateCollisions();
}

export function closeCollisions(): void {
  const running = useCollisions.getState().running;
  if (running !== null) cancelRepair(running).catch(() => {});
  clearTimeout(timer);
  useCollisions.setState({ open: false, result: null, running: null, outdated: false });
}

export function setCollisionSettings(patch: Partial<CollisionSettings>): void {
  const settings = { ...useCollisions.getState().settings, ...patch };
  saveSetting(SETTINGS_KEY, JSON.stringify(settings));
  useCollisions.setState({ settings });
  // A new minimum distance or another set of parts needs a new check.
  if (patch.minDistance !== undefined || patch.checkHidden !== undefined) void updateCollisions();
}

/**
 * Move by: moves the selected parts (locked ones stay) by the step along one axis, as one undo step. Auto-update then
 * checks again once the move settles. Returns how many parts moved.
 */
export function nudgeSelected(axis: 0 | 1 | 2, sign: 1 | -1): number {
  const step = useCollisions.getState().settings.moveStep;
  const chosen = new Set(s().selection);
  const parts = s().parts.filter((p) => chosen.has(p.id) && !p.locked);
  if (parts.length === 0) {
    toast(s().parts.some((p) => chosen.has(p.id)) ? "Locked parts do not move." : "Select the parts to move first.");
    return 0;
  }
  if (!(step > 0)) {
    toast("Enter a step greater than 0 mm.");
    return 0;
  }
  const matrices = new Map<string, number[]>();
  for (const p of parts) {
    const m = [...p.matrix];
    m[12 + axis] += sign * step;
    matrices.set(p.id, m);
  }
  s().setPartMatrices(matrices, `Move by ${sign > 0 ? "+" : "−"}${"XYZ"[axis]}`);
  return parts.length;
}

/** Runs the check now (Update), replacing a check still running. */
export async function updateCollisions(): Promise<void> {
  const state = useCollisions.getState();
  if (!state.open) return;
  if (state.running !== null) cancelRepair(state.running).catch(() => {});
  const parts = checkedParts(state.settings);
  const before = s().parts;
  if (parts.length < 2) {
    useCollisions.setState({ result: { pairs: [], checked: parts.map((p) => p.id), drawing: {} }, running: null, outdated: false });
    return;
  }
  const job = nextJob();
  useCollisions.setState({ running: job });
  try {
    const reply = await runAnalysis(
      "collisions",
      { minDistance: state.settings.minDistance },
      parts.map((p) => ({ name: p.name, mesh: p.mesh, matrix: p.matrix })),
      job,
    );
    if (useCollisions.getState().running !== job) return;
    const raw = (reply.result as { pairs?: { a: number; b: number; kind: string; distance: number }[] }).pairs ?? [];
    const pairs = raw.map((p) => ({ a: parts[p.a].id, b: parts[p.b].id, kind: p.kind === "near" ? ("near" as const) : ("collide" as const), distance: p.distance }));
    useCollisions.setState({ result: { pairs, checked: parts.map((p) => p.id), drawing: reply.drawing }, running: null, outdated: s().parts !== before });
  } catch (err) {
    if (useCollisions.getState().running !== job) return;
    useCollisions.setState({ running: null });
    if (err !== CANCELLED) toast(typeof err === "string" ? err : "Could not check for collisions.", "error");
  }
}

/** A part's state in the last check: in a collision, too close, clear, or null when it was not checked. */
export function partCollision(id: string, state: CollisionState = useCollisions.getState()): "collide" | "near" | "clear" | null {
  const result = state.result;
  if (!state.open || !result || !result.checked.includes(id)) return null;
  let worst: "collide" | "near" | "clear" = "clear";
  for (const p of result.pairs) {
    if (p.a !== id && p.b !== id) continue;
    if (p.kind === "collide") return "collide";
    worst = "near";
  }
  return worst;
}

// Auto-update: after a change settles, or every DRAG_INTERVAL_MS during a drag with Update while dragging.
const SETTLE_MS = 350;
const DRAG_INTERVAL_MS = 250;
let timer: ReturnType<typeof setTimeout> | undefined;
let lastRun = 0;

useStore.subscribe((state, prev) => {
  const c = useCollisions.getState();
  if (!c.open || state.parts === prev.parts) return;
  if (!c.outdated) useCollisions.setState({ outdated: true });
  if (!c.settings.autoUpdate) return;
  clearTimeout(timer);
  const now = Date.now();
  if (c.settings.whileDragging && now - lastRun >= DRAG_INTERVAL_MS) {
    lastRun = now;
    void updateCollisions();
    return;
  }
  timer = setTimeout(() => {
    lastRun = Date.now();
    void updateCollisions();
  }, c.settings.whileDragging ? DRAG_INTERVAL_MS : SETTLE_MS);
});

// The analysis view and the panel share the right panel: showing an analysis closes the panel.
useAnalysis.subscribe((a, prev) => {
  if (a.shown && a.shown !== prev.shown && useCollisions.getState().open) closeCollisions();
});

// ---------------------------------------------------------------------------------------------
// Interlocking test

/**
 * Analyze › Interlocking test: whether each of the selected parts (or every shown part when none is selected) can be
 * lifted out upwards, one after another, without hitting another. The answer opens in a dialog.
 */
export async function interlockingTest(): Promise<void> {
  if (browserOnly("Interlocking test")) return;
  if (useRepair.getState().session !== null) {
    toast(CLOSE_REPAIR_FIRST);
    return;
  }
  if (meshOperationRunning()) {
    toast(WAIT_MESSAGE);
    return;
  }
  const selected = new Set(s().selection);
  const parts = selected.size > 0 ? s().parts.filter((p) => selected.has(p.id)) : s().parts.filter((p) => p.visible);
  if (parts.length < 2) {
    toast("The interlocking test needs two parts or more. Select them, or select none to test every shown part.");
    return;
  }
  const job = nextJob();
  const label = "Interlocking test";
  useAnalysis.setState({ busy: { job, label, fraction: 0, cancelling: false } });
  let answer: InterlockResult | null = null;
  try {
    await runMeshOperation(label, `Could not finish the ${label.toLowerCase()}`, async () => {
      try {
        const reply = await runAnalysis(
          "interlock",
          null,
          parts.map((p) => ({ name: p.name, mesh: p.mesh, matrix: p.matrix })),
          job,
          (fraction) => {
            const busy = useAnalysis.getState().busy;
            if (busy?.job === job) useAnalysis.setState({ busy: { ...busy, fraction } });
          },
        );
        if (!useAnalysis.getState().busy?.cancelling) answer = reply.result as InterlockResult;
      } catch (err) {
        if (err !== CANCELLED) throw err;
      }
    });
  } finally {
    if (useAnalysis.getState().busy?.job === job) useAnalysis.setState({ busy: null });
  }
  const result = answer as InterlockResult | null;
  if (!result) return;
  useUi.getState().openDialog({
    kind: "interlock-result",
    rounds: result.rounds.map((round) => round.map((i) => parts[i].id)),
    trapped: result.trapped.map((i) => parts[i].id),
  });
}
