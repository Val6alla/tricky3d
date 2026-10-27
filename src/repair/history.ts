// Repair mode's inner undo history. The session (src/repair/session.ts) keeps two lists of working-copy states:
// `past`, the state before each step (oldest first), and `future`, the state after each undone step (next redo
// first). Each state carries the label of the step it belongs to. The caps are the main history's (src/history.ts):
// at most 50 steps, and at most 2 GB of mesh data that only the history keeps alive. Pure: meshes and selection
// masks are never mutated, so a state is a few references.
import { HISTORY_LIMITS, meshBytes, type HistoryLimits } from "../history";
import type { MeshData } from "../store";
import type { RepairSnapshot } from "./types";

export const REPAIR_HISTORY_LIMITS: HistoryLimits = HISTORY_LIMITS;

/** The working copy and its selection right now: what an undo or redo puts on the other list. */
export interface WorkingState {
  mesh: MeshData;
  selection: Uint8Array;
}

/**
 * Bytes of the meshes the states reference that `live` does not, each mesh counted once. `live` holds what the
 * session keeps anyway: the working copy and the mesh it started from.
 */
export function retainedBytes(states: readonly RepairSnapshot[], live: readonly MeshData[]): number {
  const counted = new Set<MeshData>(live);
  let total = 0;
  for (const state of states) {
    if (counted.has(state.mesh)) continue;
    counted.add(state.mesh);
    total += meshBytes(state.mesh);
  }
  return total;
}

/**
 * The undo list after a step: `before` (the state the step started from, with the step's label) added at the end.
 * The oldest states go first when there are more than `limits.steps`, or when the meshes only the list holds
 * exceed `limits.bytes`; the newest always stays so the step just made can be undone. A step clears the redo
 * list, so the caller drops `future`.
 */
export function pushStep(
  past: readonly RepairSnapshot[],
  before: RepairSnapshot,
  live: readonly MeshData[],
  limits: HistoryLimits = REPAIR_HISTORY_LIMITS,
): RepairSnapshot[] {
  let out = [...past, before];
  if (out.length > limits.steps) out = out.slice(out.length - limits.steps);
  while (out.length > 1 && retainedBytes(out, live) > limits.bytes) out = out.slice(1);
  return out;
}

export interface HistoryMove {
  /** The state to show now. */
  target: RepairSnapshot;
  past: RepairSnapshot[];
  future: RepairSnapshot[];
}

/** Undo: the state before the last step, and both lists after it; `current` moves to the redo list. Null when there is nothing to undo. */
export function undoStep(past: readonly RepairSnapshot[], future: readonly RepairSnapshot[], current: WorkingState): HistoryMove | null {
  const target = past[past.length - 1];
  if (!target) return null;
  return {
    target,
    past: past.slice(0, -1),
    future: [{ label: target.label, mesh: current.mesh, selection: current.selection }, ...future],
  };
}

/** Redo: the state after the next undone step, and both lists after it; `current` moves to the undo list. Null when there is nothing to redo. */
export function redoStep(past: readonly RepairSnapshot[], future: readonly RepairSnapshot[], current: WorkingState): HistoryMove | null {
  const target = future[0];
  if (!target) return null;
  return {
    target,
    past: [...past, { label: target.label, mesh: current.mesh, selection: current.selection }],
    future: future.slice(1),
  };
}
