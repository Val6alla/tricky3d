// Label generation drawn over the part: each placed label's frame as a thin outline on the surface, its shape (letters
// or the picture's dark parts, as Rust drew them) filled in the highlight colour, a tag's plate standing out, and the
// handles: a dot at the centre (move) and, on the selected label, one at a corner (size and turn) and a tag's at the
// top of its plate (where it points). The handles are page elements kept over their points each frame; their drags go
// to src/label/session.ts.
import {
  BufferAttribute,
  BufferGeometry,
  Color,
  DoubleSide,
  Group,
  LineBasicMaterial,
  LineSegments,
  Mesh,
  MeshBasicMaterial,
  Vector3,
} from "three";
import { at, captionFace, labelFrame, plateSize, TAG_GAP, unit, cross, type Frame } from "../label/frame";
import type { ShapeReply } from "../label/ipc";
import { cornerOf, endHandleDrag, moveHandleDrag, startHandleDrag, tagHandleOf, type HandleKind } from "../label/session";
import { isMirrored, type PlacedLabel } from "../label/types";
import type { Vec3 } from "../store";
import type { ViewCamera } from "./cameras";
import type { VpTheme } from "./theme";
import type { SceneLayer } from "./util";
import "../label/label.css";

/** Over the parts, like the measurements. */
const ORDER = 21;

/** One label to draw, with its shape (null while Rust draws it) and its width in mm. */
export interface LabelItem {
  label: PlacedLabel;
  shape: ShapeReply | null;
  width: number;
  selected: boolean;
}

export interface LabelLayer extends SceneLayer {
  /** Draws the labels, or nothing for null (the mode is closed). */
  sync(items: LabelItem[] | null): void;
  /** Places the handles over their points for the frame about to be drawn (CSS px). */
  beforeRender(camera: ViewCamera, width: number, height: number): void;
}

interface Handle {
  el: HTMLDivElement;
  id: string;
  kind: HandleKind;
  at: Vec3;
}

export function createLabelLayer(theme: VpTheme, host: HTMLElement): LabelLayer {
  const group = new Group();
  group.renderOrder = ORDER;
  const fill = new MeshBasicMaterial({ color: new Color(theme.accent), transparent: true, opacity: 0.85, side: DoubleSide, depthTest: false });
  const lines = new LineBasicMaterial({ color: new Color(theme.text), transparent: true, opacity: 0.9, depthTest: false });
  const selectedLines = new LineBasicMaterial({ color: new Color(theme.accent), depthTest: false });
  // By label id and kind. A handle is kept from one drawing to the next, so the one being dragged keeps the pointer.
  const handles = new Map<string, Handle>();
  const projected = new Vector3();

  function clearDrawing() {
    for (const child of [...group.children]) {
      group.remove(child);
      if (child instanceof Mesh || child instanceof LineSegments) child.geometry.dispose();
    }
  }

  function clear() {
    clearDrawing();
    for (const h of handles.values()) h.el.remove();
    handles.clear();
  }

  function segments(points: Vec3[], material: LineBasicMaterial) {
    const geometry = new BufferGeometry();
    geometry.setAttribute("position", new BufferAttribute(new Float32Array(points.flat()), 3));
    const line = new LineSegments(geometry, material);
    line.renderOrder = ORDER;
    group.add(line);
  }

  /** A closed loop of points as line segments. */
  function loop(points: Vec3[], material: LineBasicMaterial) {
    segments(points.flatMap((p, i) => [p, points[(i + 1) % points.length]]), material);
  }

  function drawShape(f: Frame, shape: ShapeReply, item: LabelItem, lift: number) {
    const { label, width } = item;
    const sx = (width / Math.max(shape.aspect, 1e-9)) * (isMirrored(label.build) ? -1 : 1);
    const sy = label.height;
    const positions = new Float32Array((shape.fill.length / 2) * 3);
    for (let i = 0; i < shape.fill.length / 2; i++) {
      const p = at(f, shape.fill[2 * i] * sx, shape.fill[2 * i + 1] * sy, lift);
      positions.set(p, i * 3);
    }
    const geometry = new BufferGeometry();
    geometry.setAttribute("position", new BufferAttribute(positions, 3));
    const mesh = new Mesh(geometry, fill);
    mesh.renderOrder = ORDER;
    group.add(mesh);
  }

  /** Puts the handle at `point`, made the first time it is needed. Returns its key. */
  function handle(id: string, kind: HandleKind, point: Vec3, active: boolean): string {
    const key = `${id}:${kind}`;
    const className = `lbl-handle is-${kind}${active ? "" : " is-idle"}`;
    const known = handles.get(key);
    if (known) {
      known.at = point;
      if (known.el.className !== className) known.el.className = className;
      return key;
    }
    const el = document.createElement("div");
    el.className = className;
    el.title = kind === "centre" ? "Drag to move the label" : kind === "corner" ? "Drag to size and turn the label (Ctrl turns only, Shift sizes only)" : "Drag to point the tag";
    el.addEventListener("pointerdown", (e) => {
      if (e.button !== 0) return;
      e.preventDefault();
      e.stopPropagation();
      el.setPointerCapture(e.pointerId);
      startHandleDrag(id, kind);
    });
    el.addEventListener("pointermove", (e) => {
      if (el.hasPointerCapture(e.pointerId)) moveHandleDrag(e.clientX, e.clientY, e.ctrlKey || e.metaKey, e.shiftKey);
    });
    const end = (e: PointerEvent) => {
      if (el.hasPointerCapture(e.pointerId)) el.releasePointerCapture(e.pointerId);
      endHandleDrag();
    };
    el.addEventListener("pointerup", end);
    el.addEventListener("pointercancel", end);
    host.appendChild(el);
    handles.set(key, { el, id, kind, at: point });
    return key;
  }

  function sync(items: LabelItem[] | null) {
    if (!items) {
      clear();
      return;
    }
    clearDrawing();
    const shown = new Set<string>();
    for (const item of items) {
      const { label, width, selected } = item;
      // Not drawn until Rust has sized it to where it went.
      if (label.fitting) continue;
      const f = labelFrame(label.point, label.normal, label.angle);
      if (!f) continue;
      // A hair off the surface, so the drawing does not flicker with it.
      const lift = Math.max(0.02, label.height * 0.002);
      const [w, h] = [width / 2, label.height / 2];
      loop([at(f, -w, -h, lift), at(f, w, -h, lift), at(f, w, h, lift), at(f, -w, h, lift)], selected ? selectedLines : lines);
      if (item.shape && label.kind !== "tag") drawShape(f, item.shape, item, lift);
      if (label.kind === "tag") {
        // The plate standing along the tag's direction, its outline round the middle of its thickness, and the
        // caption on the face Rust puts it on.
        const d = unit(label.tagDirection ?? label.normal) ?? f.w;
        const u = unit(cross(cross(d, f.u), d)) ?? f.u;
        const tag: Frame = { origin: label.point, u, v: cross(d, u), w: d };
        const [pw, ph] = plateSize(width, label.height);
        const corners = [
          [-pw / 2, TAG_GAP],
          [pw / 2, TAG_GAP],
          [pw / 2, TAG_GAP + ph],
          [-pw / 2, TAG_GAP + ph],
        ];
        loop(
          corners.map(([x, z]) => at(tag, x, 0, z)),
          selected ? selectedLines : lines,
        );
        segments([label.point, at(tag, 0, 0, TAG_GAP)], selected ? selectedLines : lines);
        const face = captionFace(tag, label.thickness, ph);
        if (item.shape) drawShape({ ...face, origin: at(face, 0, 0, lift) }, item.shape, item, 0);
      }
      shown.add(handle(label.id, "centre", label.point, selected));
      if (selected) {
        const [cx, cy] = cornerOf({ ...label, width });
        shown.add(handle(label.id, "corner", at(f, cx, cy, lift), true));
        const tag = label.kind === "tag" ? tagHandleOf(label) : null;
        if (tag) shown.add(handle(label.id, "tag", tag, true));
      }
    }
    for (const [key, h] of handles) {
      if (shown.has(key)) continue;
      h.el.remove();
      handles.delete(key);
    }
  }

  return {
    group,
    sync,
    beforeRender(camera: ViewCamera, width: number, height: number) {
      for (const h of handles.values()) {
        projected.set(h.at[0], h.at[1], h.at[2]).project(camera);
        const visible = projected.z > -1 && projected.z < 1;
        h.el.hidden = !visible;
        if (!visible) continue;
        h.el.style.left = `${((projected.x + 1) / 2) * width}px`;
        h.el.style.top = `${((1 - projected.y) / 2) * height}px`;
      }
    },
    applyTheme(next: VpTheme) {
      fill.color.set(next.accent);
      lines.color.set(next.text);
      selectedLines.color.set(next.accent);
    },
    dispose() {
      clear();
      fill.dispose();
      lines.dispose();
      selectedLines.dispose();
    },
  };
}
