// Cut surfaces: the cutting mode inside repair mode (Home › Repair › Cut surfaces, and the Repair tab's). The viewport
// hands over each point the user clicks on the working copy (addCutPoint); this file keeps the point list and the
// panel's settings. Everything that reads the mesh runs in Rust (src-tauri/src/repair/surface_cut.rs): the line along
// the surface between the points (fetchCutPath, for the viewport to draw) and the cut along it (the cut_surface
// action, one inner undo step with Cancel, like every repair run). Rust routes the line the same way for both, so the
// cut follows the line shown.
import { isTauri } from "@tauri-apps/api/core";
import { errorMessage } from "../../io/files";
import type { Vec3 } from "../../store";
import { toast } from "../../ui";
import { cutParams, fetchCutPath, forgetCutMesh, type CutLineOptions } from "../ipc";
import { runTool } from "../session";
import { useRepair } from "../store";
import { pathStale, sameRoute, useCut, type CutPoint, type CutSettings, type CutState, type InsertOrder } from "./store";

export const TWO_POINTS_FIRST = "Click at least two points on the surface to cut along.";

let nextId = 1;
/** Bumped by every route request and reset: an answer for an older one is dropped. */
let routeSeq = 0;
let watching = false;

const repair = () => useRepair.getState();

/** True while the cutting mode is on. */
export function isCutting(): boolean {
  return repair().session !== null && repair().editTool === "cut";
}

/**
 * Drops the points whenever the working copy changes under them (a cut, another action, undo, redo): they name its
 * triangles. Ends the cutting mode with repair mode. Subscribes once.
 */
function watchSession() {
  if (watching) return;
  watching = true;
  useRepair.subscribe((r, prev) => {
    if (r.session?.mesh !== prev.session?.mesh) clearPoints();
    if (!r.session && r.editTool !== null) useRepair.setState({ editTool: null });
    if (r.editTool !== "cut" && prev.editTool === "cut") {
      clearPoints();
      // Rust kept the working copy for drawing lines on; the mode is over.
      forgetCutMesh().catch((err: unknown) => console.error("repair_cut_reset failed", err));
    }
  });
}

/** Starts the cutting mode on the open repair session, with no points; a second call ends it. */
export function toggleCutMode(): void {
  if (!repair().session) return;
  watchSession();
  clearPoints();
  repair().setEditTool(isCutting() ? null : "cut");
}

/** Leaves the cutting mode; the points go. */
export function closeCutMode(): void {
  if (repair().editTool === "cut") repair().setEditTool(null);
  clearPoints();
}

function clearPoints() {
  routeSeq++;
  const s = useCut.getState();
  if (s.points.length === 0 && s.path === null && !s.routing && s.routeError === null) return;
  useCut.setState({ points: [], current: -1, added: [], path: null, routedFor: null, routing: false, routeError: null });
}

const distance = (a: Vec3, b: Vec3) => Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);

/**
 * Where a new point at `at` goes in `points`. "last": right after the current point (at the end when none is
 * current). "nearest": where it lengthens the line least, between two points or, on an open line, before the first
 * or after the last; on a closed line also between the last and the first.
 */
export function insertionIndex(points: readonly { at: Vec3 }[], current: number, at: Vec3, order: InsertOrder, closed: boolean): number {
  const n = points.length;
  if (order === "last" || n < 2) return current >= 0 && current < n ? current + 1 : n;
  const loop = closed && n >= 3;
  const detour = (i: number, j: number) => distance(points[i].at, at) + distance(at, points[j].at) - distance(points[i].at, points[j].at);
  let best = loop ? n : 0;
  let cost = loop ? detour(n - 1, 0) : distance(at, points[0].at);
  for (let i = 1; i < n; i++) {
    const c = detour(i - 1, i);
    if (c < cost) {
      cost = c;
      best = i;
    }
  }
  if (!loop && distance(points[n - 1].at, at) < cost) best = n;
  return best;
}

/** Adds a clicked point (the viewport's pick on the working copy) where the insert order puts it; it becomes current. */
export function addCutPoint(at: Vec3, triangle: number): void {
  if (!isCutting()) return;
  const s = useCut.getState();
  const index = insertionIndex(s.points, s.current, at, s.settings.order, s.settings.closed);
  const point: CutPoint = { id: nextId++, at, triangle };
  useCut.setState({ points: [...s.points.slice(0, index), point, ...s.points.slice(index)], current: index, added: [...s.added, point.id] });
  changed();
}

/** Undo last point: takes out the point added last; the one before it in the line becomes current. */
export function undoLastPoint(): void {
  const s = useCut.getState();
  const id = s.added[s.added.length - 1];
  if (id === undefined) return;
  const index = s.points.findIndex((p) => p.id === id);
  const points = s.points.filter((p) => p.id !== id);
  let current = s.current;
  if (index >= 0 && index <= current) current -= 1;
  if (current < 0 && points.length > 0) current = 0;
  useCut.setState({ points, current: points.length ? current : -1, added: s.added.slice(0, -1) });
  changed();
}

export function deleteAllPoints(): void {
  if (useCut.getState().points.length === 0) return;
  clearPoints();
}

/** Previous point / Next point: moves the current point along the line, round a closed one. */
export function stepCurrentPoint(by: -1 | 1): void {
  const { points, current, settings } = useCut.getState();
  const n = points.length;
  if (n === 0) return;
  const next = settings.closed && n >= 3 ? (current + by + n) % n : Math.min(n - 1, Math.max(0, current + by));
  if (next !== current) useCut.setState({ current: next });
}

export function setCutSettings(patch: Partial<CutSettings>): void {
  const before = useCut.getState().settings;
  const settings = { ...before, ...patch };
  useCut.setState({ settings });
  if (!sameRoute(before, settings) || (settings.auto && !before.auto)) changed();
}

/** After a change of the points or the settings: the line is routed again when Automatic update is on. */
function changed() {
  if (useCut.getState().settings.auto) void routeCut();
}

function lineOptions(s: CutSettings): CutLineOptions {
  return { smooth: s.smooth, step: s.step, snap: s.snap, closed: s.closed };
}

/** Update (and every change with Automatic update on): asks Rust for the line through the points as they are. */
export async function routeCut(): Promise<void> {
  const session = repair().session;
  if (!session || !isCutting()) return;
  const { points, settings } = useCut.getState();
  const seq = ++routeSeq;
  if (points.length < 2) {
    useCut.setState({ path: null, routedFor: { points, settings }, routing: false, routeError: null });
    return;
  }
  if (!isTauri()) {
    useCut.setState({ routing: false, routeError: "Cut surfaces needs the tricky3D desktop app." });
    return;
  }
  useCut.setState({ routing: true });
  try {
    const path = await fetchCutPath(session.mesh, points, lineOptions(settings));
    if (seq !== routeSeq || repair().session?.mesh !== session.mesh) return;
    useCut.setState({ path, routedFor: { points, settings }, routing: false, routeError: null });
  } catch (err) {
    if (seq !== routeSeq) return;
    useCut.setState({ routing: false, routeError: errorMessage(err, "Could not show the cut line") });
  }
}

/**
 * Execute cut: cuts the surface open along the line, in Rust, as one inner undo step with the status, so the cut
 * shows as holes. The points go once the working copy has changed; the cutting mode stays on for the next line.
 */
export async function executeCut(): Promise<void> {
  const { points, settings } = useCut.getState();
  if (!isCutting()) return;
  if (points.length < 2) {
    toast(TWO_POINTS_FIRST);
    return;
  }
  await runTool("cut_surface", cutParams(points, lineOptions(settings)), { withStatus: true, announce: true });
}

/** The pieces of the line shown that found no way along the surface, as "points 2 and 3"; empty when all did. */
export function brokenSpans(s: Pick<CutState, "path" | "points" | "settings" | "routedFor">): string[] {
  const path = s.path;
  if (!path || pathStale(s)) return [];
  return path.pieces.flatMap((piece, i) => (piece ? [] : [`points ${path.spans[i][0] + 1} and ${path.spans[i][1] + 1}`]));
}
