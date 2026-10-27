// The viewport's side of repair's edit tools. Move nodes: the vertex nearest the pointer on the working copy is
// marked, with its position in a tooltip by the pointer; a left drag from it moves it in the plane facing the camera,
// drawn live (the repair layer moves that vertex on the GPU only), and letting go moves it for real in Rust as one
// undo step (moveNode). Esc or a lost pointer drops the drag. Cut surfaces: the crosshair cursor; its clicks are
// src/viewport/picking.ts's. Picking and dragging stay here; the working copy changes only through Rust.
import { Plane, Ray, Vector3 } from "three";
import { isMovingNodes, formatPosition, moveNode, nearestCorner } from "../repair/nodes";
import { useRepair } from "../repair/store";
import type { Vec3 } from "../store";
import type { TrianglePickFn } from "./picking";

/** Further than this (px) from the press is a drag. */
const DRAG_SLOP = 3;
/** The tooltip's offset from the pointer, px. */
const TIP_OFFSET = 16;

export interface EditToolsDeps {
  canvas: HTMLCanvasElement;
  /** The element the tooltip goes in (the canvas fills it). */
  host: HTMLElement;
  /** The working copy's triangle under the pointer, as clicks pick it. */
  pick: TrianglePickFn;
  /** The camera ray through a client position, or null when the canvas has no size. */
  ray(clientX: number, clientY: number): Ray | null;
  /** The camera's forward direction. */
  viewDirection(): Vector3;
  /** Draws `vertex` of the working copy at `at` instead of where it is; null puts it back. */
  preview(vertex: number | null, at: Vec3 | null): void;
  /** Shows the marker on a point, or hides it. */
  marker(at: Vec3 | null): void;
  requestRender(): void;
}

export interface EditTools {
  /** True while a press belongs to Move nodes (a drag from a marked vertex): picking leaves it alone. */
  isActive(): boolean;
  dispose(): void;
}

export function attachEditTools(deps: EditToolsDeps): EditTools {
  const { canvas, host } = deps;
  const tip = document.createElement("div");
  tip.className = "viewport-tooltip";
  tip.hidden = true;
  host.appendChild(tip);

  /** The vertex under the pointer, and where it is. */
  let hover: { vertex: number; at: Vec3 } | null = null;
  let drag: { pointer: number; vertex: number; from: Vec3; x: number; y: number; moved: boolean; plane: Plane; to: Vec3 } | null = null;
  /** A move sent to Rust: its preview stays until the answer arrives. */
  let committing = false;
  const hit = new Vector3();

  function showTip(at: Vec3, e: { clientX: number; clientY: number }) {
    const r = host.getBoundingClientRect();
    tip.textContent = formatPosition(at);
    tip.style.left = `${e.clientX - r.left + TIP_OFFSET}px`;
    tip.style.top = `${e.clientY - r.top + TIP_OFFSET}px`;
    tip.hidden = false;
  }

  function clearHover() {
    if (!hover && tip.hidden) return;
    hover = null;
    tip.hidden = true;
    deps.marker(null);
    deps.requestRender();
  }

  function updateCursor() {
    const tool = useRepair.getState().session ? useRepair.getState().editTool : null;
    canvas.style.cursor = drag ? "grabbing" : tool === "cut" ? "crosshair" : tool === "move-nodes" && hover ? "grab" : "";
  }

  function hoverAt(e: PointerEvent) {
    const session = useRepair.getState().session;
    const found = session && !useRepair.getState().busy && !committing ? deps.pick(e.clientX, e.clientY) : null;
    if (!session || !found) {
      clearHover();
      updateCursor();
      return;
    }
    const vertex = nearestCorner(session.mesh, found.triangle, found.point);
    const p = session.mesh.positions;
    const at: Vec3 = [p[vertex * 3], p[vertex * 3 + 1], p[vertex * 3 + 2]];
    if (hover?.vertex !== vertex) {
      deps.marker(at);
      deps.requestRender();
    }
    hover = { vertex, at };
    showTip(at, e);
    updateCursor();
  }

  /** Where the drag has the vertex now: the pointer's ray on the plane through its start that faces the camera. */
  function dragTo(e: PointerEvent): Vec3 | null {
    const ray = deps.ray(e.clientX, e.clientY);
    if (!ray || !drag || !ray.intersectPlane(drag.plane, hit)) return null;
    return [hit.x, hit.y, hit.z];
  }

  function endDrag() {
    if (!drag) return;
    const pointer = drag.pointer;
    drag = null;
    if (canvas.hasPointerCapture?.(pointer)) canvas.releasePointerCapture(pointer);
    updateCursor();
  }

  function dropDrag() {
    if (!drag) return;
    endDrag();
    deps.preview(null, null);
    clearHover();
  }

  const onDown = (e: PointerEvent) => {
    if (e.button !== 0 || !isMovingNodes() || !hover || useRepair.getState().busy || committing) return;
    const from = hover.at;
    const normal = deps.viewDirection().clone().negate();
    drag = {
      pointer: e.pointerId,
      vertex: hover.vertex,
      from,
      x: e.clientX,
      y: e.clientY,
      moved: false,
      plane: new Plane().setFromNormalAndCoplanarPoint(normal, new Vector3(...from)),
      to: from,
    };
    canvas.setPointerCapture?.(e.pointerId);
    updateCursor();
  };

  const onMove = (e: PointerEvent) => {
    if (drag && e.pointerId === drag.pointer) {
      if (!drag.moved && Math.hypot(e.clientX - drag.x, e.clientY - drag.y) < DRAG_SLOP) return;
      const to = dragTo(e);
      if (!to) return;
      drag.moved = true;
      drag.to = to;
      deps.preview(drag.vertex, to);
      deps.marker(to);
      showTip(to, e);
      deps.requestRender();
      return;
    }
    if (isMovingNodes() && e.buttons === 0) hoverAt(e);
    else if (hover) clearHover();
  };

  const onUp = (e: PointerEvent) => {
    if (!drag || e.pointerId !== drag.pointer) return;
    const { vertex, to, moved } = drag;
    endDrag();
    if (!moved) return;
    committing = true;
    // The preview stays until the moved working copy replaces it; a refused or failed move puts it back.
    void moveNode(vertex, to).then((changed) => {
      committing = false;
      if (!changed) deps.preview(null, null);
      clearHover();
      deps.requestRender();
    });
  };

  const onKey = (e: KeyboardEvent) => {
    if (e.key !== "Escape" || !drag) return;
    e.preventDefault();
    e.stopPropagation();
    dropDrag();
  };

  const onLeave = () => {
    if (!drag) clearHover();
  };
  const onLost = () => {
    if (drag) dropDrag();
  };

  // Leaving Move nodes, or the working copy changing under the marker, hides it.
  const unsubscribe = useRepair.subscribe((r, prev) => {
    if (r.editTool !== prev.editTool || r.session?.mesh !== prev.session?.mesh || !r.session) {
      if (!isMovingNodes()) dropDrag();
      clearHover();
      updateCursor();
    }
  });

  // Registered before picking's own listeners (src/viewport/engine.ts), so a press here is claimed first.
  canvas.addEventListener("pointerdown", onDown);
  canvas.addEventListener("pointermove", onMove);
  canvas.addEventListener("pointerup", onUp);
  canvas.addEventListener("pointercancel", dropDrag);
  canvas.addEventListener("lostpointercapture", onLost);
  canvas.addEventListener("pointerleave", onLeave);
  window.addEventListener("keydown", onKey, { capture: true });

  return {
    isActive: () => drag !== null,
    dispose() {
      unsubscribe();
      canvas.removeEventListener("pointerdown", onDown);
      canvas.removeEventListener("pointermove", onMove);
      canvas.removeEventListener("pointerup", onUp);
      canvas.removeEventListener("pointercancel", dropDrag);
      canvas.removeEventListener("lostpointercapture", onLost);
      canvas.removeEventListener("pointerleave", onLeave);
      window.removeEventListener("keydown", onKey, { capture: true });
      tip.remove();
    },
  };
}
