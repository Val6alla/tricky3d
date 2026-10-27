// Clip planes (right panel › Clip planes) and the Cuts scope. View state only: not undoable, not saved.
import { create } from "zustand";
import { centreOf, partBounds, unionBounds, type Bounds } from "./mesh";
import type { Part, Vec3 } from "./store";

export type ClipAxis = "x" | "y" | "z" | "custom";
export const CLIP_AXES: readonly ClipAxis[] = ["x", "y", "z", "custom"];

export interface ClipRow {
  enabled: boolean;
  /** Plane position in mm along the row's direction; null means the middle of the current range. */
  offset: number | null;
  /** Keeps the other side: coordinate >= offset instead of <= offset. */
  flipped: boolean;
}

/** Which parts the Cut button cuts. */
export type CutScope = "selected" | "all";

export interface ClipSettings {
  rows: Record<ClipAxis, ClipRow>;
  /** Unit direction of the Custom plane. */
  customNormal: Vec3;
  /** Draws the clipped-away geometry ghosted instead of hiding it. */
  transparentCuts: boolean;
  cutScope: CutScope;
}

/** A plane in three.js form: it keeps the points where dot(normal, p) + constant >= 0. */
export interface PlaneSpec {
  normal: Vec3;
  constant: number;
}

/** Lowest and highest offset, in mm. */
export type Range = [min: number, max: number];

/** Points diagonally at the default iso camera, so turning Custom on opens the corner facing the user. */
export const DEFAULT_CUSTOM_NORMAL: Vec3 = [1 / Math.sqrt(3), -1 / Math.sqrt(3), 1 / Math.sqrt(3)];

const AXIS_DIRECTIONS: Record<Exclude<ClipAxis, "custom">, Vec3> = { x: [1, 0, 0], y: [0, 1, 0], z: [0, 0, 1] };

const dot = (a: Vec3, b: Vec3) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];

/** Range of the sliders: the platform box joined with every visible part's exact world box. */
export function sceneBounds(platform: Vec3, parts: readonly Part[]): Bounds {
  const box: Bounds = { min: [0, 0, 0], max: [...platform] };
  return unionBounds([box, ...parts.filter((p) => p.visible).map(partBounds)]) ?? box;
}

/** Direction the row's offset is measured along. */
export function rowDirection(axis: ClipAxis, customNormal: Vec3): Vec3 {
  return axis === "custom" ? customNormal : AXIS_DIRECTIONS[axis];
}

/** Lowest and highest dot(direction, corner) over the box's 8 corners: where the plane just touches it. */
export function offsetRange(direction: Vec3, b: Bounds): Range {
  let lo = 0;
  let hi = 0;
  for (let a = 0; a < 3; a++) {
    const d = direction[a];
    if (d === 0) continue;
    lo += d * (d > 0 ? b.min[a] : b.max[a]);
    hi += d * (d > 0 ? b.max[a] : b.min[a]);
  }
  return [lo, hi];
}

export function clampTo(value: number, [lo, hi]: Range): number {
  return Math.min(hi, Math.max(lo, value));
}

/** The offset a row shows and cuts at: its own value clamped to the range, or the middle when unset. */
export function rowOffset(row: ClipRow, range: Range): number {
  return clampTo(row.offset ?? (range[0] + range[1]) / 2, range);
}

/** Range of one row's offset for the current scene. */
export function rowRange(axis: ClipAxis, s: Pick<ClipSettings, "customNormal">, bounds: Bounds): Range {
  return offsetRange(rowDirection(axis, s.customNormal), bounds);
}

/**
 * How far each plane is moved into the side it removes, in mm. The GPU tests every pixel against the
 * planes in 32-bit floats relative to the camera, off by up to about 1e-6 of the scene's size, so a face
 * lying exactly on a plane (a slider on a whole mm against a part's flat side) was kept or dropped pixel by
 * pixel and drew as speckled noise. 1e-5 of the scene's diagonal stays clear of that error at any scale
 * (a fixed margin would not on a large platform) and below the 0.01 mm the panel shows on a usual one.
 */
export function planeMargin(bounds: Bounds): number {
  return 1e-5 * Math.hypot(bounds.max[0] - bounds.min[0], bounds.max[1] - bounds.min[1], bounds.max[2] - bounds.min[2]);
}

/**
 * The enabled planes in three.js form. A row keeps dot(direction, p) <= offset (the part beyond the
 * slider is cut away), or the other side when flipped. A point is hidden if any plane removes it.
 * Each plane sits planeMargin() beyond the offset, so faces on the plane are kept whole; the offsets
 * the panel shows do not move.
 */
export function activePlanes(s: Pick<ClipSettings, "rows" | "customNormal">, bounds: Bounds): PlaneSpec[] {
  const out: PlaneSpec[] = [];
  const margin = planeMargin(bounds);
  for (const axis of CLIP_AXES) {
    const row = s.rows[axis];
    if (!row.enabled) continue;
    const d = rowDirection(axis, s.customNormal);
    const offset = rowOffset(row, offsetRange(d, bounds));
    out.push(
      row.flipped
        ? { normal: [d[0], d[1], d[2]], constant: margin - offset }
        : { normal: [-d[0], -d[1], -d[2]], constant: margin + offset },
    );
  }
  return out;
}

/** True when any of the planes removes the point. */
export function isPointClipped(planes: readonly PlaneSpec[], p: Vec3): boolean {
  return planes.some((plane) => dot(plane.normal, p) + plane.constant < 0);
}

/**
 * True when some plane removes part of the box: its corner furthest into the removed side, the lowest
 * dot(normal, corner) of the 8, is clipped. On a part's exact world box this is exact for the X, Y and Z
 * planes; a Custom plane may only clip an empty corner of the box.
 */
export function isBoxCut(planes: readonly PlaneSpec[], b: Bounds | null): boolean {
  return b !== null && planes.some((plane) => offsetRange(plane.normal, b)[0] + plane.constant < 0);
}

export function anyEnabled(s: Pick<ClipSettings, "rows">): boolean {
  return CLIP_AXES.some((axis) => s.rows[axis].enabled);
}

/** Unit vector along `v`, or null for a zero or non-finite vector. */
export function normalized(v: Vec3): Vec3 | null {
  const length = Math.hypot(v[0], v[1], v[2]);
  if (!Number.isFinite(length) || length < 1e-9) return null;
  return [v[0] / length, v[1] / length, v[2] / length];
}

/**
 * Offset along `next` for a plane that keeps passing through the same point when its normal turns
 * from `prev` to `next`: the point of the old plane nearest the scene centre. Changing the direction
 * then pivots the plane in place instead of moving it to wherever the old offset lands.
 */
export function reanchorOffset(offset: number, prev: Vec3, next: Vec3, bounds: Bounds): number {
  const centre = centreOf(bounds);
  return dot(next, centre) + (offset - dot(prev, centre)) * dot(next, prev);
}

/** Slider step: a 1-2-5 value near 1/200 of the span, so arrow keys move a useful amount. */
export function sliderStep(span: number): number {
  if (!(span > 0) || !Number.isFinite(span)) return 1;
  const raw = span / 200;
  const pow = 10 ** Math.floor(Math.log10(raw));
  const f = raw / pow;
  return (f < 1.5 ? 1 : f < 3.5 ? 2 : f < 7.5 ? 5 : 10) * pow;
}

/** Offset as shown in the panel: 2 decimals, never "-0.00". */
export function formatMm(v: number): string {
  return (Math.abs(v) < 0.005 ? 0 : v).toFixed(2);
}

const offRow = (): ClipRow => ({ enabled: false, offset: null, flipped: false });
const offRows = (): Record<ClipAxis, ClipRow> => ({ x: offRow(), y: offRow(), z: offRow(), custom: offRow() });

export interface ClipState extends ClipSettings {
  /** Turns a plane on or off. A plane turned on with no offset yet starts in the middle of `range`. */
  setEnabled(axis: ClipAxis, enabled: boolean, range: Range): void;
  setOffset(axis: ClipAxis, offset: number): void;
  setFlipped(axis: ClipAxis, flipped: boolean): void;
  /**
   * Points the Custom plane along `normal` (normalised), turning it about the point it passes
   * nearest the centre of `bounds`. Returns false and changes nothing for a zero vector.
   */
  setCustomNormal(normal: Vec3, bounds: Bounds): boolean;
  setTransparentCuts(on: boolean): void;
  setCutScope(scope: CutScope): void;
  /** All planes off, offsets back to mid-range, no flips, transparent cuts off. Keeps the custom direction and cut scope. */
  reset(): void;
}

function patchRow(s: ClipSettings, axis: ClipAxis, patch: Partial<ClipRow>): Pick<ClipSettings, "rows"> {
  return { rows: { ...s.rows, [axis]: { ...s.rows[axis], ...patch } } };
}

export const useClip = create<ClipState>()((set, get) => ({
  rows: offRows(),
  customNormal: DEFAULT_CUSTOM_NORMAL,
  transparentCuts: false,
  cutScope: "selected",

  setEnabled(axis, enabled, range) {
    const row = get().rows[axis];
    if (row.enabled === enabled) return;
    set((s) => patchRow(s, axis, enabled ? { enabled, offset: rowOffset(row, range) } : { enabled }));
  },

  setOffset(axis, offset) {
    if (Number.isFinite(offset) && get().rows[axis].offset !== offset) set((s) => patchRow(s, axis, { offset }));
  },

  setFlipped(axis, flipped) {
    if (get().rows[axis].flipped !== flipped) set((s) => patchRow(s, axis, { flipped }));
  },

  setCustomNormal(normal, bounds) {
    const next = normalized(normal);
    if (!next) return false;
    const s = get();
    const row = s.rows.custom;
    const shown = rowOffset(row, offsetRange(s.customNormal, bounds));
    const offset = row.offset === null ? null : reanchorOffset(shown, s.customNormal, next, bounds);
    set({ customNormal: next, ...patchRow(s, "custom", { offset }) });
    return true;
  },

  setTransparentCuts(on) {
    set({ transparentCuts: on });
  },

  setCutScope(scope) {
    set({ cutScope: scope });
  },

  reset() {
    set({ rows: offRows(), transparentCuts: false });
  },
}));
