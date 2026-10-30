// Arrange › Align, Lay flat, Rotate to smallest box, Orient and Duplicate. Rust works out the new matrices
// (src/arrange/ipc.ts); this module picks the parts, runs the job with progress and Cancel (src/arrange/ArrangeBusy.tsx),
// names the copies and applies the answer as one undo step. It follows the mesh operations' rules (src/ops/meshOps.ts):
// one at a time, none in repair mode, and nothing applied when the run was cancelled or the parts changed meanwhile.
import { create } from "zustand";
import { browserOnly } from "../io/files";
import { nextJob } from "../jobs";
import { isStale, meshOperationRunning, runMeshOperation, staleMessage, WAIT_MESSAGE } from "../ops/meshOps";
import { CANCELLED, cancelRepair } from "../repair/ipc";
import { CLOSE_REPAIR_FIRST, isRepairing } from "../repair/session";
import { DEFAULT_PACKING, useStore, type NewPart, type Part, type Vec3 } from "../store";
import { transformTargets } from "../transformTools";
import { toast, useUi } from "../ui";
import { runArrange, type ArrangePart } from "./ipc";
import type { AlignHow, BoxAxes, DuplicateHow, DuplicateMode, OrientCandidate, OrientGoal, OrientScores } from "./types";

/** A run in progress: what the busy box shows, and what Cancel stops. */
export interface ArrangeBusy {
  job: number;
  label: string;
  fraction: number;
  cancelling: boolean;
}

export const useArrange = create<{ busy: ArrangeBusy | null }>(() => ({ busy: null }));

const s = () => useStore.getState();

/** Stops the run in progress: Rust stops at once and nothing changes. */
export function cancelArrange(): void {
  const busy = useArrange.getState().busy;
  if (!busy || busy.cancelling) return;
  useArrange.setState({ busy: { ...busy, cancelling: true } });
  cancelRepair(busy.job).catch(() => {});
}

/** Says why an arrange command cannot start now, or returns false when it can. */
export function refusedNow(what: string): boolean {
  if (browserOnly(what)) return true;
  if (isRepairing()) {
    toast(CLOSE_REPAIR_FIRST);
    return true;
  }
  if (meshOperationRunning()) {
    toast(WAIT_MESSAGE);
    return true;
  }
  return false;
}

const meshOf = (p: Part, target: boolean): ArrangePart => {
  const { priority, rotation } = p.packing ?? DEFAULT_PACKING;
  return { name: p.name, mesh: p.mesh, matrix: p.matrix, target, priority, rotation };
};

/**
 * Runs `action` in Rust on `targets` (and `others`, the parts in the way), with the busy box's progress and Cancel.
 * Returns Rust's answer, or null when it was cancelled, failed (said in a toast) or the parts changed meanwhile.
 */
export async function arrangeJob(
  label: string,
  action: string,
  settings: Record<string, unknown>,
  targets: readonly Part[],
  others: readonly Part[] = [],
): Promise<Record<string, unknown> | null> {
  const start = s();
  const generation = start.generation;
  const job = nextJob();
  let answer: Record<string, unknown> | null = null;
  let cancelled = false;
  useArrange.setState({ busy: { job, label, fraction: 0, cancelling: false } });
  try {
    await runMeshOperation(label, `Could not finish ${label}`, async () => {
      try {
        const reply = await runArrange(
          action,
          settings,
          [...targets.map((p) => meshOf(p, true)), ...others.map((p) => meshOf(p, false))],
          start.platform,
          job,
          (fraction) => {
            const busy = useArrange.getState().busy;
            if (busy?.job === job) useArrange.setState({ busy: { ...busy, fraction } });
          },
        );
        if (useArrange.getState().busy?.cancelling) {
          cancelled = true;
          return;
        }
        if (isStale([...targets, ...others], generation, s())) {
          toast(staleMessage(label), "warn");
          return;
        }
        answer = reply;
      } catch (err) {
        if (err !== CANCELLED) throw err;
        cancelled = true;
      }
    });
  } finally {
    if (useArrange.getState().busy?.job === job) useArrange.setState({ busy: null });
  }
  if (cancelled) toast(`${label} was cancelled. Nothing changed.`);
  return answer;
}

/** The parts a command acts on (selected, shown, unlocked), or null after saying why there are not enough. */
function targetsOr(verb: string, past: string, min = 1): Part[] | null {
  const t = transformTargets(s());
  if (t.parts.length >= min) return t.parts;
  if (t.locked + t.hidden > 0 && t.parts.length === 0) {
    toast(t.hidden === 0 ? `Locked parts cannot be ${past}. Unlock them first.` : t.locked === 0 ? `Hidden parts cannot be ${past}. Show them first.` : `Hidden or locked parts cannot be ${past}.`, "warn");
  } else {
    toast(min > 1 ? `Select at least ${min} parts to ${verb}.` : `Select the parts to ${verb} first.`);
  }
  return null;
}

/** Each target's new matrix from an answer's `matrices`; parts whose matrix stays out of it. */
function matricesOf(targets: readonly Part[], answer: Record<string, unknown>): Map<string, number[]> {
  const list = (answer.matrices ?? []) as number[][];
  const out = new Map<string, number[]>();
  targets.forEach((p, i) => {
    const m = list[i];
    if (m && m.length === 16 && m.every(Number.isFinite)) out.set(p.id, m);
  });
  return out;
}

/** Applies matrices as one undo step `label`; says so when nothing moved, since no step is recorded then. */
function applyMatrices(matrices: Map<string, number[]>, label: string, unchanged: string): void {
  const before = s().parts;
  s().setPartMatrices(matrices, label);
  if (s().parts === before) toast(unchanged);
}

// ---------------------------------------------------------------------------------------------
// Align

const ALIGN_LABELS: Record<AlignHow, string> = {
  left: "Align left",
  right: "Align right",
  front: "Align front",
  back: "Align back",
  bottom: "Align bottom",
  top: "Align top",
  "centre-x": "Centre in X",
  "centre-y": "Centre in Y",
  "centre-z": "Centre in Z",
  "platform-centre": "Centre on platform",
  origin: "Move to origin",
  drop: "Drop onto platform",
};

const RELATIVE: readonly AlignHow[] = ["left", "right", "front", "back", "bottom", "top", "centre-x", "centre-y", "centre-z"];

/** Arrange › Align: lines the selected parts up with each other, or puts them on the platform. One undo step. */
export async function alignParts(how: AlignHow): Promise<void> {
  const label = ALIGN_LABELS[how];
  if (refusedNow(label)) return;
  const targets = RELATIVE.includes(how) ? targetsOr("align", "aligned", 2) : targetsOr("move", "moved");
  if (!targets) return;
  const answer = await arrangeJob(label, "align", { align: how }, targets);
  if (answer) applyMatrices(matricesOf(targets, answer), label, "The parts are already in place.");
}

// ---------------------------------------------------------------------------------------------
// Lay flat, smallest box

/** Arrange › Lay flat automatically: each selected part on its largest flat face. One undo step. */
export async function layFlatAuto(): Promise<void> {
  if (refusedNow("Lay flat")) return;
  const targets = targetsOr("lay flat", "turned");
  if (!targets) return;
  const answer = await arrangeJob("Lay flat", "lay-flat-auto", {}, targets);
  if (answer) applyMatrices(matricesOf(targets, answer), "Lay flat", "The parts already lie flat.");
}

/** Arrange › Lay a face on the platform: the next click on a part's face lays it flat (src/viewport/picking.ts). */
export function startLayFlat(): void {
  if (refusedNow("Lay flat")) return;
  const ui = useUi.getState();
  ui.setViewTool(ui.viewTool === "lay-flat" ? null : "lay-flat");
}

/** The click of Lay a face on the platform: `hit` is the part, the world point and the face's normal there. */
export async function layFlatAt(hit: { partId: string; point: Vec3; normal: Vec3 }): Promise<void> {
  const part = s().parts.find((p) => p.id === hit.partId);
  if (!part) return;
  if (part.locked) {
    toast("Locked parts cannot be turned. Unlock it first.", "warn");
    return;
  }
  if (refusedNow("Lay flat")) return;
  const answer = await arrangeJob("Lay flat", "lay-flat", { point: hit.point, normal: hit.normal }, [part]);
  if (answer) applyMatrices(matricesOf([part], answer), "Lay flat", "That face already lies on the platform.");
}

/** Arrange › Rotate to smallest box: opens its dialog for the selection, or says why not. */
export function openMinBox(): void {
  if (refusedNow("Rotate to smallest box")) return;
  if (targetsOr("rotate", "rotated")) useUi.getState().openDialog({ kind: "min-box" });
}

/** The dialog's OK: turns each selected part to its smallest box, about Z only or all axes. One undo step. */
export async function minBox(axes: BoxAxes): Promise<void> {
  if (refusedNow("Rotate to smallest box")) return;
  const targets = targetsOr("rotate", "rotated");
  if (!targets) return;
  const answer = await arrangeJob("Rotate to smallest box", "min-box", { axes }, targets);
  if (answer) applyMatrices(matricesOf(targets, answer), "Rotate to smallest box", "The parts already have their smallest box.");
}

// ---------------------------------------------------------------------------------------------
// Orient

/** Home › Orient: opens its dialog for one selected part, or says why not. */
export function openOrient(): void {
  if (refusedNow("Orient")) return;
  const targets = targetsOr("orient", "turned");
  if (!targets) return;
  if (targets.length > 1) {
    toast("Select one part to orient.");
    return;
  }
  useUi.getState().openDialog({ kind: "orient", partId: targets[0].id });
}

/** How the part does now, and the best ways for it to stand by `goal`; null when cancelled or failed. */
export async function searchOrientations(part: Part, goal: OrientGoal): Promise<{ now: OrientScores; candidates: OrientCandidate[] } | null> {
  if (refusedNow("Orient")) return null;
  const answer = await arrangeJob("Orient", "orient", { goal }, [part]);
  if (!answer) return null;
  return { now: answer.now as OrientScores, candidates: (answer.candidates ?? []) as OrientCandidate[] };
}

// ---------------------------------------------------------------------------------------------
// Duplicate

const NUMBERED = /^(.*) \((\d+)\)$/;

/**
 * Names for copies of the parts named `sources`, in order: "<name> (2)", "(3)"… after the highest number any part in
 * `existing` already has for that name (the original counts as 1). A source that is itself a numbered copy counts on
 * from its base name.
 */
export function copyNames(existing: readonly string[], sources: readonly string[]): string[] {
  const highest = new Map<string, number>();
  for (const name of existing) {
    const m = NUMBERED.exec(name);
    const base = m ? m[1] : name;
    highest.set(base, Math.max(highest.get(base) ?? 1, m ? Number(m[2]) : 1));
  }
  return sources.map((name) => {
    const base = NUMBERED.exec(name)?.[1] ?? name;
    const k = (highest.get(base) ?? 1) + 1;
    highest.set(base, k);
    return `${base} (${k})`;
  });
}

export const DUPLICATE_LABELS: Record<DuplicateMode, string> = {
  "in-place": "Duplicate in place",
  fill: "Fill the platform",
  grid: "Duplicate in a grid",
};

/** Home › Duplicate and Arrange › Copies: opens the dialog for `mode`, or says why not. */
export function openDuplicate(mode: DuplicateMode): void {
  if (refusedNow(DUPLICATE_LABELS[mode])) return;
  if (targetsOr("duplicate", "duplicated")) useUi.getState().openDialog({ kind: "duplicate", mode });
}

/**
 * The dialog's OK: copies the selected parts as one group, as `how` says, named "<name> (2)" and on. Fill the platform
 * also moves the originals to the grid's first place and keeps clear of every other part. One undo step; the copies
 * end up selected. Returns how many copies were made.
 */
export async function duplicateParts(how: DuplicateHow): Promise<number> {
  const label = DUPLICATE_LABELS[how.mode];
  if (refusedNow(label)) return 0;
  const targets = targetsOr("duplicate", "duplicated");
  if (!targets) return 0;
  const ids = new Set(targets.map((p) => p.id));
  const others = how.mode === "fill" ? s().parts.filter((p) => !ids.has(p.id)) : [];
  const answer = await arrangeJob(label, "duplicate", { duplicate: how }, targets, others);
  if (!answer) return 0;
  const copies = (answer.copies ?? []) as { source: number; matrix: number[] }[];
  const names = copyNames(
    s().parts.map((p) => p.name),
    copies.map((c) => targets[c.source].name),
  );
  const add: NewPart[] = copies.map((c, i) => {
    const p = targets[c.source];
    return { name: names[i], mesh: p.mesh, matrix: c.matrix, color: p.color, packing: p.packing, source: p.source };
  });
  const made = s().arrangeParts(matricesOf(targets, answer), add, label).length;
  toast(made === 1 ? "Made 1 copy." : `Made ${made} copies.`);
  return made;
}
