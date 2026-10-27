// Move / Rotate / Scale commands, the numeric dialog's maths, and Arrange › Place / Centre on platform.
// Every command changes part matrices only (meshes stay untouched) and records exactly one undo step.
import { parseNumber } from "./numbers";
import { applyDelta, det3, multiply, rotationAbout, translationMatrix, type Axis } from "./matrix";
import { centreOf, partBounds, unionBounds, type Bounds } from "./mesh";
import { useStore, type AppState, type Part, type TransformMode, type Vec3 } from "./store";
import { toast, useUi } from "./ui";

// Pure maths ---------------------------------------------------------------------------------

/** Largest position, offset or size accepted, in mm. */
export const MAX_MM = 100000;
/** Scale limits in percent of the current size. */
export const MIN_PERCENT = 0.01;
export const MAX_PERCENT = 100000;

const AXIS_IDS: readonly Axis[] = [0, 1, 2];

/**
 * Corrections smaller than this (mm) are rounding noise left by an earlier move of a rotated part,
 * not a move; dropping them keeps a second press from recording an undo step that changes nothing.
 */
const NOISE_MM = 1e-6;

const tidy = (d: number) => (Math.abs(d) <= NOISE_MM ? 0 : d);

/**
 * A number typed in a dialog field: "12,5", "12.5", "12.5 mm", "22,5°" or "50 %", read like the Clip
 * panel's fields. NaN when the text is not a number. The fields are plain text because a number input
 * under an English UI drops a typed comma ("12,5" becomes 125) without saying so.
 */
export function parseField(text: string): number {
  return parseNumber(text.trim().replace(/\s*[°%]$/, "")) ?? NaN;
}

/** Up to 3 decimals, no trailing zeros, never "-0". */
export function formatField(v: number): string {
  const r = Math.round(v * 1000) / 1000;
  return String(r === 0 ? 0 : r);
}

/** The field's text after an arrow-key step of `by`; null when it holds no number to step from. */
export function stepField(text: string, by: number): string | null {
  const v = parseField(text);
  return Number.isFinite(v) ? formatField(v + by) : null;
}

/** What a transform acts on: the selected parts that are shown and not locked, like the gizmo and the selection brackets. */
export interface Targets {
  parts: Part[];
  /** Selected parts left out because they are locked. */
  locked: number;
  /** Selected parts left out because they are hidden (and not locked). */
  hidden: number;
}

export function transformTargets(s: Pick<AppState, "parts" | "selection">): Targets {
  const ids = new Set(s.selection);
  const out: Targets = { parts: [], locked: 0, hidden: 0 };
  for (const p of s.parts) {
    if (!ids.has(p.id)) continue;
    if (p.locked) out.locked++;
    else if (!p.visible) out.hidden++;
    else out.parts.push(p);
  }
  return out;
}

const count = (n: number, noun: string) => (n === 1 ? `1 ${noun}` : `${n} ${noun}s`);

/** "1 hidden part and 2 locked parts"; empty when nothing is left out. */
export function leftOutParts({ hidden, locked }: Pick<Targets, "hidden" | "locked">): string {
  const bits: string[] = [];
  if (hidden) bits.push(count(hidden, "hidden part"));
  if (locked) bits.push(count(locked, "locked part"));
  return bits.join(" and ");
}

/** Combined world box of the parts; null when none has geometry. */
export function groupBox(parts: readonly Part[]): Bounds | null {
  return unionBounds(parts.map(partBounds));
}

export function boxSize(box: Bounds): Vec3 {
  return [box.max[0] - box.min[0], box.max[1] - box.min[1], box.max[2] - box.min[2]];
}

/** The point of the box that "Move to position" places: its minimum corner (Z = 0 rests on the platform) or its centre. */
export type RefPoint = "min" | "centre";

export function refPoint(box: Bounds, ref: RefPoint): Vec3 {
  return ref === "min" ? [...box.min] : centreOf(box);
}

/** Translation that brings the box's reference point to `target`. */
export function moveToDelta(box: Bounds, ref: RefPoint, target: Vec3): number[] {
  const p = refPoint(box, ref);
  return translationMatrix([tidy(target[0] - p[0]), tidy(target[1] - p[1]), tidy(target[2] - p[2])]);
}

export function rotateDelta(axis: Axis, degrees: number, pivot: Vec3): number[] {
  return rotationAbout(axis, (degrees * Math.PI) / 180, pivot);
}

/**
 * Per-axis factors that turn the `current` box size into `target`. With `keep` the `driver` axis sets
 * one factor for all three. An axis with no extent (a flat part) cannot be sized and keeps factor 1.
 * Null when the driver axis has no extent.
 */
export function sizeFactors(current: Vec3, target: Vec3, keep: boolean, driver: Axis): Vec3 | null {
  if (keep) {
    if (!(current[driver] > 0)) return null;
    const f = target[driver] / current[driver];
    return [f, f, f];
  }
  return [0, 1, 2].map((i) => (current[i] > 0 ? target[i] / current[i] : 1)) as Vec3;
}

/** All three sizes that keep the box's proportions when axis `i` is set to `value` mm; null when they cannot be worked out. */
export function proportionalSizes(current: Vec3, i: Axis, value: number): Vec3 | null {
  if (!Number.isFinite(value) || !(value > 0) || !(current[i] > 0)) return null;
  return current.map((c, j) => (j === i ? value : (c * value) / current[i])) as Vec3;
}

/** What the Scale dialog's fields hold. */
export interface ScaleInput {
  /** "To size" (target sizes in mm) rather than "By percentage". */
  bySize: boolean;
  /** Keep proportions: the `driver` field sets one factor for all three axes. */
  keep: boolean;
  /** Field values per axis; NaN for an empty field. */
  percent: readonly number[];
  size: readonly number[];
  /** The field edited last. */
  driver: Axis;
  /** The selection's current world box size. */
  current: Vec3;
}

export interface ScaleCheck {
  /** Per-axis factors, also when they are out of range so the dialog can show the result; null when the fields give none. */
  factors: Vec3 | null;
  /** What to fix, or null when the factors can be applied. */
  error: string | null;
  /** Axes whose input field is at fault. */
  bad: Axis[];
}

/**
 * Scale factors from the dialog's fields, or what to fix. Besides the typed fields this checks every
 * resulting size, including those the lock derives from the driver and those a percentage gives.
 */
export function checkScale({ bySize, keep, percent, size, driver, current }: ScaleInput): ScaleCheck {
  const fail = (error: string, bad: Axis[], factors: Vec3 | null = null): ScaleCheck => ({ factors, error, bad });
  let factors: Vec3 | null;
  if (!bySize) {
    // With the lock on all three percent fields mirror the driver.
    const p = AXIS_IDS.map((i) => percent[keep ? driver : i]);
    const missing = AXIS_IDS.filter((i) => !Number.isFinite(p[i]));
    if (missing.length) return fail(keep ? "Enter a number for the scale." : "Enter a number for X, Y and Z.", missing);
    const out = AXIS_IDS.filter((i) => p[i] < MIN_PERCENT || p[i] > MAX_PERCENT);
    if (out.length) return fail(`Enter a scale from ${MIN_PERCENT} to ${MAX_PERCENT} %.`, out);
    factors = p.map((v) => v / 100) as Vec3;
  } else {
    // A flat axis cannot be sized; with the lock on only the driver is typed and the others follow it.
    const used = keep ? [driver] : AXIS_IDS.filter((i) => current[i] > 0);
    const missing = used.filter((i) => !Number.isFinite(size[i]));
    if (missing.length) return fail("Enter a size for each axis.", missing);
    const out = used.filter((i) => !(size[i] > 0 && size[i] <= MAX_MM));
    if (out.length) return fail(`Enter a size greater than 0 and up to ${MAX_MM} mm.`, out);
    factors = sizeFactors(current, [size[0], size[1], size[2]], keep, driver);
    if (!factors) return fail("The selection has no size to scale.", []);
  }
  const f = factors;
  // The small allowance lets a typed 100000 mm through despite rounding in size × (target / size).
  const big = AXIS_IDS.filter((i) => current[i] * f[i] > MAX_MM + NOISE_MM);
  if (big.length) return fail(`The new size can be at most ${MAX_MM} mm.`, big, f);
  const extreme = AXIS_IDS.filter((i) => f[i] < MIN_PERCENT / 100 || f[i] > MAX_PERCENT / 100);
  if (extreme.length) return fail(`The new size must be ${MIN_PERCENT} % to ${MAX_PERCENT} % of the current size.`, extreme, f);
  return { factors: f, error: null, bad: [] };
}

/** Each part moved in Z alone so that its own lowest point rests on the platform (Z = 0). Parts already there are left out. */
export function placeOnPlatformMatrices(parts: readonly Part[]): Map<string, number[]> {
  const out = new Map<string, number[]>();
  for (const p of parts) {
    const b = partBounds(p);
    const dz = b ? tidy(-b.min[2]) : 0;
    if (dz !== 0) out.set(p.id, multiply(translationMatrix([0, 0, dz]), p.matrix));
  }
  return out;
}

/** The parts moved together in X and Y so their combined box is centred on the platform; Z stays. Empty when already centred. */
export function centreOnPlatformMatrices(parts: readonly Part[], platform: Vec3): Map<string, number[]> {
  const box = groupBox(parts);
  if (!box) return new Map();
  const [cx, cy] = centreOf(box);
  const dx = tidy(platform[0] / 2 - cx);
  const dy = tidy(platform[1] / 2 - cy);
  if (dx === 0 && dy === 0) return new Map();
  return applyDelta(parts, translationMatrix([dx, dy, 0]));
}

/** False when a matrix would be unusable (not finite, or flattened to nothing). */
function usable(matrices: Map<string, number[]>): boolean {
  for (const m of matrices.values()) {
    if (!m.every(Number.isFinite)) return false;
    const d = det3(m);
    if (d === 0 || !Number.isFinite(d)) return false;
  }
  return true;
}

// Commands -----------------------------------------------------------------------------------

const WORDS: Record<TransformMode, { verb: string; past: string }> = {
  translate: { verb: "move", past: "moved" },
  rotate: { verb: "rotate", past: "rotated" },
  scale: { verb: "scale", past: "scaled" },
};

/** The selection's shown, unlocked parts, or null after telling the user why there is nothing to change. */
function targetsOrExplain(verb: string, past: string): Targets | null {
  const s = useStore.getState();
  const t = transformTargets(s);
  if (t.parts.length) return t;
  if (t.locked === 0 && t.hidden === 0) toast(`Select the parts to ${verb} first.`);
  else if (t.hidden === 0) toast(`Locked parts cannot be ${past}. Unlock them first.`, "warn");
  else if (t.locked === 0) toast(`Hidden parts cannot be ${past}. Show them first.`, "warn");
  else toast(`Hidden or locked parts cannot be ${past}.`, "warn");
  return null;
}

/** Warns about selected parts a command left alone. */
function reportLeftOut(t: Targets, past: string) {
  const n = t.hidden + t.locked;
  if (n) toast(`${leftOutParts(t)} ${n === 1 ? "was" : "were"} not ${past}.`, "warn");
}

/** Opens the numeric Move / Rotate / Scale dialog for the selection, or says why it cannot. */
export function openTransformDialog(mode: TransformMode): void {
  const { verb, past } = WORDS[mode];
  if (targetsOrExplain(verb, past)) useUi.getState().openDialog({ kind: "transform", mode });
}

/** Move / Rotate / Scale buttons: show the gizmo that does it (Move and Rotate share the combined one) and open the numeric dialog. */
export function startTransform(mode: TransformMode): void {
  useStore.getState().setGizmoMode(mode === "scale" ? "scale" : "combined");
  openTransformDialog(mode);
}

/**
 * Scale mode lasts for one scale: afterwards the selection shows the move and rotate gizmo again. While View › Always
 * show gizmo is on that gizmo shows under the Select tool anyway, so Scale goes back to Select and the next Esc clears
 * the selection instead of only renaming the tool in the status bar. Esc in Scale ends it the same way.
 */
export function finishScale(): void {
  const s = useStore.getState();
  if (s.gizmoMode === "scale") s.setGizmoMode(s.view.alwaysShowGizmo ? null : "combined");
}

/**
 * Applies a world-space `delta` (column-major) to the parts `ids` as one undo step, leaving out any that
 * are no longer selected, shown and unlocked. The dialog passes the parts it was opened for, so a
 * selection change while it is open cannot redirect it. Returns false, changing nothing, when the result
 * would be unusable (for example scaled to nothing).
 */
export function transformParts(ids: readonly string[], delta: readonly number[], label: string): boolean {
  const wanted = new Set(ids);
  const parts = transformTargets(useStore.getState()).parts.filter((p) => wanted.has(p.id));
  const matrices = applyDelta(parts, delta);
  if (!usable(matrices)) return false;
  if (matrices.size) useStore.getState().setPartMatrices(matrices, label);
  return true;
}

/**
 * The Scale dialog's OK: scales the parts `ids` by the world-space `delta` as one "Scale" step (transformParts), and
 * then, as after a scale gizmo drag, ends Scale mode (finishScale). Returns false when the scale is refused: nothing
 * changes and Scale stays on, so the dialog can say why and the user can try another value.
 */
export function applyScale(ids: readonly string[], delta: readonly number[]): boolean {
  if (!transformParts(ids, delta, "Scale")) return false;
  finishScale();
  return true;
}

/** Sets matrices as one undo step; tells the user when nothing moved, since no step is recorded then. */
function arrange(matrices: Map<string, number[]>, label: string, unchanged: string) {
  const before = useStore.getState().parts;
  useStore.getState().setPartMatrices(matrices, label);
  if (useStore.getState().parts === before) toast(unchanged);
}

/** Arrange › Place on platform: rest each selected part on Z = 0. */
export function placeOnPlatform(): void {
  const t = targetsOrExplain("place on the platform", "moved");
  if (!t) return;
  arrange(placeOnPlatformMatrices(t.parts), "Place on platform", "The selection already rests on the platform.");
  reportLeftOut(t, "moved");
}

/** Arrange › Centre on platform: move the selection to the platform's centre in X and Y. */
export function centreOnPlatform(): void {
  const t = targetsOrExplain("centre on the platform", "moved");
  if (!t) return;
  const platform = useStore.getState().platform;
  arrange(centreOnPlatformMatrices(t.parts, platform), "Centre on platform", "The selection is already centred on the platform.");
  reportLeftOut(t, "moved");
}
