import { Matrix4, Object3D, Raycaster, Sphere, Vector2, Vector3, type Ray, type Scene } from "three";
import { TransformControls } from "three/examples/jsm/controls/TransformControls.js";
import type { History, Snapshot } from "../history";
import { applyDelta } from "../matrix";
import { shownGizmo, useStore, type AppState, type GizmoMode, type Part, type TransformMode } from "../store";
import { finishScale, openTransformDialog } from "../transformTools";
import { rayFromCamera, type ViewCamera } from "./cameras";
import type { ViewControls } from "./controls";
import { createGizmoHandles, GIZMO_PX, type ArcTurn } from "./gizmoHandles";
import {
  DRAG_LABELS,
  dragLabel,
  dragStep,
  facingSigns,
  groupMatrices,
  lineOfSight,
  pivotOf,
  sameHandle,
  SNAP,
  startDrag,
  transformableParts,
  worldPerPixel,
  type GizmoDrag,
  type Handle,
} from "./gizmoMath";
import { readTheme, watchColorScheme } from "./theme";
import "./gizmo.css";

export interface GizmoOptions {
  scene: Scene;
  camera: ViewCamera;
  canvas: HTMLCanvasElement;
  /** The viewport element around the canvas; the live value by the cursor is placed in it. */
  host: HTMLElement;
  orbit: ViewControls;
  requestRender(): void;
}

export interface Gizmo {
  /** Follows the store: shownGizmo(s) on the selection's unlocked, visible parts. */
  sync(s: AppState): void;
  /** The projection changed. */
  setCamera(camera: ViewCamera): void;
  /**
   * Lays the combined gizmo out for the frame about to be drawn (constant screen size, every handle on the camera's side).
   * Returns the world sphere the shown gizmo reaches over, or null when none shows: zoomed far out it is far
   * bigger than the scene, and the clip planes must take it in.
   */
  beforeRender(width: number, height: number): Sphere | null;
  /** True while the pointer is over a handle or a drag runs; viewport clicks are not selections then. */
  isActive(): boolean;
  dispose(): void;
}

/** Pointer travel (px) under which a press on a handle stays a click and moves nothing; as in picking. */
const CLICK_SLOP = 4;
/** Smallest scale factor a drag reaches, so parts never collapse flat or turn inside out. */
const MIN_SCALE = 0.01;
/** Where the live value sits from the pointer, CSS px: right of and below it, clear of the cursor arrow. */
const VALUE_OFFSET = { x: 14, y: 18 };
/** Space the live value keeps from the viewport's edges, CSS px. */
const VALUE_MARGIN = 4;
/** How far the move and rotate gizmo reaches from its centre, in arrow lengths: past the tips and their pickers. */
const COMBINED_REACH = 1.1;
/**
 * How far TransformControls' scale gizmo reaches from its centre, as a share of the view's height: it sizes
 * itself to the view rather than in px, and its handles end about 0.15 of the height out.
 */
const SCALE_REACH = 0.2;

/** A pointer aimed into the view: its world ray (built with rayFromCamera) and canvas position, with the canvas size it was measured on (CSS px, y down). */
interface Aim {
  ray: Ray;
  x: number;
  y: number;
  width: number;
  height: number;
}

/** A drag on the scale gizmo (TransformControls). */
interface ScaleDrag {
  before: Snapshot;
  /** History when the drag began. Any other change to it (undo, delete...) cancels the drag. */
  history: History;
  pivotStartInverse: Matrix4;
  starts: Map<string, Matrix4>;
  /** False until the pointer leaves the click slop, so a click or double-click never nudges parts. */
  moved: boolean;
}

/** A drag on a handle of the move and rotate gizmo. */
interface HandleDrag {
  handle: Handle;
  pointerId: number;
  before: Snapshot;
  /** As ScaleDrag.history. */
  history: History;
  /** The parts at the press: every step puts its delta in front of these matrices, so steps never add up rounding. */
  starts: Part[];
  /** The running drag maths; null when the handle cannot be dragged from this view (the press ray runs along its line or plane). */
  math: GizmoDrag | null;
  /** Line of sight and camera-facing octant at the press: the handles keep that layout until the release. */
  viewDir: Vector3;
  signs: [number, number, number];
  /** Where the gizmo is drawn: the press centre, plus the offset so far for a move. A turn pivots on the centre. */
  at: Vector3;
  /** An arc's turn so far (the last step's angle, snapped like the parts), which the grabbed arc follows; null for a move. */
  turn: ArcTurn | null;
  /** As ScaleDrag.moved. */
  moved: boolean;
}

/**
 * The selection's gizmo, in one of two modes (shownGizmo). The move and rotate gizmo is our own: the
 * handles come from gizmoHandles.ts, the drag maths from gizmoMath.ts, and this file does the pointer
 * work. The scale gizmo is TransformControls driving an invisible pivot at the centre of the selection's
 * combined box. Neither touches the part meshes: each drag step sets the part matrices as a live preview
 * (setPartMatrices with no label) and the release records one undo step. Both stay world-aligned and
 * re-centre on the selection's new box after every drag.
 */
export function createGizmo({ scene, camera: initialCamera, canvas, host, orbit, requestRender }: GizmoOptions): Gizmo {
  let camera = initialCamera;

  // Scale: TransformControls, mode fixed.
  const pivot = new Object3D();
  const controls = new TransformControls(camera, canvas);
  // Its orthographic rays would start on the camera plane, which zoomed in close lies inside the scene:
  // the handles of a part nearer than the camera would be drawn but could not be grabbed.
  const controlsRaycaster = controls.getRaycaster();
  const setFromCamera: typeof controlsRaycaster.setFromCamera = (coords, cam) => rayFromCamera(controlsRaycaster, coords, cam);
  controlsRaycaster.setFromCamera = setFromCamera;
  controls.setSpace("world");
  controls.setMode("scale");
  const helper = controls.getHelper();

  // Move and rotate: own handles, picked and dragged with the same rays as part picking.
  const handles = createGizmoHandles(readTheme());
  const raycaster = new Raycaster();
  const ndc = new Vector2();
  const value = document.createElement("div");
  value.className = "viewport-gizmo-value";
  value.setAttribute("aria-hidden", "true");
  value.hidden = true;
  host.append(value);
  scene.add(pivot, helper, handles.group);

  /** The gizmo on screen; null while there is no tool or nothing to move. */
  let shown: GizmoMode | null = null;
  /** The move and rotate gizmo's centre at rest: the centre of the selection's combined box. */
  const centre = new Vector3();
  let hovered: Handle | null = null;
  let handleDrag: HandleDrag | null = null;
  let scaleDrag: ScaleDrag | null = null;
  let snapping = false;
  /** The dialog a double-click opens: set by a press on a handle, cleared by a press anywhere else. */
  let pressed: TransformMode | null = null;
  /**
   * The last two presses both landed on handles of `pressed`'s dialog. A double-click opens it only then: a first
   * click that selects a part brings the gizmo up under the pointer, and the second must not open Move from there.
   */
  let pressedTwice = false;
  let cursor = "";
  /** The pointer is over the canvas with no button held, so what is under it is lit. */
  let hovering = false;
  const press = { x: 0, y: 0 };
  const pointer = { x: 0, y: 0 };
  const sight = new Vector3();
  const toCamera = new Vector3();
  const offset = new Vector3();
  const reach = new Sphere();

  /** Shows the gizmo the store asks for on the selection, or hides both. Never runs mid-drag. */
  function apply(s: AppState) {
    const mode = shownGizmo(s);
    const at = mode ? pivotOf(transformableParts(s)) : null;
    shown = at ? mode : null;
    if (at && shown === "scale") {
      pivot.position.fromArray(at);
      pivot.quaternion.identity();
      pivot.scale.setScalar(1);
      pivot.updateMatrix();
      if (controls.object !== pivot) controls.attach(pivot);
    } else if (controls.object) {
      controls.detach();
    }
    handles.group.visible = shown === "combined";
    if (at && shown === "combined") centre.fromArray(at);
    else setHover(null);
  }

  function updateCursor() {
    const next = controls.dragging || handleDrag ? "grabbing" : controls.axis !== null || hovered ? "grab" : "";
    if (next === cursor) return;
    cursor = next;
    canvas.style.cursor = next;
  }

  function setHover(next: Handle | null) {
    if (sameHandle(next, hovered)) return;
    hovered = next;
    handles.highlight(hovered, handleDrag?.handle ?? null);
    updateCursor();
    requestRender();
  }

  /**
   * Lays the move and rotate gizmo out for the camera as it is now: GIZMO_PX per arrow on a canvas
   * `heightPx` high, every handle towards the viewer. While a drag runs the layout stays as the press
   * saw it (the handle under the pointer must not swap sides or vanish), and only follows a move, or
   * turns the grabbed arc with a rotation. Returns the world mm per arrow length, 0 when the gizmo is not shown.
   */
  function layout(heightPx: number): number {
    if (shown !== "combined" || !(heightPx > 0)) return 0;
    camera.updateMatrixWorld();
    const at = handleDrag ? handleDrag.at : centre;
    const viewDir = handleDrag ? handleDrag.viewDir : lineOfSight(camera, at, sight);
    const signs = handleDrag ? handleDrag.signs : facingSigns(toCamera.copy(viewDir).negate());
    const scale = worldPerPixel(camera, at, heightPx) * GIZMO_PX;
    handles.layout(at, scale, viewDir, signs, handleDrag?.turn);
    return scale;
  }

  /** Points the shared ray through a client point; returns that point in canvas CSS px. Null while the canvas has no size. */
  function aim(clientX: number, clientY: number): Aim | null {
    const rect = canvas.getBoundingClientRect();
    if (!rect.width || !rect.height) return null;
    const x = clientX - rect.left;
    const y = clientY - rect.top;
    ndc.set((x / rect.width) * 2 - 1, -(y / rect.height) * 2 + 1);
    camera.updateMatrixWorld();
    rayFromCamera(raycaster, ndc, camera);
    return { ray: raycaster.ray, x, y, width: rect.width, height: rect.height };
  }

  /** The handle under an aimed pointer. The layout is redone first: the camera may have moved since the last frame. */
  function pick(p: Aim | null) {
    if (shown !== "combined" || !p || p.x < 0 || p.y < 0 || p.x > p.width || p.y > p.height) return null;
    layout(p.height);
    return handles.pick(raycaster);
  }

  /** A left press while the move and rotate gizmo shows: starts a drag when it lands on a handle. */
  function pressHandle(e: PointerEvent) {
    const p = aim(e.clientX, e.clientY);
    const hit = pick(p);
    setHover(hit?.handle ?? null);
    if (!hit || !p) return;
    pressed = hit.handle.kind === "rotate" ? "rotate" : "translate";
    const s = useStore.getState();
    // aim() brought the camera's matrixWorld up to date.
    const viewDir = lineOfSight(camera, centre, new Vector3());
    canvas.setPointerCapture(e.pointerId);
    orbit.enabled = false;
    handleDrag = {
      handle: hit.handle,
      pointerId: e.pointerId,
      before: s.snapshot(),
      history: s.history,
      starts: transformableParts(s),
      // The centre ball moves in the screen plane, square to the camera's forward axis rather than to the line of sight.
      math: startDrag(hit.handle, centre, p.ray, camera.getWorldDirection(new Vector3())),
      viewDir,
      signs: facingSigns(toCamera.copy(viewDir).negate()),
      at: centre.clone(),
      turn: hit.handle.kind === "rotate" ? { axis: hit.handle.axis, angle: 0 } : null,
      moved: false,
    };
    handles.highlight(hit.handle, hit.handle);
    updateCursor();
    requestRender();
  }

  /** One drag step for the last pointer position and Shift: the parts move as a live preview, the value follows the cursor. */
  function stepHandleDrag() {
    const d = handleDrag;
    if (!d) return;
    // Some edits change the parts and the history in two store updates, and sync() only sees the first.
    if (useStore.getState().history !== d.history) {
      endHandleDrag();
      apply(useStore.getState());
      return;
    }
    const p = d.math && aim(pointer.x, pointer.y);
    if (!d.math || !p) return;
    const result = dragStep(d.math, p.ray, snapping);
    d.at.copy(d.math.centre).add(offset.fromArray(result.offset));
    if (d.turn) d.turn.angle = result.angle;
    useStore.getState().setPartMatrices(applyDelta(d.starts, result.delta), null);
    showValue(dragLabel(d.handle, result));
    requestRender();
  }

  /** Ends a handle drag without an undo step; the caller puts the gizmo back. */
  function endHandleDrag() {
    handleDrag = null;
    orbit.enabled = true;
    value.hidden = true;
    handles.highlight(hovered, null);
    updateCursor();
    requestRender();
  }

  /** Ends a handle drag as a release: its previews become one undo step, and the gizmo re-centres on the selection. */
  function releaseHandleDrag() {
    const done = handleDrag;
    if (!done) return;
    endHandleDrag();
    const s = useStore.getState();
    // recordChange does nothing when the parts never moved (a click on a handle). After an undo or
    // another edit mid-drag the store has moved on, and `before` no longer describes this step.
    if (s.history === done.history) s.recordChange(done.handle.kind === "rotate" ? DRAG_LABELS.rotate : DRAG_LABELS.translate, done.before);
    apply(useStore.getState());
    // The gizmo moved, or the turned arc went back to rest away from the pointer: light what is under the pointer now.
    setHover(pick(aim(pointer.x, pointer.y))?.handle ?? null);
  }

  /** The live value, beside the pointer and kept inside the viewport: flipped to the pointer's other side near an edge. */
  function showValue(text: string) {
    value.textContent = text;
    value.hidden = false;
    const box = host.getBoundingClientRect();
    const x = besidePointer(pointer.x - box.left, VALUE_OFFSET.x, value.offsetWidth, box.width);
    const y = besidePointer(pointer.y - box.top, VALUE_OFFSET.y, value.offsetHeight, box.height);
    value.style.transform = `translate(${x}px, ${y}px)`;
  }

  function beginScaleDrag() {
    orbit.enabled = false;
    const s = useStore.getState();
    pivot.updateMatrix();
    scaleDrag = {
      before: s.snapshot(),
      history: s.history,
      pivotStartInverse: pivot.matrix.clone().invert(),
      starts: new Map(transformableParts(s).map((p) => [p.id, new Matrix4().fromArray(p.matrix)])),
      moved: false,
    };
  }

  function endScaleDrag() {
    orbit.enabled = true;
    const done = scaleDrag;
    scaleDrag = null;
    const s = useStore.getState();
    // As releaseHandleDrag. A scale that made a step ends scale mode; a click on a handle keeps it.
    if (done && s.history === done.history) {
      s.recordChange(DRAG_LABELS.scale, done.before);
      if (useStore.getState().history !== done.history) finishScale();
    }
    apply(useStore.getState());
    requestRender();
  }

  /** Ends the scale drag without an undo step; endScaleDrag (through dragging-changed) puts the gizmo back. */
  function cancelScaleDrag() {
    scaleDrag = null;
    controls.pointerUp(null);
  }

  function onObjectChange() {
    if (!scaleDrag) return;
    // Some edits change the parts and the history in two store updates, and sync() only sees the first.
    if (useStore.getState().history !== scaleDrag.history) {
      cancelScaleDrag();
      return;
    }
    if (!scaleDrag.moved) {
      if (Math.hypot(pointer.x - press.x, pointer.y - press.y) < CLICK_SLOP) {
        pivot.scale.setScalar(1);
        return;
      }
      scaleDrag.moved = true;
    }
    pivot.scale.set(Math.max(pivot.scale.x, MIN_SCALE), Math.max(pivot.scale.y, MIN_SCALE), Math.max(pivot.scale.z, MIN_SCALE));
    pivot.updateMatrix();
    useStore.getState().setPartMatrices(groupMatrices(scaleDrag.pivotStartInverse, pivot.matrix, scaleDrag.starts), null);
  }

  function setSnap(on: boolean) {
    if (on === snapping) return;
    snapping = on;
    controls.setScaleSnap(on ? SNAP.scale : null);
    // Re-runs the drag on the last pointer so the new snap shows without moving the mouse.
    if (scaleDrag?.moved) controls.pointerMove(null);
    if (handleDrag?.moved) stepHandleDrag();
  }

  function applyTheme() {
    const t = readTheme();
    controls.setColors(t.axisX, t.axisY, t.axisZ, t.accent);
    handles.applyTheme(t);
  }

  const onDraggingChanged = (e: { value: unknown }) => {
    if (e.value === true) beginScaleDrag();
    else endScaleDrag();
    updateCursor();
  };
  // Added after the scale controls' own listener, so a press on a scale handle has already started its
  // drag here. Picking's listener comes after this one, so a press on any handle is claimed before it looks.
  const onPointerDown = (e: PointerEvent) => {
    // A second finger or pen while a handle drag runs is not a new press.
    if (handleDrag) return;
    press.x = pointer.x = e.clientX;
    press.y = pointer.y = e.clientY;
    setSnap(e.shiftKey);
    const previous = pressed;
    pressed = e.button === 0 && controls.dragging ? "scale" : null;
    if (e.button === 0 && shown === "combined") {
      if (e.ctrlKey || e.metaKey) {
        // A selection click: Ctrl/Cmd toggles the part under the pointer, and Control-click opens the part menu on
        // macOS (picking). Held over a handle it means nothing to a drag, where only Shift snaps. The light goes out
        // first, or isActive() would claim the press, and stays out while the button is held.
        hovering = false;
        setHover(null);
      } else {
        pressHandle(e);
      }
    }
    pressedTwice = pressed !== null && pressed === previous;
  };
  // For the scale controls, the drag's pointermove listener is added on each press, after this one, so it sees `pointer` current.
  const onPointerMove = (e: PointerEvent) => {
    if (handleDrag && e.pointerId !== handleDrag.pointerId) return;
    pointer.x = e.clientX;
    pointer.y = e.clientY;
    setSnap(e.shiftKey);
    if (!handleDrag) {
      // No lit handles while the view is orbited or panned under a held button.
      hovering = e.buttons === 0;
      setHover(hovering ? (pick(aim(e.clientX, e.clientY))?.handle ?? null) : null);
      return;
    }
    if (!handleDrag.moved) {
      if (Math.hypot(pointer.x - press.x, pointer.y - press.y) < CLICK_SLOP) return;
      handleDrag.moved = true;
    }
    stepHandleDrag();
  };
  const onPointerUp = (e: PointerEvent) => {
    if (handleDrag?.pointerId !== e.pointerId) return;
    pointer.x = e.clientX;
    pointer.y = e.clientY;
    releaseHandleDrag();
  };
  // A highlighted handle would otherwise stay lit (and claim the next click) after the pointer leaves.
  const onPointerLeave = () => {
    if (!controls.dragging && controls.axis !== null) controls.axis = null;
    hovering = false;
    if (!handleDrag) setHover(null);
  };
  const onDoubleClick = (e: MouseEvent) => {
    // Same checks and messages as the ribbon buttons.
    if (e.button === 0 && pressedTwice && pressed && shown) openTransformDialog(pressed);
  };
  // The browser can end a pointer without a pointerup (pen or touch gestures, the OS taking the
  // pointer). Finish the drag here as a release, which keeps the move as one undo step. Deferred a
  // microtask because a normal release also loses the capture, and an engine may report that inside
  // the scale controls' own pointerup, before the drag ends.
  const onPointerLost = (e: PointerEvent) => {
    queueMicrotask(() => {
      if (controls.dragging) controls.pointerUp(null);
      if (handleDrag?.pointerId === e.pointerId) releaseHandleDrag();
    });
  };
  const onKey = (e: KeyboardEvent) => setSnap(e.shiftKey);
  const onBlur = () => setSnap(false);
  // Esc steps back a tool or clears the selection (keyboard.ts). Under a running drag that would deselect the parts
  // being dragged, which follow the pointer all the same and get the release's undo step, so Esc means nothing until
  // the release. Capture phase on window, so this runs before any control or that bubble listener sees the key.
  const onEscape = (e: KeyboardEvent) => {
    if (e.key !== "Escape" || !(handleDrag || scaleDrag)) return;
    e.preventDefault();
    e.stopImmediatePropagation();
  };

  controls.addEventListener("change", requestRender);
  controls.addEventListener("objectChange", onObjectChange);
  controls.addEventListener("dragging-changed", onDraggingChanged);
  controls.addEventListener("axis-changed", updateCursor);
  canvas.addEventListener("pointerdown", onPointerDown);
  canvas.addEventListener("pointermove", onPointerMove);
  canvas.addEventListener("pointerup", onPointerUp);
  canvas.addEventListener("pointerleave", onPointerLeave);
  canvas.addEventListener("pointercancel", onPointerLost);
  canvas.addEventListener("lostpointercapture", onPointerLost);
  canvas.addEventListener("dblclick", onDoubleClick);
  window.addEventListener("keydown", onEscape, true);
  window.addEventListener("keydown", onKey);
  window.addEventListener("keyup", onKey);
  window.addEventListener("blur", onBlur);
  // The engine is still being built here and cannot render yet; its own theme listener requests the frame.
  const unwatchTheme = watchColorScheme(applyTheme);
  applyTheme();

  return {
    sync(s) {
      const drag = handleDrag ?? scaleDrag;
      if (drag) {
        // The drag's own previews (and selection changes) leave the history alone: finish the drag first.
        if (s.history === drag.history) return;
        // Undo, redo or another edit mid-drag: the store has moved on, so the drag ends without a step.
        if (handleDrag) endHandleDrag();
        else cancelScaleDrag();
      }
      apply(s);
    },

    setCamera(next) {
      camera = next;
      controls.camera = next;
    },

    beforeRender(_width, height) {
      // The view can move under a still pointer (wheel zoom, a camera animation): keep lit what is under it.
      if (hovering && !handleDrag) setHover(pick(aim(pointer.x, pointer.y))?.handle ?? null);
      if (shown === "combined") return reach.set(handleDrag?.at ?? centre, layout(height) * COMBINED_REACH);
      if (shown !== "scale") return null;
      camera.updateMatrixWorld();
      return reach.set(pivot.position, worldPerPixel(camera, pivot.position, height) * height * SCALE_REACH);
    },

    isActive() {
      return controls.dragging || controls.axis !== null || handleDrag !== null || hovered !== null;
    },

    dispose() {
      unwatchTheme();
      window.removeEventListener("keydown", onEscape, true);
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("keyup", onKey);
      window.removeEventListener("blur", onBlur);
      canvas.removeEventListener("pointerdown", onPointerDown);
      canvas.removeEventListener("pointermove", onPointerMove);
      canvas.removeEventListener("pointerup", onPointerUp);
      canvas.removeEventListener("pointerleave", onPointerLeave);
      canvas.removeEventListener("pointercancel", onPointerLost);
      canvas.removeEventListener("lostpointercapture", onPointerLost);
      canvas.removeEventListener("dblclick", onDoubleClick);
      controls.removeEventListener("change", requestRender);
      controls.removeEventListener("objectChange", onObjectChange);
      controls.removeEventListener("dragging-changed", onDraggingChanged);
      controls.removeEventListener("axis-changed", updateCursor);
      scaleDrag = null;
      handleDrag = null;
      if (cursor) canvas.style.cursor = "";
      controls.detach();
      controls.dispose();
      // The raycaster is shared by every TransformControls: give it back its own method, unless a newer gizmo took it.
      if (controlsRaycaster.setFromCamera === setFromCamera) Reflect.deleteProperty(controlsRaycaster, "setFromCamera");
      scene.remove(pivot, helper, handles.group);
      handles.dispose();
      value.remove();
    },
  };
}

/**
 * Start (CSS px) of a label `size` long placed `gap` after the pointer at `along` in a viewport `room`
 * long; before the pointer when it would run past the far edge, and never outside VALUE_MARGIN.
 */
function besidePointer(along: number, gap: number, size: number, room: number): number {
  const after = along + gap;
  const start = after + size <= room - VALUE_MARGIN ? after : along - gap - size;
  return Math.max(VALUE_MARGIN, Math.min(start, room - size - VALUE_MARGIN));
}
