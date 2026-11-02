// Hollow drawn in the view (the cavity itself is a stand-in part, drawn by the parts layer): each drain hole's solid
// as Rust shaped it, see-through, in the highlight colour or red when it would not be cut; a handle over each hole's
// place (filled when selected) whose drag moves the hole along the part's outside; and a box round the walls too thin
// to hollow. Drags and clicks go to src/hollow/session.ts.
import { BufferAttribute, BufferGeometry, Color, DoubleSide, Group, LineBasicMaterial, LineSegments, Mesh, MeshBasicMaterial, Vector3 } from "three";
import { moveHole, selectHole, settleHoles, type PlacedHole } from "../hollow/session";
import type { ThinWalls } from "../hollow/types";
import type { Vec3 } from "../store";
import { viewport } from "./api";
import type { ViewCamera } from "./cameras";
import type { VpTheme } from "./theme";
import type { SceneLayer } from "./util";
import "../hollow/hollow.css";

const ORDER = 22;

export interface HollowItems {
  holes: readonly PlacedHole[];
  selected: string | null;
  thin: ThinWalls | null;
}

export interface HollowLayer extends SceneLayer {
  /** Draws the holes and the thin walls' box, or nothing for null (the panel is closed). */
  sync(items: HollowItems | null): void;
  /** Places the handles over their points for the frame about to be drawn (CSS px). */
  beforeRender(camera: ViewCamera, width: number, height: number): void;
}

interface Handle {
  el: HTMLDivElement;
  at: Vec3;
}

export function createHollowLayer(theme: VpTheme, host: HTMLElement): HollowLayer {
  const group = new Group();
  group.renderOrder = ORDER;
  const material = (color: string) =>
    new MeshBasicMaterial({ color: new Color(color), transparent: true, opacity: 0.55, side: DoubleSide, depthWrite: false });
  let good = material(theme.accent);
  let bad = material(theme.repairError);
  const box = new LineBasicMaterial({ color: new Color(theme.repairError), depthTest: false, transparent: true, opacity: 0.9 });
  const handles = new Map<string, Handle>();
  const projected = new Vector3();

  function clearDrawing() {
    for (const child of [...group.children]) {
      group.remove(child);
      if (child instanceof Mesh || child instanceof LineSegments) child.geometry.dispose();
    }
  }

  function clearHandles() {
    for (const h of handles.values()) h.el.remove();
    handles.clear();
  }

  function handle(id: string, at: Vec3, selected: boolean, refused: boolean) {
    const className = `hw-handle${selected ? " is-selected" : ""}${refused ? " is-bad" : ""}`;
    const known = handles.get(id);
    if (known) {
      known.at = at;
      if (known.el.className !== className) known.el.className = className;
      return;
    }
    const el = document.createElement("div");
    el.className = className;
    el.title = "Drag to move the hole; click to select it";
    let moved = false;
    el.addEventListener("pointerdown", (e) => {
      if (e.button !== 0) return;
      e.preventDefault();
      e.stopPropagation();
      el.setPointerCapture(e.pointerId);
      moved = false;
      selectHole(id);
    });
    el.addEventListener("pointermove", (e) => {
      if (!el.hasPointerCapture(e.pointerId)) return;
      moved = true;
      // The handle sits on the surface: hide it from the pick so the part under it is hit.
      el.style.pointerEvents = "none";
      moveHole(id, viewport.surfaceAt(e.clientX, e.clientY));
      el.style.pointerEvents = "";
    });
    const end = (e: PointerEvent) => {
      if (el.hasPointerCapture(e.pointerId)) el.releasePointerCapture(e.pointerId);
      if (moved) settleHoles();
      moved = false;
    };
    el.addEventListener("pointerup", end);
    el.addEventListener("pointercancel", end);
    host.appendChild(el);
    handles.set(id, { el, at });
  }

  function sync(items: HollowItems | null) {
    clearDrawing();
    if (!items) {
      clearHandles();
      return;
    }
    const shown = new Set<string>();
    for (const p of items.holes) {
      const refused = p.check !== null && !p.check.ok;
      if (p.solid && p.solid.indices.length > 0) {
        const geometry = new BufferGeometry();
        geometry.setAttribute("position", new BufferAttribute(p.solid.positions, 3));
        geometry.setIndex(new BufferAttribute(p.solid.indices, 1));
        const mesh = new Mesh(geometry, refused ? bad : good);
        mesh.renderOrder = ORDER;
        group.add(mesh);
      }
      handle(p.id, p.hole.at, p.id === items.selected, refused);
      shown.add(p.id);
    }
    for (const [id, h] of handles) {
      if (shown.has(id)) continue;
      h.el.remove();
      handles.delete(id);
    }
    if (items.thin) {
      const [a, b] = items.thin.bounds;
      const c = (i: number): Vec3 => [i & 1 ? b[0] : a[0], i & 2 ? b[1] : a[1], i & 4 ? b[2] : a[2]];
      const edges = [0, 1, 2, 3, 4, 5, 6, 7, 0, 2, 1, 3, 4, 6, 5, 7, 0, 4, 1, 5, 2, 6, 3, 7];
      const geometry = new BufferGeometry();
      geometry.setAttribute("position", new BufferAttribute(new Float32Array(edges.flatMap((i) => c(i))), 3));
      const lines = new LineSegments(geometry, box);
      lines.renderOrder = ORDER;
      group.add(lines);
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
      good.dispose();
      bad.dispose();
      good = material(next.accent);
      bad = material(next.repairError);
      box.color.set(next.repairError);
    },
    dispose() {
      clearDrawing();
      clearHandles();
      good.dispose();
      bad.dispose();
      box.dispose();
    },
  };
}
