import type { MeshData, Part } from "./store";

/** The undoable part of the app state. Parts and their meshes are immutable, so a snapshot is a few references. */
export interface Snapshot {
  parts: Part[];
  selection: string[];
}

export interface HistoryEntry {
  /** What the step did, e.g. "Move" or "Delete". */
  label: string;
  before: Snapshot;
  after: Snapshot;
  /** Which copy a paste belongs to. Undo and redo of an older copy do not move the next paste. */
  clipboard?: number;
}

export interface History {
  /** Oldest first; the last entry is the next undo. */
  past: HistoryEntry[];
  /** Next redo first. */
  future: HistoryEntry[];
}

export interface HistoryLimits {
  steps: number;
  /** Bytes of mesh data that only the history keeps alive. */
  bytes: number;
}

export const HISTORY_LIMITS: HistoryLimits = { steps: 50, bytes: 2 * 1024 ** 3 };

export const emptyHistory = (): History => ({ past: [], future: [] });

export function meshBytes(mesh: MeshData): number {
  return mesh.positions.byteLength + mesh.indices.byteLength;
}

/** Bytes of the meshes the entries reference that `current` does not, each mesh counted once. */
export function retainedBytes(entries: HistoryEntry[], current: Part[]): number {
  const live = new Set(current.map((p) => p.mesh));
  const counted = new Set<MeshData>();
  let total = 0;
  for (const entry of entries) {
    for (const part of [...entry.before.parts, ...entry.after.parts]) {
      if (live.has(part.mesh) || counted.has(part.mesh)) continue;
      counted.add(part.mesh);
      total += meshBytes(part.mesh);
    }
  }
  return total;
}

/**
 * Adds a step and clears the redo list. The oldest steps go first when there are more than
 * `limits.steps`, or when the meshes only the history holds exceed `limits.bytes`; the newest step
 * always stays so the change just made can be undone.
 */
export function pushEntry(history: History, entry: HistoryEntry, current: Part[], limits = HISTORY_LIMITS): History {
  let past = [...history.past, entry];
  if (past.length > limits.steps) past = past.slice(past.length - limits.steps);
  while (past.length > 1 && retainedBytes(past, current) > limits.bytes) past = past.slice(1);
  return { past, future: [] };
}

/** The step to undo and the history after undoing it, or null when there is nothing to undo. */
export function undoStep(history: History): { entry: HistoryEntry; history: History } | null {
  const entry = history.past[history.past.length - 1];
  if (!entry) return null;
  return { entry, history: { past: history.past.slice(0, -1), future: [entry, ...history.future] } };
}

/** The step to redo and the history after redoing it, or null when there is nothing to redo. */
export function redoStep(history: History): { entry: HistoryEntry; history: History } | null {
  const entry = history.future[0];
  if (!entry) return null;
  return { entry, history: { past: [...history.past, entry], future: history.future.slice(1) } };
}
