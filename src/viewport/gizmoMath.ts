import { Matrix4, OrthographicCamera, Plane, Vector3, type PerspectiveCamera, type Ray } from "three";
import { rotationAbout, translationMatrix, type Axis } from "../matrix";
import { centreOf, partBounds, unionBounds } from "../mesh";
import type { AppState, Part, TransformMode, Vec3 } from "../store";
import { formatValue } from "./rulerScale";

/** Undo-step names for a gizmo drag in each mode. */
export const DRAG_LABELS: Record<TransformMode, string> = { translate: "Move", rotate: "Rotate", scale: "Scale" };

/** Shift snapping: moves to 1 mm, rotations to 15°, scale factors to 5%. */
export const SNAP = { translate: 1, rotate: Math.PI / 12, scale: 0.05 } as const;

/** Selected parts the gizmo moves: visible and not locked. */
export function transformableParts(s: Pick<AppState, "parts" | "selection">): Part[] {
  const selected = new Set(s.selection);
  return s.parts.filter((p) => selected.has(p.id) && p.visible && !p.locked);
}

/** Centre of the parts' combined world box, where the gizmo sits. Null when there is nothing to move. */
export function pivotOf(parts: Part[]): Vec3 | null {
  const box = unionBounds(parts.map(partBounds));
  return box ? centreOf(box) : null;
}

const delta = new Matrix4();
const moved = new Matrix4();

/**
 * New part matrices for a group drag: delta = pivotNow × pivotStart⁻¹ applied on the left of each
 * part's start matrix, so the whole group moves, turns or scales about the pivot as one body.
 */
export function groupMatrices(
  pivotStartInverse: Matrix4,
  pivotNow: Matrix4,
  starts: ReadonlyMap<string, Matrix4>,
): Map<string, number[]> {
  delta.multiplyMatrices(pivotNow, pivotStartInverse);
  const out = new Map<string, number[]>();
  for (const [id, start] of starts) out.set(id, moved.multiplyMatrices(delta, start).toArray());
  return out;
}

// The combined move/rotate gizmo's drag maths. Everything is in world space: the gizmo stays aligned
// with the world axes, so a handle's line, plane or rotation axis is always a world axis through the
// centre. The drawing (gizmoHandles.ts) and the pointer handling (gizmo.ts) build on these.

/** A handle of the combined gizmo. A plane square is named by its normal: { kind: "plane", axis: 2 } is XY. */
export type Handle =
  | { kind: "axis"; axis: Axis }
  | { kind: "plane"; axis: Axis }
  | { kind: "rotate"; axis: Axis }
  | { kind: "free" };

/** True when two handles are the same one. */
export function sameHandle(a: Handle | null, b: Handle | null): boolean {
  if (!a || !b) return a === b;
  if (a.kind === "free" || b.kind === "free") return a.kind === b.kind;
  return a.kind === b.kind && a.axis === b.axis;
}

/** The other two axes in cyclic order (X → [Y, Z], Y → [Z, X], Z → [X, Y]), so u × v = axis. */
export function planeAxes(axis: Axis): [Axis, Axis] {
  return [((axis + 1) % 3) as Axis, ((axis + 2) % 3) as Axis];
}

/**
 * +1 or −1 per world axis: the side of the gizmo centre the camera is on, which every handle is turned
 * or mirrored to. `toCamera` points from the centre to the camera. A component within a thousandth of its
 * length counts as +1 (0 does too): the Top and Bottom presets sit 1e-4 rad off the pole, on the front
 * side (POLE_EPS), and that tilt must not swing the Y arrow round, or the face-on square and arc out from
 * between the X and Y arrows.
 */
export function facingSigns(toCamera: Vector3): [number, number, number] {
  const tiny = 1e-3 * toCamera.length();
  const side = (c: number) => (c < 0 && -c >= tiny ? -1 : 1);
  return [side(toCamera.x), side(toCamera.y), side(toCamera.z)];
}

/** |cos| between the line of sight and an arrow above which the arrow points at the viewer (within ~16°) and hides: it would draw as a dot. */
export const END_ON = 0.96;
/**
 * |cos| between the line of sight and a square's normal or an arc's axis below which the square or arc
 * is seen edge-on (within ~12°) and hides: it would draw as a sliver, and the pointer ray would meet its
 * plane at so shallow an angle that a pixel of travel swept the hit a long way across it.
 */
export const EDGE_ON = 0.2;

/** False for an arrow pointing at the viewer and a square or arc seen edge-on; the centre ball always shows. `viewDir` is a unit vector from the camera towards the centre. */
export function handleVisible(handle: Handle, viewDir: Vector3): boolean {
  if (handle.kind === "free") return true;
  const cos = Math.abs(viewDir.getComponent(handle.axis));
  return handle.kind === "axis" ? cos <= END_ON : cos >= EDGE_ON;
}

/**
 * World mm per CSS px at `point`'s depth: perspective grows with depth, orthographic follows zoom.
 * Reads the camera's matrixWorld and projection settings as they stand (the caller keeps them current,
 * as rendering does). Perspective depth is measured along the line of sight, not as the distance to the
 * point, so every point in a plane square to the view has the same scale, like the picture itself; a
 * point at or behind the camera counts as on the near plane, so the result stays positive.
 * `viewHeightPx` is the canvas height and must be above 0.
 */
export function worldPerPixel(camera: PerspectiveCamera | OrthographicCamera, point: Vector3, viewHeightPx: number): number {
  if (camera instanceof OrthographicCamera) return (camera.top - camera.bottom) / camera.zoom / viewHeightPx;
  // The camera looks down its local −Z: forward is minus matrixWorld's third column, its position the fourth.
  const e = camera.matrixWorld.elements;
  const back = Math.hypot(e[8], e[9], e[10]) || 1;
  const depth = -((point.x - e[12]) * e[8] + (point.y - e[13]) * e[9] + (point.z - e[14]) * e[10]) / back;
  const viewHeight = 2 * Math.max(depth, camera.near) * Math.tan((camera.fov * Math.PI) / 360);
  return viewHeight / camera.zoom / viewHeightPx;
}

const eye = new Vector3();

/**
 * Unit vector from the camera towards `at`; an orthographic camera's line of sight is its forward axis
 * everywhere. It decides which handles hide (handleVisible) and which side they face (facingSigns of its
 * reverse). Reads the camera's matrixWorld, which the caller keeps current. Not the plane the centre ball
 * moves in: that is square to the forward axis (startDrag), which a perspective camera's line of sight
 * to an off-centre selection is not.
 */
export function lineOfSight(camera: PerspectiveCamera | OrthographicCamera, at: Vector3, out: Vector3): Vector3 {
  if (camera instanceof OrthographicCamera) return camera.getWorldDirection(out);
  return out.subVectors(at, eye.setFromMatrixPosition(camera.matrixWorld)).normalize();
}

/** A running drag. Mutable: dragStep keeps the unwrapped turn and the last good result in it. */
export interface GizmoDrag {
  readonly handle: Handle;
  /** The gizmo centre at the press (a copy): moves offset the selection from it, turns pivot on it. */
  readonly centre: Vector3;
}

/** What a drag step does to the selection. */
export interface DragResult {
  /** World offset, mm; [0, 0, 0] for a turn. */
  offset: Vec3;
  /** Turn in radians about the handle's axis, counter-clockwise looking from + down the axis (like rotationAbout); 0 for moves. */
  angle: number;
  /** Column-major world transform for the step (translationMatrix(offset) or rotationAbout(axis, angle, centre)), to put in front of each part's start matrix with applyDelta. */
  delta: number[];
}

/** How a drag reads the pointer, settled at the press. */
type Motion =
  /** Along a world axis: `t0` is how far along the axis (mm from the centre) the press ray came closest to it. */
  | { how: "line"; axis: Axis; t0: number }
  /**
   * In a plane through the centre: a square's, or the screen plane (centre ball). `from` is the press ray's
   * hit. `normal` is the world axis the plane is square to, whose offset stays exactly 0: a square's axis,
   * or the axis an axis view's screen plane was squared to; null for a screen plane at a slant.
   */
  | { how: "plane"; plane: Plane; from: Vector3; normal: Axis | null }
  /**
   * Turning by the pointer ray's hit on the rotation plane, which keeps the grabbed ball under the cursor;
   * `raw` is the last hit's angle, before unwrapping. Hits within `minRadius` of the axis have no angle and are held.
   */
  | { how: "turn"; axis: Axis; plane: Plane; raw: number; minRadius: number };

/** startDrag's working state behind GizmoDrag; only this module reads it. */
interface DragState extends GizmoDrag {
  readonly motion: Motion;
  /**
   * The last good measurement, unsnapped: world offset for a move, the unwrapped turn in radians for an
   * arc. A step whose ray misses leaves it as it is, so the last result stands (and still follows Shift).
   */
  readonly offset: Vec3;
  turned: number;
}

/**
 * Below this, sin² of the angle between a ray and a handle's line, or cos² of the angle between a ray and
 * a plane's normal, counts as parallel (within ~0.002°): the hit would be far off or undefined.
 */
const PARALLEL = 1e-9;

/** cos of 1e-3 rad: a forward axis this close to a world axis looks straight down it. */
const ON_AXIS = Math.cos(1e-3);

const hitPoint = new Vector3();
const toCentre = new Vector3();

const unitAxis = (axis: Axis) => new Vector3().setComponent(axis, 1);

/**
 * Starts dragging `handle` of the gizmo at `centre`. `forward` is the camera's unit forward axis: the screen plane's
 * normal for the centre ball. Null when the ray runs along the handle's line or plane.
 */
export function startDrag(handle: Handle, centre: Vector3, ray: Ray, forward: Vector3): GizmoDrag | null {
  const at = centre.clone();
  const motion = startMotion(handle, at, ray, forward);
  if (!motion) return null;
  const drag: DragState = { handle, centre: at, motion, offset: [0, 0, 0], turned: 0 };
  return drag;
}

function startMotion(handle: Handle, centre: Vector3, ray: Ray, forward: Vector3): Motion | null {
  switch (handle.kind) {
    case "axis": {
      const t0 = alongAxis(ray, centre, handle.axis);
      return t0 === null ? null : { how: "line", axis: handle.axis, t0 };
    }
    case "plane":
    case "free": {
      const normal = handle.kind === "plane" ? handle.axis : squareTo(forward);
      const n = normal === null ? forward.clone().normalize() : unitAxis(normal);
      const plane = new Plane().setFromNormalAndCoplanarPoint(n, centre);
      const from = hitPlane(ray, plane, new Vector3());
      return from && { how: "plane", plane, from, normal };
    }
    case "rotate": {
      const { axis } = handle;
      const plane = new Plane().setFromNormalAndCoplanarPoint(unitAxis(axis), centre);
      const w = hitPlane(ray, plane, hitPoint)?.sub(centre);
      const raw = w ? angleAbout(axis, w, 0) : null;
      // A ray through the centre hits it only to rounding (~1e-13 of the coordinates), and that error's
      // direction is no angle. So later hits within a billionth of the press's radius of the axis are held:
      // far below any radius the arcs are drawn at, far above that rounding.
      return w && raw !== null ? { how: "turn", axis, plane, raw, minRadius: 1e-9 * w.length() } : null;
    }
  }
}

/**
 * The world axis a forward axis within 1e-3 rad of looks down, or null. The centre ball moves in the
 * screen plane: square to the camera's forward axis, not to the line of sight to the centre, so in
 * perspective an off-centre selection keeps its depth in the view instead of sliding along it. In an axis
 * view that plane is taken as exactly square to the axis, and the move along it as exactly 0: the Top and
 * Bottom presets sit 1e-4 rad off the pole (POLE_EPS), which would otherwise drift a part resting on Z = 0
 * by a ten-thousandth of the travel.
 */
function squareTo(forward: Vector3): Axis | null {
  const length = forward.length();
  for (const axis of [0, 1, 2] as const) if (Math.abs(forward.getComponent(axis)) > ON_AXIS * length) return axis;
  return null;
}

/** The drag's result for the pointer ray now; with `snap` offsets round to SNAP.translate per world axis and turns to SNAP.rotate. Keeps the last result when the ray misses. */
export function dragStep(drag: GizmoDrag, ray: Ray, snap: boolean): DragResult {
  const d = drag as DragState;
  const m = d.motion;
  switch (m.how) {
    case "line": {
      const t = alongAxis(ray, d.centre, m.axis);
      // The other two components were set to 0 at the start and never change.
      if (t !== null) d.offset[m.axis] = t - m.t0;
      return moveResult(d.offset, snap);
    }
    case "plane": {
      const hit = hitPlane(ray, m.plane, hitPoint);
      if (hit) {
        hit.sub(m.from).toArray(d.offset);
        // The hit lies on the plane only to rounding; a square, or the centre ball in an axis view, moves strictly within it.
        if (m.normal !== null) d.offset[m.normal] = 0;
      }
      return moveResult(d.offset, snap);
    }
    case "turn": {
      const hit = hitPlane(ray, m.plane, hitPoint);
      const raw = hit && angleAbout(m.axis, hit.sub(d.centre), m.minRadius);
      if (raw !== null) {
        // Each step adds the change since the last one, taken the short way round, so a drag past 180°
        // keeps counting (to +190°, not −170°) and a full circle comes back at 360°.
        d.turned += wrap(raw - m.raw);
        m.raw = raw;
      }
      return turnResult(m.axis, d.centre, d.turned, snap);
    }
  }
}

/** −0 as +0 (−0 + 0 is +0): a snapped −0.3 mm must read back as 0, not −0. */
const tidy = (v: number) => v + 0;

/** Snapping rounds the move from the press, not the position, so a part resting on Z = 0 stays there when moved with Shift. */
function moveResult(raw: Vec3, snap: boolean): DragResult {
  const offset = raw.map((v) => tidy(snap ? Math.round(v / SNAP.translate) * SNAP.translate : v)) as Vec3;
  return { offset, angle: 0, delta: translationMatrix(offset) };
}

function turnResult(axis: Axis, centre: Vector3, turned: number, snap: boolean): DragResult {
  const angle = tidy(snap ? Math.round(turned / SNAP.rotate) * SNAP.rotate : turned);
  return { offset: [0, 0, 0], angle, delta: rotationAbout(axis, angle, [centre.x, centre.y, centre.z]) };
}

/**
 * How far along the line centre + t·e_axis (t, mm from the centre) `ray` comes closest to it. Minimising
 * |w + t·e − s·d|² over t and s, with w = centre − origin, d the ray direction, b = e·d and dd = d·d (e is
 * a unit world axis): ∂/∂t gives e·w + t − s·b = 0 and ∂/∂s gives d·w + t·b − s·dd = 0, so
 * t = (b·(d·w) − dd·(e·w)) / (dd − b²) and s = (d·w + t·b) / dd. dd − b² is dd·sin² of the angle between
 * ray and line. Null when they run parallel, or when the closest point lies behind the ray's start: the
 * pointer has passed the axis's vanishing point, and t would jump to the far end of the axis.
 */
function alongAxis(ray: Ray, centre: Vector3, axis: Axis): number | null {
  const d = ray.direction;
  const w = toCentre.subVectors(centre, ray.origin);
  const b = d.getComponent(axis);
  const dd = d.lengthSq();
  const den = dd - b * b;
  if (!(den >= PARALLEL * dd)) return null;
  const dw = d.dot(w);
  const t = (b * dw - dd * w.getComponent(axis)) / den;
  return (dw + t * b) / dd >= 0 ? t : null;
}

/** Ray.intersectPlane (null when the plane is behind the ray), but a ray within PARALLEL of the plane misses rather than hitting far off. */
function hitPlane(ray: Ray, plane: Plane, out: Vector3): Vector3 | null {
  const cos = ray.direction.dot(plane.normal);
  if (!(cos * cos >= PARALLEL * ray.direction.lengthSq())) return null;
  return ray.intersectPlane(plane, out);
}

/**
 * Angle of `w` about the world axis, from the first of its planeAxes towards the second: counter-clockwise
 * looking from + down the axis, the sense rotationAbout turns. Null within `minRadius` of the axis, where
 * there is no angle to read.
 */
function angleAbout(axis: Axis, w: Vector3, minRadius: number): number | null {
  const [u, v] = planeAxes(axis);
  const wu = w.getComponent(u);
  const wv = w.getComponent(v);
  return Math.hypot(wu, wv) > minRadius ? Math.atan2(wv, wu) : null;
}

/** `a` moved by a whole turn into (−π, π]. Both angles a step compares are atan2 results in [−π, π], so one turn is always enough. */
function wrap(a: number): number {
  if (a > Math.PI) return a - 2 * Math.PI;
  if (a <= -Math.PI) return a + 2 * Math.PI;
  return a;
}

const AXIS_NAMES = ["X", "Y", "Z"] as const;

/** One decimal, trailing ".0" dropped, a true minus, and "+" on anything that does not read as 0: "+12.5", "−3", "0". */
function signed(value: number): string {
  const text = formatValue(value, 1);
  return text === "0" || text.startsWith("−") ? text : `+${text}`;
}

/**
 * Text by the cursor during a drag: the dragged axis, a square's two axes, or all three for the centre ball,
 * in mm ("X +3  Y −2 mm"); an arc's turn in degrees, counting on past a full turn ("Z +390°").
 */
export function dragLabel(handle: Handle, result: DragResult): string {
  if (handle.kind === "rotate") return `${AXIS_NAMES[handle.axis]} ${signed((result.angle * 180) / Math.PI)}°`;
  const axes: Axis[] = handle.kind === "axis" ? [handle.axis] : [0, 1, 2];
  // A square's normal is the axis it does not move along; the other two stay in X, Y, Z order.
  const shown = handle.kind === "plane" ? axes.filter((a) => a !== handle.axis) : axes;
  return `${shown.map((a) => `${AXIS_NAMES[a]} ${signed(result.offset[a])}`).join("  ")} mm`;
}
