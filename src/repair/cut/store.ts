// Cut surfaces' state: the clicked points, the current one, the panel's settings and the routed line Rust last sent
// for them. src/repair/cut/session.ts is the only code that changes it.
import { create } from "zustand";
import type { Vec3 } from "../../store";
import type { CutPath } from "../ipc";

/** Where a new point goes in the line: next to the nearest stretch of it, or after the current point. */
export type InsertOrder = "nearest" | "last";

export interface CutSettings {
  /** Join the points with a smooth curve. */
  smooth: boolean;
  /** mm between the smooth curve's samples. */
  step: number;
  /** mm; points and crossings this close to a vertex go through it. */
  snap: number;
  /** Join the last point back to the first. */
  closed: boolean;
  /** Route the line again after every change; off, only the Update button does. */
  auto: boolean;
  order: InsertOrder;
}

/** The panel's defaults; Smooth step length, Snap distance and Close line match Rust's (surface_cut::OPTIONS). */
export const DEFAULT_CUT: CutSettings = { smooth: false, step: 2, snap: 0.5, closed: false, auto: true, order: "last" };

/** A clicked point: where on the working copy, the triangle the click hit, and an id that stays as points come and go. */
export interface CutPoint {
  id: number;
  at: Vec3;
  triangle: number;
}

export interface CutState {
  points: CutPoint[];
  /** Index of the current point (Previous point / Next point move it; At last position inserts after it); -1 for none. */
  current: number;
  /** Point ids in the order they were added, for Undo last point. */
  added: number[];
  settings: CutSettings;
  /** The routed line for `routedFor`; null before the first route or with fewer than two points. */
  path: CutPath | null;
  /** The points and settings `path` was routed for. */
  routedFor: { points: CutPoint[]; settings: CutSettings } | null;
  /** A route is on its way from Rust. */
  routing: boolean;
  /** Why the last route failed, as a sentence; null when it worked. */
  routeError: string | null;
}

export const useCut = create<CutState>()(() => ({
  points: [],
  current: -1,
  added: [],
  settings: DEFAULT_CUT,
  path: null,
  routedFor: null,
  routing: false,
  routeError: null,
}));

/** True when the two settings route the same line (Automatic update and the insert order do not change it). */
export function sameRoute(a: CutSettings, b: CutSettings): boolean {
  return a.smooth === b.smooth && a.step === b.step && a.snap === b.snap && a.closed === b.closed;
}

/** True when the line drawn is not the line for the points and settings as they are now. */
export function pathStale(s: Pick<CutState, "points" | "settings" | "routedFor">): boolean {
  return s.routedFor === null || s.routedFor.points !== s.points || !sameRoute(s.routedFor.settings, s.settings);
}
