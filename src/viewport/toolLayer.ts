// Repair's edit tools drawn over the working copy: Cut surfaces' points as pink spheres (the current one larger and
// deeper), its line along the surface in red (faded while a newer one is on its way from Rust), a thin straight line
// where a piece of it found no way along the surface, and Move nodes' marker on the point under the pointer. The
// spheres keep their size on screen (beforeRender), and everything honours the clip planes. Rust routes the line
// (src/repair/cut/session.ts asks for it); this only draws what it sent.
import { Color, DynamicDrawUsage, Group, InstancedMesh, Matrix4, Mesh, MeshStandardMaterial, Quaternion, SphereGeometry, Vector2, Vector3, type Plane } from "three";
import { LineMaterial } from "three/examples/jsm/lines/LineMaterial.js";
import { LineSegments2 } from "three/examples/jsm/lines/LineSegments2.js";
import { LineSegmentsGeometry } from "three/examples/jsm/lines/LineSegmentsGeometry.js";
import { pathStale, type CutState } from "../repair/cut/store";
import type { Vec3 } from "../store";
import type { ViewCamera } from "./cameras";
import { worldPerPixel } from "./gizmoMath";
import type { PartsLayer } from "./parts";
import type { VpTheme } from "./theme";
import type { SceneLayer } from "./util";

/** Sphere radii in CSS px. */
const POINT_RADIUS = 5;
const CURRENT_RADIUS = 7;
const MARKER_RADIUS = 6;
/** The cut line and the straight line over a gap, in CSS px. */
const LINE_WIDTH = 3;
const GAP_WIDTH = 1.5;
/** The line's opacity while a newer one is on its way. */
const STALE_OPACITY = 0.4;
/** After the repair layer's highlights (its lines end at 9). */
const ORDER = { line: 10, spheres: 11 };

export interface ToolLayer extends SceneLayer {
  /** Draws the cut points and line of `cut`, or nothing for null (the cutting mode is off). */
  syncCut(cut: CutState | null): void;
  /** Move nodes' marker on a point, or hidden for null. */
  setMarker(at: Vec3 | null): void;
  /** Sizes the spheres for the frame about to be drawn (`height`: the view's height in CSS px). */
  beforeRender(camera: ViewCamera, height: number): void;
}

/** The segment pairs (x, y, z, x, y, z each) along a polyline of points (x, y, z each). */
export function polylineSegments(points: Float32Array): Float32Array {
  const n = Math.floor(points.length / 3);
  const out = new Float32Array(Math.max(0, n - 1) * 6);
  for (let i = 0; i + 1 < n; i++) {
    out.set(points.subarray(i * 3, i * 3 + 6), i * 6);
  }
  return out;
}

export function createToolLayer(theme: VpTheme, parts: Pick<PartsLayer, "watchClipping">): ToolLayer {
  const group = new Group();
  group.name = "repair edit tools";
  const ball = new SphereGeometry(1, 20, 14);
  // White, tinted per instance.
  const pointMaterial = new MeshStandardMaterial({ roughness: 0.45, metalness: 0 });
  let spheres = new InstancedMesh(ball, pointMaterial, 8);
  spheres.name = "cut points";
  spheres.count = 0;
  spheres.renderOrder = ORDER.spheres;
  spheres.frustumCulled = false;
  spheres.instanceMatrix.setUsage(DynamicDrawUsage);
  const marker = new Mesh(ball, new MeshStandardMaterial({ roughness: 0.45, metalness: 0, color: theme.node }));
  marker.name = "node marker";
  marker.visible = false;
  marker.renderOrder = ORDER.spheres;
  let markerAt: Vec3 | null = null;

  const drawingBuffer = new Vector2();
  const thick = (name: string, width: number, color: string) => {
    const material = new LineMaterial({ linewidth: width, transparent: true });
    material.color.set(color);
    const lines = new LineSegments2(new LineSegmentsGeometry(), material);
    lines.name = name;
    lines.renderOrder = ORDER.line;
    lines.visible = false;
    lines.raycast = () => {};
    lines.onBeforeRender = (renderer) => {
      material.resolution.copy(renderer.getDrawingBufferSize(drawingBuffer));
      material.linewidth = width * renderer.getPixelRatio();
    };
    return lines;
  };
  const line = thick("cut line", LINE_WIDTH, theme.cutLine);
  const gaps = thick("cut line gaps", GAP_WIDTH, theme.repairError);
  // The straight line over a gap runs through the part wherever its surface curves outwards: drawn over it.
  gaps.material.depthTest = false;
  for (const object of [spheres, marker]) object.raycast = () => {};
  group.add(line, gaps, spheres, marker);

  let colors = { point: new Color(theme.cutPoint), current: new Color(theme.cutCurrent) };
  /** Each sphere's centre and radius in px, for beforeRender. */
  let centres: Vec3[] = [];
  let radii: number[] = [];

  const unwatchClipping = parts.watchClipping((planes: Plane[]) => {
    for (const material of [pointMaterial, marker.material, line.material, gaps.material]) material.clippingPlanes = planes;
  });

  function setSegments(lines: LineSegments2, segments: Float32Array | null) {
    const shown = segments !== null && segments.length >= 6;
    lines.geometry.dispose();
    const geometry = new LineSegmentsGeometry();
    if (shown) geometry.setPositions(segments);
    lines.geometry = geometry;
    lines.visible = shown;
  }

  function setSpheres(at: Vec3[], sizes: number[], tints: Color[]) {
    if (at.length > spheres.instanceMatrix.count) {
      // A bigger buffer, twice the size; the geometry and material stay.
      const next = new InstancedMesh(ball, pointMaterial, Math.max(at.length, 2 * spheres.instanceMatrix.count));
      next.name = spheres.name;
      next.renderOrder = ORDER.spheres;
      next.frustumCulled = false;
      next.instanceMatrix.setUsage(DynamicDrawUsage);
      next.raycast = () => {};
      group.remove(spheres);
      spheres.dispose();
      spheres = next;
      group.add(spheres);
    }
    spheres.count = at.length;
    tints.forEach((c, i) => spheres.setColorAt(i, c));
    if (spheres.instanceColor) spheres.instanceColor.needsUpdate = true;
    centres = at;
    radii = sizes;
  }

  const scaleOf = new Vector3();
  const centre = new Vector3();
  const matrix = new Matrix4();
  const still = new Quaternion();

  return {
    group,

    applyTheme(t) {
      colors = { point: new Color(t.cutPoint), current: new Color(t.cutCurrent) };
      line.material.color.set(t.cutLine);
      gaps.material.color.set(t.repairError);
      marker.material.color.set(t.node);
    },

    syncCut(cut) {
      if (!cut || cut.points.length === 0) {
        setSpheres([], [], []);
        setSegments(line, null);
        setSegments(gaps, null);
        return;
      }
      const fresh = cut.path !== null && !pathStale(cut);
      // Where Rust put the points (snapped to vertices) once it has routed them, where they were clicked till then.
      const at = fresh && cut.path ? cut.path.anchors : cut.points.map((p) => p.at);
      setSpheres(
        at,
        at.map((_, i) => (i === cut.current ? CURRENT_RADIUS : POINT_RADIUS)),
        at.map((_, i) => (i === cut.current ? colors.current : colors.point)),
      );
      const path = cut.path;
      const pieces = path ? path.pieces.filter((p): p is Float32Array => p !== null).map(polylineSegments) : [];
      const length = pieces.reduce((n, p) => n + p.length, 0);
      const joined = new Float32Array(length);
      pieces.reduce((o, p) => (joined.set(p, o), o + p.length), 0);
      setSegments(line, length ? joined : null);
      line.material.opacity = fresh && !cut.routing ? 1 : STALE_OPACITY;
      // Straight over each piece that found no way, so the gap shows.
      const broken: number[] = [];
      if (path && fresh) {
        path.pieces.forEach((piece, i) => {
          if (piece) return;
          const [a, b] = path.spans[i];
          broken.push(...path.anchors[a], ...path.anchors[b]);
        });
      }
      setSegments(gaps, broken.length ? Float32Array.from(broken) : null);
    },

    setMarker(at) {
      markerAt = at;
      marker.visible = at !== null;
    },

    beforeRender(camera, height) {
      if (!(height > 0)) return;
      camera.updateMatrixWorld();
      for (let i = 0; i < spheres.count; i++) {
        centre.fromArray(centres[i]);
        const r = radii[i] * worldPerPixel(camera, centre, height);
        spheres.setMatrixAt(i, matrix.compose(centre, still, scaleOf.setScalar(r)));
      }
      if (spheres.count) spheres.instanceMatrix.needsUpdate = true;
      if (markerAt) {
        marker.position.fromArray(markerAt);
        marker.scale.setScalar(MARKER_RADIUS * worldPerPixel(camera, marker.position, height));
      }
    },

    dispose() {
      unwatchClipping();
      spheres.dispose();
      ball.dispose();
      pointMaterial.dispose();
      marker.material.dispose();
      for (const lines of [line, gaps]) {
        lines.geometry.dispose();
        lines.material.dispose();
      }
    },
  };
}
