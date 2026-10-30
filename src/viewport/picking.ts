import type { Vector3 } from "three";
import { isBooleanOpen, selectInList } from "../boolean/session";
import { addCutPoint } from "../repair/cut/session";
import { useBoolean } from "../boolean/store";
import { IS_MAC } from "../platform";
import { isMeasureOpen, measureClick, type MeasureHit } from "../measure/session";
import { isLabelOpen, labelClick } from "../label/session";
import { layFlatAt } from "../arrange/session";
import { isRepairing, selectAt, selectNone, selectTriangles } from "../repair/session";
import { useRepair } from "../repair/store";
import { selectMode, useStore, type SelectMode, type Vec3 } from "../store";
import { useUi } from "../ui";
import { viewport } from "./api";
import { rectBetween, type ScreenRect } from "./boxSelect";

/** Further than this (px) between press and release is a drag (orbit, pan), not a click. */
const CLICK_SLOP = 4;

export interface PickHit {
  partId: string;
  point: Vector3;
}

export type PickFn = (clientX: number, clientY: number) => PickHit | null;

/** In repair mode: the working copy's triangle under the pointer, and the world point hit on it. */
export interface TrianglePick {
  triangle: number;
  point: Vec3;
}

export type TrianglePickFn = (clientX: number, clientY: number) => TrianglePick | null;

/** Box selection's two halves in the engine: the rectangle on the canvas, and what it selects. */
export interface BoxSelector {
  /** Shows the rectangle (canvas pixels), or hides it with null. */
  draw(rect: ScreenRect | null): void;
  /** Selects what the rectangle covers: parts, or in repair mode triangles of the working copy. */
  select(rect: ScreenRect, mode: SelectMode): void;
}

/**
 * Left click selects, with the same modifiers as the parts tree (Shift adds, Ctrl toggles, Cmd on
 * macOS: see selectMode); empty space clears unless a modifier is held. Right click, or Control-click
 * on macOS, on a part selects it unless it is already selected, then opens the part context menu.
 * In repair mode clicks select triangles of the working copy instead (`pickTriangle`), with the current
 * select tool: a plain click replaces the selection, Shift (or the toggle key) adds or removes, empty space
 * clears; a right click opens the repair menu, selecting the triangle under the pointer first when
 * nothing is selected. Parts are never selected from the viewport then. While a repair run is going,
 * clicks do nothing: its result replaces the mesh a pick or the menu's triangle points into, and brings
 * the selection the run started with (its progress dialog covers the window once it has taken a moment).
 * With Cut surfaces on, a left click on the working copy adds a point to the cut line instead of selecting, and with
 * Move nodes on a left click selects nothing (its drags are src/viewport/editTools.ts's); neither draws a rectangle.
 * In Boolean operations a left click on a part in the lists selects it there (empty space clears), and nothing else
 * is selected from the viewport; the Zoom to area tool turns every left drag into a rectangle to zoom into.
 * A press that `onGizmo` claims (a gizmo handle under the pointer) is never a click.
 *
 * A left drag that starts on empty space (no part under the pointer; in repair mode, no triangle of the working
 * copy) draws a selection rectangle; on release `box` selects what it covers, with the same modifiers as a click
 * (Shift adds, the toggle key toggles; the keys held at release count). Esc or a lost pointer drops the rectangle.
 * Not on macOS with Option (that drag pans) or Control (that press is a right click), nor while a repair run goes.
 *
 * Zoom to › Area and Point (the UI store's viewTool) come first in any mode: the next left drag draws the rectangle to
 * zoom into (`box.select` zooms), and the next left click on a part centres the view on the point hit. So does Arrange ›
 * Lay a face on the platform: the next left click on a part lays the face hit on the platform.
 *
 * In Measure mode a left click on a part goes to the measurement in progress (`measure` says what was hit), and no
 * drag draws a rectangle. In label generation a left click places a label or selects one (src/label/session.ts
 * labelClick), and no drag draws a rectangle either; the labels' handles take their own drags.
 */
export function attachPicking(
  canvas: HTMLCanvasElement,
  pick: PickFn,
  pickTriangle: TrianglePickFn,
  onGizmo: () => boolean,
  box: BoxSelector,
  measure: (clientX: number, clientY: number) => MeasureHit | null = () => null,
): () => void {
  const presses = new Map<number, { x: number; y: number }>();
  /** A left press on empty space: a box once it moves past the click slop. */
  let drag: { pointer: number; x: number; y: number; drawing: boolean } | null = null;
  let brush: { pointer: number; x: number; y: number; moved: boolean; painted: boolean; last: number; mode: SelectMode } | null = null;

  // The gizmo's own pointerdown listener runs first, so it has already claimed the press.
  const onDown = (e: PointerEvent) => {
    if (e.button === 0 && onGizmo()) {
      presses.delete(e.button);
      return;
    }
    presses.set(e.button, { x: e.clientX, y: e.clientY });
    if (e.button !== 0 || (IS_MAC && (e.altKey || e.ctrlKey))) return;
    if (useUi.getState().viewTool === "zoom-area") {
      drag = { pointer: e.pointerId, x: e.clientX, y: e.clientY, drawing: false };
      return;
    }
    if (useRepair.getState().busy) return;
    if (isRepairing() && useRepair.getState().editTool === null && useRepair.getState().selectTool === "brush") {
      brush = { pointer: e.pointerId, x: e.clientX, y: e.clientY, moved: false, painted: false, last: -1, mode: selectMode(e) };
      return;
    }
    if (boxStartsAt(e)) drag = { pointer: e.pointerId, x: e.clientX, y: e.clientY, drawing: false };
  };
  const onMove = (e: PointerEvent) => {
    if (brush && e.pointerId === brush.pointer) {
      if (Math.hypot(e.clientX - brush.x, e.clientY - brush.y) < CLICK_SLOP) return;
      if (!brush.moved) {
        brush.moved = true;
        canvas.setPointerCapture?.(e.pointerId);
      }
      if (useRepair.getState().busy) return;
      const hit = pickTriangle(e.clientX, e.clientY);
      if (!hit || hit.triangle === brush.last) return;
      const mode = brush.painted ? (brush.mode === "toggle" ? "toggle" : "add") : brush.mode;
      brush.painted = true;
      brush.last = hit.triangle;
      selectAt(hit.triangle, mode);
      return;
    }
    if (!drag || e.pointerId !== drag.pointer) return;
    if (!drag.drawing) {
      if (Math.hypot(e.clientX - drag.x, e.clientY - drag.y) < CLICK_SLOP) return;
      drag.drawing = true;
      // Releasing outside the canvas still ends the box.
      canvas.setPointerCapture?.(e.pointerId);
    }
    box.draw(rectOf(drag, e));
  };
  const dropBox = () => {
    if (drag?.drawing) box.draw(null);
    drag = null;
  };
  const onCancel = () => {
    presses.clear();
    brush = null;
    dropBox();
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape" && drag?.drawing) {
      e.preventDefault();
      e.stopPropagation();
      presses.delete(0);
      dropBox();
    }
  };
  const onUp = (e: PointerEvent) => {
    if (e.button === 0 && brush && e.pointerId === brush.pointer) {
      const painted = brush.moved;
      brush = null;
      if (painted) {
        presses.delete(0);
        return;
      }
    }
    if (e.button === 0 && drag) {
      const ended = drag;
      dropBox();
      if (ended.drawing && e.pointerId === ended.pointer) {
        presses.delete(0);
        if (useUi.getState().viewTool === "zoom-area" || !useRepair.getState().busy) box.select(rectOf(ended, e), selectMode(e));
        return;
      }
    }
    const press = presses.get(e.button);
    presses.delete(e.button);
    if (!press || Math.hypot(e.clientX - press.x, e.clientY - press.y) >= CLICK_SLOP) return;
    const menu = e.button === 2 || (e.button === 0 && IS_MAC && e.ctrlKey);
    const viewTool = useUi.getState().viewTool;
    if (viewTool !== null && e.button === 0 && !menu) {
      // A click with Area does nothing: it waits for its drag.
      if (viewTool === "zoom-point") centreAt(e);
      else if (viewTool === "lay-flat") layFlatClick(e);
      return;
    }
    if (isLabelOpen()) {
      if (e.button === 0 && !menu) labelClick(viewport.surfaceAt(e.clientX, e.clientY));
      return;
    }
    if (isMeasureOpen()) {
      if (menu) openPartMenuAt(e);
      else if (e.button === 0) {
        const hit = measure(e.clientX, e.clientY);
        if (hit) void measureClick(hit);
      }
      return;
    }
    if (isRepairing()) {
      if (useRepair.getState().busy) return;
      const tool = useRepair.getState().editTool;
      if (menu) openRepairMenuAt(e);
      else if (e.button === 0 && tool === "cut") addCutPointAt(e);
      else if (e.button === 0 && tool === null) selectTriangleAt(e);
    } else if (isBooleanOpen()) {
      if (e.button === 0 && !menu) selectListPartAt(e);
    } else if (menu) openPartMenuAt(e);
    else if (e.button === 0) selectPartAt(e);
  };
  const onContextMenu = (e: MouseEvent) => e.preventDefault();

  /** Nothing to click under the pointer: no part, or in repair mode no triangle of the working copy. */
  function emptyAt(e: PointerEvent): boolean {
    return isRepairing() ? !pickTriangle(e.clientX, e.clientY) : !pick(e.clientX, e.clientY);
  }

  /**
   * A left drag from here draws a rectangle: from empty space, or in Boolean operations from anywhere with Zoom to area
   * (and never with its Select tool, which selects one part at a time).
   */
  function boxStartsAt(e: PointerEvent): boolean {
    if (isBooleanOpen()) return useBoolean.getState().tool === "zoom-area";
    if (isMeasureOpen() || isLabelOpen()) return false;
    if (isRepairing() && useRepair.getState().editTool !== null) return false;
    if (isRepairing() && useRepair.getState().selectTool === "area") return true;
    return emptyAt(e);
  }

  /** The rectangle from the press to the pointer, in canvas pixels. */
  function rectOf(from: { x: number; y: number }, e: PointerEvent): ScreenRect {
    const r = canvas.getBoundingClientRect();
    return rectBetween(from.x - r.left, from.y - r.top, e.clientX - r.left, e.clientY - r.top);
  }

  /** Zoom to › Point: the point under the pointer on a part (in repair mode on the working copy) becomes the centre. */
  function centreAt(e: PointerEvent) {
    const point = isRepairing() ? pickTriangle(e.clientX, e.clientY)?.point : pick(e.clientX, e.clientY)?.point.toArray();
    if (!point) return;
    useUi.getState().setViewTool(null);
    viewport.centreOn([point[0], point[1], point[2]]);
  }

  /** Arrange › Lay a face on the platform: the face under the pointer goes down onto the platform. */
  function layFlatClick(e: PointerEvent) {
    const hit = viewport.surfaceAt(e.clientX, e.clientY);
    if (!hit) return;
    useUi.getState().setViewTool(null);
    void layFlatAt(hit);
  }

  function selectPartAt(e: PointerEvent) {
    const hit = pick(e.clientX, e.clientY);
    const store = useStore.getState();
    if (hit) store.select([hit.partId], selectMode(e));
    else if (selectMode(e) === "replace") store.clearSelection();
  }

  function selectListPartAt(e: PointerEvent) {
    const hit = pick(e.clientX, e.clientY);
    if (hit) selectInList(hit.partId);
    else if (selectMode(e) === "replace") selectInList(null);
  }

  function openPartMenuAt(e: PointerEvent) {
    const hit = pick(e.clientX, e.clientY);
    if (!hit) return;
    const store = useStore.getState();
    if (!store.selection.includes(hit.partId)) store.select([hit.partId], "replace");
    const { x, y, z } = hit.point;
    useUi.getState().openContextMenu({ x: e.clientX, y: e.clientY, point: [x, y, z] });
  }

  function selectTriangleAt(e: PointerEvent) {
    const hit = pickTriangle(e.clientX, e.clientY);
    const modified = selectMode(e) !== "replace";
    if (hit) selectAt(hit.triangle, modified ? "toggle" : "replace");
    else if (!modified) selectNone();
  }

  function addCutPointAt(e: PointerEvent) {
    const hit = pickTriangle(e.clientX, e.clientY);
    if (hit) addCutPoint(hit.point, hit.triangle);
  }

  /** The repair menu's actions work on the whole working copy too, so it also opens beside it (triangle null). */
  function openRepairMenuAt(e: PointerEvent) {
    const hit = pickTriangle(e.clientX, e.clientY);
    if (hit && useRepair.getState().session?.selectedCount === 0) selectTriangles([hit.triangle], "replace");
    useUi.getState().openContextMenu({ x: e.clientX, y: e.clientY, point: hit?.point ?? null, triangle: hit?.triangle ?? null });
  }

  canvas.addEventListener("pointerdown", onDown);
  canvas.addEventListener("pointermove", onMove);
  canvas.addEventListener("pointerup", onUp);
  canvas.addEventListener("pointercancel", onCancel);
  canvas.addEventListener("lostpointercapture", dropBox);
  canvas.addEventListener("contextmenu", onContextMenu);
  window.addEventListener("keydown", onKey, { capture: true });
  return () => {
    dropBox();
    canvas.removeEventListener("pointerdown", onDown);
    canvas.removeEventListener("pointermove", onMove);
    canvas.removeEventListener("pointerup", onUp);
    canvas.removeEventListener("pointercancel", onCancel);
    canvas.removeEventListener("lostpointercapture", dropBox);
    canvas.removeEventListener("contextmenu", onContextMenu);
    window.removeEventListener("keydown", onKey, { capture: true });
  };
}
