// Measure mode drawn over the parts: each measurement's features (points, edges, circles, spheres, surfaces) and its
// dimension line with arrowheads, the clicks of the one in progress, and every value in a label. Markers and arrowheads
// keep their size on screen (beforeRender; arrows only while "Scale arrows when zooming" is on), the labels are page
// elements placed over their anchors each frame, stacked upwards where two would cover each other, so all of it stays
// readable while the view turns and zooms. Drawn on
// top of the parts, through them, like dimensions on a drawing. Rust worked every point out (src/measure); this only
// draws.
import { BufferGeometry, Color, ConeGeometry, DynamicDrawUsage, Group, InstancedMesh, Matrix4, MeshBasicMaterial, Quaternion, SphereGeometry, Vector2, Vector3 } from "three";
import { LineMaterial } from "three/examples/jsm/lines/LineMaterial.js";
import { LineSegments2 } from "three/examples/jsm/lines/LineSegments2.js";
import { LineSegmentsGeometry } from "three/examples/jsm/lines/LineSegmentsGeometry.js";
import type { MeasureOptions, PendingFeature } from "../measure/store";
import type { MeasureFeature, MeasureResult } from "../measure/types";
import type { Vec3 } from "../store";
import type { ViewCamera } from "./cameras";
import { worldPerPixel } from "./gizmoMath";
import type { VpTheme } from "./theme";
import type { SceneLayer } from "./util";
import "../measure/measure.css";

/** In CSS px at 100%. */
const MARKER_RADIUS = 4;
const ARROW_LENGTH = 12;
const ARROW_RADIUS = 4;
const LINE_WIDTH = 2;
/** Segments of a drawn circle. */
const CIRCLE_SEGMENTS = 96;
/** Over the parts and the repair layer. */
const ORDER = 20;

/** One measurement to draw. */
export interface MeasureItem {
  id: string;
  result: MeasureResult | null;
  /** The label's text. */
  text: string;
  error: string | null;
  selected: boolean;
}

export interface MeasureDrawing {
  items: MeasureItem[];
  /** The features of the measurement in progress, and its point clicks towards a fitted circle or sphere. */
  pending: PendingFeature[];
  options: MeasureOptions;
}

export interface MeasureLayer extends SceneLayer {
  /** Draws a drawing, or nothing for null (the mode is closed). */
  sync(drawing: MeasureDrawing | null): void;
  /** Sizes the markers and arrows and places the labels for the frame about to be drawn (CSS px). */
  beforeRender(camera: ViewCamera, width: number, height: number): void;
}

type Tint = "measure" | "selected" | "pending";

interface Marker {
  at: Vec3;
  tint: Tint;
}

interface Arrow {
  /** The arrowhead's tip, and the direction it points along (unit). */
  tip: Vec3;
  dir: Vec3;
  tint: Tint;
  /** World length when arrows keep their size on the part. */
  fixed: number;
}

const sub = (a: Vec3, b: Vec3): Vec3 => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const len = (a: Vec3) => Math.hypot(a[0], a[1], a[2]);
const unit = (a: Vec3): Vec3 | null => {
  const l = len(a);
  return l > 0 ? [a[0] / l, a[1] / l, a[2] / l] : null;
};
const cross = (a: Vec3, b: Vec3): Vec3 => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];

/** Two unit vectors at right angles to `n` and each other. */
function basis(n: Vec3): [Vec3, Vec3] {
  const pick: Vec3 = Math.abs(n[0]) < 0.9 ? [1, 0, 0] : [0, 1, 0];
  const u = unit(cross(n, pick)) ?? [1, 0, 0];
  return [u, cross(n, u)];
}

/** Segment pairs round a circle. */
function circleSegments(centre: Vec3, normal: Vec3, radius: number, out: number[]) {
  const [u, v] = basis(unit(normal) ?? [0, 0, 1]);
  const at = (i: number) => {
    const t = (2 * Math.PI * i) / CIRCLE_SEGMENTS;
    const c = Math.cos(t) * radius;
    const s = Math.sin(t) * radius;
    return [centre[0] + u[0] * c + v[0] * s, centre[1] + u[1] * c + v[1] * s, centre[2] + u[2] * c + v[2] * s];
  };
  for (let i = 0; i < CIRCLE_SEGMENTS; i++) out.push(...at(i), ...at(i + 1));
}

/** The arc of an angle at `apex` between unit `a` and `b`, `radius` from it. */
function arcSegments(apex: Vec3, a: Vec3, b: Vec3, radius: number, out: number[]) {
  const cos = Math.max(-1, Math.min(1, a[0] * b[0] + a[1] * b[1] + a[2] * b[2]));
  const angle = Math.acos(cos);
  const n = unit(cross(a, b));
  if (!n || angle < 1e-6) return;
  const w = cross(n, a);
  const steps = Math.max(4, Math.ceil(angle / 0.08));
  const at = (i: number) => {
    const t = (angle * i) / steps;
    const c = Math.cos(t) * radius;
    const s = Math.sin(t) * radius;
    return [apex[0] + a[0] * c + w[0] * s, apex[1] + a[1] * c + w[1] * s, apex[2] + a[2] * c + w[2] * s];
  };
  for (let i = 0; i < steps; i++) out.push(...at(i), ...at(i + 1));
}

/** The lines and markers of one feature. */
function drawFeature(f: MeasureFeature, tint: Tint, lines: number[], markers: Marker[], arrows: Arrow[]) {
  switch (f.kind) {
    case "point":
      markers.push({ at: f.p, tint });
      break;
    case "edge":
      lines.push(...f.a, ...f.b);
      markers.push({ at: f.a, tint }, { at: f.b, tint });
      break;
    case "circle":
      circleSegments(f.centre, f.normal, f.radius, lines);
      markers.push({ at: f.centre, tint });
      break;
    case "sphere":
      for (const n of [[1, 0, 0], [0, 1, 0], [0, 0, 1]] as Vec3[]) circleSegments(f.centre, n, f.radius, lines);
      markers.push({ at: f.centre, tint });
      break;
    case "surface": {
      // An arrowhead standing on the surface, pointing at it.
      markers.push({ at: f.point, tint });
      const dir = unit(f.normal);
      if (dir) arrows.push({ tip: f.point, dir: [-dir[0], -dir[1], -dir[2]], tint, fixed: 0 });
      break;
    }
    case "wall":
      lines.push(...f.from, ...f.to);
      markers.push({ at: f.from, tint });
      break;
  }
}

/** A dimension line from `a` to `b` with an arrowhead at each end, pointing out. */
function dimension(a: Vec3, b: Vec3, tint: Tint, lines: number[], arrows: Arrow[]) {
  lines.push(...a, ...b);
  const dir = unit(sub(b, a));
  if (!dir) return;
  const fixed = len(sub(b, a)) * 0.12;
  arrows.push({ tip: b, dir, tint, fixed }, { tip: a, dir: [-dir[0], -dir[1], -dir[2]], tint, fixed });
}

export function createMeasureLayer(theme: VpTheme, host: HTMLElement): MeasureLayer {
  const group = new Group();
  group.name = "measurements";
  const labels = document.createElement("div");
  labels.className = "ms-labels";
  labels.hidden = true;
  host.appendChild(labels);

  const ball = new SphereGeometry(1, 16, 12);
  // Tip at the origin, pointing along +Y, so a rotation and a scale place it.
  const cone = new ConeGeometry(1, 1, 16).translate(0, -0.5, 0);
  const pointMaterial = new MeshBasicMaterial({ depthTest: false, transparent: true });
  const arrowMaterial = new MeshBasicMaterial({ depthTest: false, transparent: true });
  let spheres = instanced(ball, pointMaterial, 16);
  let cones = instanced(cone, arrowMaterial, 16);

  const drawingBuffer = new Vector2();
  const lineFor = (name: string, color: string) => {
    const material = new LineMaterial({ linewidth: LINE_WIDTH, depthTest: false, transparent: true });
    material.color.set(color);
    const lines = new LineSegments2(new LineSegmentsGeometry(), material);
    lines.name = name;
    lines.renderOrder = ORDER;
    lines.visible = false;
    lines.frustumCulled = false;
    lines.raycast = () => {};
    lines.onBeforeRender = (renderer) => {
      material.resolution.copy(renderer.getDrawingBufferSize(drawingBuffer));
      material.linewidth = LINE_WIDTH * renderer.getPixelRatio();
    };
    return lines;
  };
  const lineSets: Record<Tint, LineSegments2> = {
    measure: lineFor("measurement lines", theme.measure),
    selected: lineFor("selected measurement lines", theme.measureSelected),
    pending: lineFor("measurement in progress", theme.measurePending),
  };
  group.add(lineSets.measure, lineSets.selected, lineSets.pending, spheres, cones);

  let colors: Record<Tint, Color> = tints(theme);
  let markers: Marker[] = [];
  let arrows: Arrow[] = [];
  let options: MeasureOptions | null = null;
  /** Each label, its anchor, and its size in px (read once it is on show). */
  let labelAt: { el: HTMLDivElement; at: Vec3; width: number; height: number }[] = [];

  function instanced(geometry: BufferGeometry, material: MeshBasicMaterial, count: number): InstancedMesh<BufferGeometry, MeshBasicMaterial> {
    const mesh = new InstancedMesh(geometry, material, count);
    mesh.count = 0;
    mesh.renderOrder = ORDER + 1;
    mesh.frustumCulled = false;
    mesh.instanceMatrix.setUsage(DynamicDrawUsage);
    mesh.raycast = () => {};
    return mesh;
  }

  /** Makes an instanced mesh hold at least `n` instances. */
  function room(
    mesh: InstancedMesh<BufferGeometry, MeshBasicMaterial>,
    n: number,
    make: (count: number) => InstancedMesh<BufferGeometry, MeshBasicMaterial>,
  ): InstancedMesh<BufferGeometry, MeshBasicMaterial> {
    if (n <= mesh.instanceMatrix.count) return mesh;
    const next = make(Math.max(n, 2 * mesh.instanceMatrix.count));
    group.remove(mesh);
    mesh.dispose();
    group.add(next);
    return next;
  }

  function setLines(tint: Tint, segments: number[]) {
    const lines = lineSets[tint];
    lines.geometry.dispose();
    const geometry = new LineSegmentsGeometry();
    if (segments.length >= 6) geometry.setPositions(segments);
    lines.geometry = geometry;
    lines.visible = segments.length >= 6;
  }

  function setLabels(items: MeasureItem[]) {
    labels.replaceChildren();
    labelAt = [];
    for (const item of items) {
      const at = item.result?.anchor;
      if (!at) continue;
      const el = document.createElement("div");
      el.className = `ms-label${item.selected ? " is-selected" : ""}${item.error ? " is-error" : ""}`;
      el.textContent = item.error ?? item.text;
      labels.appendChild(el);
      labelAt.push({ el, at, width: 0, height: 0 });
    }
  }

  const scaleOf = new Vector3();
  const centre = new Vector3();
  const matrix = new Matrix4();
  const still = new Quaternion();
  const turn = new Quaternion();
  const up = new Vector3(0, 1, 0);
  const along = new Vector3();
  const projected = new Vector3();

  return {
    group,

    applyTheme(t) {
      lineSets.measure.material.color.set(t.measure);
      lineSets.selected.material.color.set(t.measureSelected);
      lineSets.pending.material.color.set(t.measurePending);
      colors = tints(t);
    },

    sync(drawing) {
      const lines: Record<Tint, number[]> = { measure: [], selected: [], pending: [] };
      markers = [];
      arrows = [];
      options = drawing?.options ?? null;
      for (const item of drawing?.items ?? []) {
        const r = item.result;
        if (!r) continue;
        const tint: Tint = item.selected ? "selected" : "measure";
        for (const f of r.features) drawFeature(f, tint, lines[tint], markers, arrows);
        if (r.line && (r.kind === "distance" || r.kind === "thickness")) dimension(r.line[0], r.line[1], tint, lines[tint], arrows);
        if (r.line && r.kind === "radius") {
          lines[tint].push(...r.line[0], ...r.line[1]);
          const dir = unit(sub(r.line[1], r.line[0]));
          if (dir) arrows.push({ tip: r.line[1], dir, tint, fixed: len(sub(r.line[1], r.line[0])) * 0.15 });
        }
        if (r.apex && r.arms) {
          const reach = Math.min(...r.features.filter((f) => f.kind === "point").map((f) => len(sub((f as { p: Vec3 }).p, r.apex!))).filter((d) => d > 0));
          if (Number.isFinite(reach)) arcSegments(r.apex, r.arms[0], r.arms[1], reach * 0.3, lines[tint]);
        }
      }
      for (const p of drawing?.pending ?? []) {
        drawFeature(p.world, "pending", lines.pending, markers, arrows);
        if (p.outline.length >= 6) lines.pending.push(...p.outline);
      }
      for (const tint of Object.keys(lines) as Tint[]) setLines(tint, lines[tint]);
      spheres = room(spheres, markers.length, (n) => instanced(ball, pointMaterial, n));
      cones = room(cones, arrows.length, (n) => instanced(cone, arrowMaterial, n));
      spheres.count = markers.length;
      cones.count = arrows.length;
      markers.forEach((m, i) => spheres.setColorAt(i, colors[m.tint]));
      arrows.forEach((a, i) => cones.setColorAt(i, colors[a.tint]));
      if (spheres.instanceColor) spheres.instanceColor.needsUpdate = true;
      if (cones.instanceColor) cones.instanceColor.needsUpdate = true;
      setLabels(drawing?.items ?? []);
      labels.hidden = drawing === null;
      group.visible = drawing !== null;
    },

    beforeRender(camera, width, height) {
      if (!group.visible || !(height > 0) || !options) return;
      camera.updateMatrixWorld();
      const markerPx = (MARKER_RADIUS * options.markerSize) / 100;
      const arrowScale = options.arrowSize / 100;
      markers.forEach((m, i) => {
        centre.fromArray(m.at);
        spheres.setMatrixAt(i, matrix.compose(centre, still, scaleOf.setScalar(markerPx * worldPerPixel(camera, centre, height))));
      });
      arrows.forEach((a, i) => {
        centre.fromArray(a.tip);
        // On screen: a fixed size in px. On the part: a share of the line, or for a surface's normal its size now.
        const px = worldPerPixel(camera, centre, height);
        const length = options!.scaleArrows || a.fixed <= 0 ? ARROW_LENGTH * arrowScale * px : a.fixed * arrowScale;
        const radius = (length * ARROW_RADIUS) / ARROW_LENGTH;
        turn.setFromUnitVectors(up, along.fromArray(a.dir));
        cones.setMatrixAt(i, matrix.compose(centre, turn, scaleOf.set(radius, length, radius)));
      });
      if (markers.length) spheres.instanceMatrix.needsUpdate = true;
      if (arrows.length) cones.instanceMatrix.needsUpdate = true;
      const view = new Matrix4().multiplyMatrices(camera.projectionMatrix, camera.matrixWorldInverse);
      // Sizes are read once, all together, before anything is moved: no layout between moves.
      for (const label of labelAt) {
        if (label.width === 0) {
          label.width = label.el.offsetWidth;
          label.height = label.el.offsetHeight;
        }
      }
      const placed: { left: number; top: number; right: number; bottom: number }[] = [];
      const covers = (r: { left: number; top: number; right: number; bottom: number }) =>
        placed.some((p) => r.left < p.right && r.right > p.left && r.top < p.bottom && r.bottom > p.top);
      for (const { el, at, width: w, height: h } of labelAt) {
        projected.fromArray(at).applyMatrix4(view);
        const shown = projected.z > -1 && projected.z < 1;
        el.style.visibility = shown ? "visible" : "hidden";
        if (!shown) continue;
        // Centred above the anchor; moved up a label at a time while it would cover one already placed.
        const x = ((projected.x + 1) / 2) * width;
        const y = ((1 - projected.y) / 2) * height;
        const box = { left: x - w / 2, top: y - h * 1.3, right: x + w / 2, bottom: y - h * 0.3 };
        for (let tries = 0; tries < 8 && covers(box); tries++) {
          box.top -= h + 2;
          box.bottom -= h + 2;
        }
        placed.push({ ...box });
        el.style.transform = `translate(${Math.round(box.left)}px, ${Math.round(box.top)}px)`;
      }
    },

    dispose() {
      labels.remove();
      for (const lines of Object.values(lineSets)) {
        lines.geometry.dispose();
        lines.material.dispose();
      }
      spheres.dispose();
      cones.dispose();
      ball.dispose();
      cone.dispose();
      pointMaterial.dispose();
      arrowMaterial.dispose();
    },
  };
}

function tints(theme: VpTheme): Record<Tint, Color> {
  return { measure: new Color(theme.measure), selected: new Color(theme.measureSelected), pending: new Color(theme.measurePending) };
}
