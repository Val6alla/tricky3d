import { DoubleSide, Group, Mesh, MeshBasicMaterial, PlaneGeometry, Sphere, Vector2, Vector3 } from "three";
import { LineMaterial } from "three/examples/jsm/lines/LineMaterial.js";
import { LineSegments2 } from "three/examples/jsm/lines/LineSegments2.js";
import { LineSegmentsGeometry } from "three/examples/jsm/lines/LineSegmentsGeometry.js";
import { useCutPreview, type CutPreviewState } from "../cutPreview";
import { centreOf, type Bounds } from "../mesh";
import type { CutPlane } from "../ops/types";
import type { Vec3 } from "../store";
import type { VpTheme } from "./theme";
import type { SceneLayer } from "./util";

export interface PlanePreview extends SceneLayer {
  /** World sphere the shown quad reaches over, so the camera's near and far planes can take it in; null while hidden. */
  reach(): Sphere | null;
}

/** Share of the box diagonal added to the quad's side, so its edge stands clear of the parts it crosses. */
export const QUAD_MARGIN = 0.1;
/** Smallest side in mm, so a cut through a single point or an empty box still shows. */
const MIN_SIDE = 1;
const FILL_OPACITY = 0.25;
const OUTLINE_OPACITY = 0.9;
/** Outline width in CSS pixels, the same on every display density. */
const OUTLINE_WIDTH = 1.5;
/** After parts and see-through parts, before the selection brackets (10) and the gizmo. */
const RENDER_ORDER = 5;

const dot = (a: Vec3, b: Vec3) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];

/**
 * Where the quad goes: centred on the box centre's projection onto the plane, a square whose side is the
 * box diagonal plus QUAD_MARGIN. Wherever the plane crosses the box, the section lies within half a
 * diagonal of that point, so the square (inscribed radius 0.55 diagonals) covers all of it.
 */
export function quadPlacement(plane: CutPlane, box: Bounds): { centre: Vec3; side: number } {
  const c = centreOf(box);
  const n = plane.normal;
  const d = dot(n, c) - plane.offset;
  const diagonal = Math.hypot(box.max[0] - box.min[0], box.max[1] - box.min[1], box.max[2] - box.min[2]);
  return { centre: [c[0] - n[0] * d, c[1] - n[1] * d, c[2] - n[2] * d], side: Math.max(diagonal, MIN_SIDE) * (1 + QUAD_MARGIN) };
}

/** The unit square's 4 edges as segment pairs, in the plane z = 0 like PlaneGeometry. */
// prettier-ignore
const SQUARE_EDGES = [
  -0.5, -0.5, 0,  0.5, -0.5, 0,
   0.5, -0.5, 0,  0.5,  0.5, 0,
   0.5,  0.5, 0, -0.5,  0.5, 0,
  -0.5,  0.5, 0, -0.5, -0.5, 0,
];

const UP = new Vector3(0, 0, 1);

/**
 * The Cut dialog's plane (src/cutPreview.ts) as a translucent double-sided square in the accent colour
 * with a thin outline. A unit square placed by the group's transform, so an update rewrites no buffers.
 * Depth-tested but writes no depth: parts in front hide it, it hides nothing, and the line where it
 * enters a part shows where the cut runs. A small polygon offset lets it win over a part face lying
 * exactly in the plane. It is not clipped by the clip planes, never hit by picking, and adds nothing to
 * the scene bounds, so zoom-to-fit and the clip-plane ranges ignore it.
 */
export function createPlanePreview(theme: VpTheme, requestRender: () => void): PlanePreview {
  const fillGeometry = new PlaneGeometry(1, 1);
  const fillMaterial = new MeshBasicMaterial({
    side: DoubleSide,
    transparent: true,
    opacity: FILL_OPACITY,
    depthWrite: false,
    polygonOffset: true,
    polygonOffsetFactor: -1,
    polygonOffsetUnits: -1,
  });
  const fill = new Mesh(fillGeometry, fillMaterial);
  fill.renderOrder = RENDER_ORDER;

  const outlineGeometry = new LineSegmentsGeometry().setPositions(SQUARE_EDGES);
  const outlineMaterial = new LineMaterial({ linewidth: OUTLINE_WIDTH, transparent: true, opacity: OUTLINE_OPACITY, depthWrite: false });
  const outline = new LineSegments2(outlineGeometry, outlineMaterial);
  outline.renderOrder = RENDER_ORDER + 1;
  const drawingBuffer = new Vector2();
  // LineMaterial widths are in units of `resolution`; drawing-buffer pixels times the ratio keeps CSS px.
  outline.onBeforeRender = (renderer) => {
    outlineMaterial.resolution.copy(renderer.getDrawingBufferSize(drawingBuffer));
    outlineMaterial.linewidth = OUTLINE_WIDTH * renderer.getPixelRatio();
  };

  for (const object of [fill, outline]) object.raycast = () => {};
  const group = new Group();
  group.name = "cut plane preview";
  group.add(fill, outline);
  group.visible = false;

  const normal = new Vector3();
  const reach = new Sphere();

  function show({ plane, box }: Pick<CutPreviewState, "plane" | "box">) {
    group.visible = plane !== null && box !== null;
    if (!plane || !box) return;
    const { centre, side } = quadPlacement(plane, box);
    group.position.fromArray(centre);
    group.quaternion.setFromUnitVectors(UP, normal.fromArray(plane.normal).normalize());
    group.scale.setScalar(side);
    reach.set(group.position, side * Math.SQRT1_2);
  }

  function applyTheme(t: VpTheme) {
    fillMaterial.color.set(t.accent);
    outlineMaterial.color.set(t.accent);
  }
  applyTheme(theme);

  // The engine draws once it is built, so the first show needs no render request.
  show(useCutPreview.getState());
  const unsubscribe = useCutPreview.subscribe((s, prev) => {
    if (s.plane === prev.plane && s.box === prev.box) return;
    show(s);
    requestRender();
  });

  return {
    group,
    applyTheme,
    reach: () => (group.visible ? reach : null),
    dispose() {
      unsubscribe();
      fillGeometry.dispose();
      fillMaterial.dispose();
      outlineGeometry.dispose();
      outlineMaterial.dispose();
    },
  };
}
