// Repair mode's drawing of the working copy (world space): its shaded surface in the part's colour with a thin dark
// wireframe over it (in wireframe shading, the wireframe alone in the part's colour), the highlights the Status and
// View tabs switch on, and the selected triangles in green above the other highlights. Every overlay shares the working copy's position buffer and draws a subset of it through an
// index of its own, built when the repair store changes and never per frame; everything honours the clip planes.
// Outside repair mode the layer draws nothing and holds no GPU buffers.
import {
  BackSide,
  BufferAttribute,
  BufferGeometry,
  DoubleSide,
  DynamicDrawUsage,
  FrontSide,
  Group,
  LineBasicMaterial,
  LineSegments,
  Mesh,
  MeshStandardMaterial,
  Vector2,
  Vector3,
  type Plane,
} from "three";
import { LineMaterial } from "three/examples/jsm/lines/LineMaterial.js";
import { LineSegments2 } from "three/examples/jsm/lines/LineSegments2.js";
import { LineSegmentsGeometry } from "three/examples/jsm/lines/LineSegmentsGeometry.js";
import type { Bounds } from "../mesh";
import { edgeAdjacency, triangleNormals, type EdgeAdjacency } from "../repair/selection";
import type { RepairState } from "../repair/store";
import type { MeshData, Shading, Vec3 } from "../store";
import type { ViewCamera } from "./cameras";
import { worldPerPixel } from "./gizmoMath";
import { GHOST_OPACITY, surfaceMaterial, type PartsLayer } from "./parts";
import type { VpTheme } from "./theme";
import type { SceneLayer } from "./util";

/** Hole and non-manifold edges, in CSS px, over a dark outline 1 px wider on each side so they read on light parts. */
const LINE_WIDTH = 3;
const HALO_WIDTH = LINE_WIDTH + 2;
/** The wireframe's opacity. An edge between two triangles is drawn once for each, which darkens it to about 0.5. */
const WIRE_OPACITY = 0.3;
/**
 * The wireframe fades in where the mesh's lines stand apart on screen: invisible where a vertex's shortest edge is
 * under the first number of device pixels (lines that close cover the surface and only darken it), fully drawn from
 * the second. So a 700k-triangle part shows its shading from afar, and its triangles wherever they are big enough.
 */
const WIRE_FADE: [number, number] = [3, 12];
/** Back faces show the part colour times this: the inside reads dark, and red stays free for inverted triangles. */
const INSIDE_SHADE = 0.45;
/** Slack on the dihedral test, so float noise in the normals never marks an edge of a flat surface at 0°. */
const ANGLE_SLACK = 1e-6;
/**
 * Draw order after the surface (0). The fills are opaque and drawn by priority, the selection last; the wireframe
 * and the lines are blended after them. Dimmed parts come after all of it (parts.ts).
 */
const ORDER = { slim: 1, degenerate: 2, inverted: 3, intersecting: 4, selected: 5, wire: 6, edges: 7, halo: 8, line: 9 };

type FillName = "slim" | "degenerate" | "inverted" | "intersecting" | "selected";

export interface RepairLayer extends SceneLayer {
  /**
   * Brings the drawing in line with the repair store: the session's working copy, selection and highlights, the
   * Status tab's switches and the View tab's slim faces. `color` is the repaired part's colour; `shading`
   * "wireframe" leaves the surface out and draws the wireframe as the working copy itself, like a wireframe part:
   * in that colour, opaque and without the fade. True when anything drawn changed.
   */
  sync(state: RepairState, color: string, shading: Shading): boolean;
  /**
   * Readies the wireframe for the frame about to be drawn (`bufferHeight`: the drawing buffer's height in device
   * pixels). Its fade is per vertex, in the shader; over the surface this leaves the whole wireframe out when even the
   * mesh's widest spacing, seen at the working copy's nearest point, would fade it away, so a part seen from afar draws
   * no lines. Drawn alone, in wireframe shading, it always stays.
   */
  beforeRender(camera: ViewCamera, bufferHeight: number): void;
  /** The working copy's world box; null outside repair mode or when it has no vertices. */
  bounds(): Bounds | null;
  /**
   * Move nodes' live drag: draws `vertex` of the working copy at `at`, and every triangle and highlight round it with
   * it, on the GPU only (the working copy itself stays as it is); null draws it where it is again. A new working copy
   * ends it too.
   */
  previewVertex(vertex: number | null, at: Vec3 | null): void;
}

/** What one working copy puts on the GPU: its positions once, and the geometries that share them. */
interface MeshView {
  mesh: MeshData;
  surface: BufferGeometry;
  /** Every geometry over the positions, the surface's included, to dispose together. */
  geometries: BufferGeometry[];
  bounds: Bounds | null;
  /** The largest of the vertices' spacings (vertexSpacing), mm. */
  widestSpacing: number;
  /** The slim faces for one smallest-angle setting, kept while the setting stays. */
  slim: { degrees: number; mask: Uint8Array } | null;
  hiding: boolean;
  /** While a vertex is dragged: the positions drawn (a copy, so the working copy never changes) and the vertex. */
  preview: { positions: Float32Array; vertex: number } | null;
}

/** Thick lines over segment pairs, drawn twice: a dark outline, then the colour. */
interface ThickLines {
  outline: LineSegments2;
  line: LineSegments2;
}

/**
 * The wireframe's material: LineBasicMaterial with a per-vertex fade. The `spacing` attribute (mm) becomes device
 * pixels at the vertex's depth (perspective) or at the zoom (orthographic) through the projection's vertical scale,
 * and WIRE_FADE turns that into the line's alpha. The `fade` uniform switches it on (1) or off (0, every line drawn
 * whole, for the wireframe drawn alone).
 */
function wireMaterial(bufferHeight: { value: number }, fade: { value: number }): LineBasicMaterial {
  const material = new LineBasicMaterial({ transparent: true, opacity: WIRE_OPACITY, depthWrite: false });
  material.onBeforeCompile = (shader) => {
    shader.uniforms.bufferHeight = bufferHeight;
    shader.uniforms.fade = fade;
    shader.vertexShader = shader.vertexShader
      .replace("#include <common>", "#include <common>\nattribute float spacing;\nuniform float bufferHeight;\nuniform float fade;\nvarying float vFade;")
      .replace(
        "#include <fog_vertex>",
        [
          "#include <fog_vertex>",
          "float perUnit = 0.5 * bufferHeight * projectionMatrix[1][1] / (isPerspectiveMatrix(projectionMatrix) ? max(-mvPosition.z, 1e-6) : 1.0);",
          `vFade = mix(1.0, smoothstep(${WIRE_FADE[0].toFixed(1)}, ${WIRE_FADE[1].toFixed(1)}, spacing * perUnit), fade);`,
        ].join("\n"),
      );
    shader.fragmentShader = shader.fragmentShader
      .replace("#include <common>", "#include <common>\nvarying float vFade;")
      .replace("#include <opaque_fragment>", "diffuseColor.a *= vFade;\n#include <opaque_fragment>");
  };
  return material;
}

/** The repair layer, clipped like the parts. */
export function createRepairLayer(theme: VpTheme, parts: Pick<PartsLayer, "watchClipping">): RepairLayer {
  const group = new Group();
  group.name = "repair working copy";
  group.visible = false;

  const empty = new BufferGeometry();
  const front = new Mesh(empty, surfaceMaterial(FrontSide));
  const back = new Mesh(empty, surfaceMaterial(BackSide));
  const ghost = new Mesh(
    empty,
    new MeshStandardMaterial({
      side: DoubleSide,
      flatShading: true,
      roughness: 0.8,
      metalness: 0,
      transparent: true,
      opacity: GHOST_OPACITY,
      depthWrite: false,
      clipIntersection: true,
    }),
  );
  ghost.visible = false;
  const bufferHeight = { value: 1 };
  const fade = { value: 1 };
  const wire = new LineSegments(empty, wireMaterial(bufferHeight, fade));
  wire.renderOrder = ORDER.wire;
  /** Wireframe shading: the wire is drawn alone, as the working copy itself (applyWire). */
  let wireOnly = false;
  /** The theme's edge colour, the wire's over the surface. */
  let edgeColor = theme.edge;
  // Names for the scene inspector and the tests.
  front.name = "surface";
  back.name = "inside";
  ghost.name = "ghost";
  wire.name = "wireframe";

  /** A highlight over whole triangles: lit like the surface, from either side, exactly on it (the surface sits back). */
  const fill = (name: FillName, color: string) => {
    const mesh = new Mesh(empty, new MeshStandardMaterial({ side: DoubleSide, flatShading: true, roughness: 0.8, metalness: 0, depthWrite: false, color }));
    mesh.name = `${name} triangles`;
    mesh.renderOrder = ORDER[name];
    return mesh;
  };
  // Highlight colours come from the --vp-repair-* tokens (theme.css), the same in both themes because they are drawn
  // on the part; inverted triangles take the inside colour (--vp-backface), the red a flipped face shows elsewhere.
  const fills: Record<FillName, Mesh<BufferGeometry, MeshStandardMaterial>> = {
    slim: fill("slim", theme.repairSlim),
    degenerate: fill("degenerate", theme.repairDegenerate),
    inverted: fill("inverted", theme.backface),
    intersecting: fill("intersecting", theme.repairError),
    selected: fill("selected", theme.repairSelected),
  };

  /** A highlight over mesh edges, as thin lines on top of the wireframe. */
  const thinLines = (name: string, color: string) => {
    const lines = new LineSegments(empty, new LineBasicMaterial({ color, transparent: true, depthWrite: false }));
    lines.name = name;
    lines.renderOrder = ORDER.edges;
    return lines;
  };
  const sharpLines = thinLines("sharp edges", theme.accent);
  const slimLines = thinLines("slim edges", theme.repairSlim);
  const overlays = [...Object.values(fills), sharpLines, slimLines];

  const drawingBuffer = new Vector2();
  const nearest = new Vector3();
  const thickLines = (name: string, color: string): ThickLines => {
    const geometry = new LineSegmentsGeometry();
    const pass = (passName: string, width: number, order: number, lineColor: string) => {
      const material = new LineMaterial({ linewidth: width, transparent: true });
      material.color.set(lineColor);
      const lines = new LineSegments2(geometry, material);
      lines.name = passName;
      lines.renderOrder = order;
      lines.raycast = () => {};
      // LineMaterial widths are in units of `resolution`; drawing-buffer pixels times the ratio keeps CSS px.
      lines.onBeforeRender = (renderer) => {
        material.resolution.copy(renderer.getDrawingBufferSize(drawingBuffer));
        material.linewidth = width * renderer.getPixelRatio();
      };
      return lines;
    };
    const out = { outline: pass(`${name} outline`, HALO_WIDTH, ORDER.halo, theme.edge), line: pass(name, LINE_WIDTH, ORDER.line, color) };
    out.outline.visible = out.line.visible = false;
    return out;
  };
  const holeLines = thickLines("hole edges", theme.repairHole);
  const errorLines = thickLines("error edges", theme.repairError);

  group.add(front, back, ghost, wire, ...overlays);
  for (const thick of [holeLines, errorLines]) group.add(thick.outline, thick.line);
  for (const object of [front, back, ghost, wire, ...overlays]) object.raycast = () => {};

  let current: MeshView | null = null;
  const materials = [front, back, wire, ...overlays, holeLines.outline, holeLines.line, errorLines.outline, errorLines.line].map((o) => o.material);
  let ghostShown = false;
  const unwatchClipping = parts.watchClipping((planes: Plane[], ghostPlanes: Plane[], ghostOn: boolean) => {
    for (const material of materials) material.clippingPlanes = planes;
    ghost.material.clippingPlanes = ghostPlanes;
    ghostShown = ghostOn && planes.length > 0;
    ghost.visible = ghostShown && current !== null;
  });

  /** The inputs each drawn piece was last built from, to skip work when they are the same objects again. */
  const built = new Map<object, unknown[]>();
  const changed = (piece: object, inputs: unknown[]) => {
    const before = built.get(piece);
    if (before && before.length === inputs.length && before.every((v, i) => v === inputs[i])) return false;
    built.set(piece, inputs);
    return true;
  };

  function showMesh(mesh: MeshData) {
    dropMesh();
    // One position attribute for every geometry: three.js uploads it once and every overlay draws from it.
    const position = new BufferAttribute(mesh.positions, 3);
    const over = (index: BufferAttribute | null) => {
      const geometry = new BufferGeometry();
      geometry.setAttribute("position", position);
      if (index) geometry.setIndex(index);
      return geometry;
    };
    const surface = over(new BufferAttribute(mesh.indices, 1));
    surface.computeBoundingBox();
    surface.computeBoundingSphere();
    const box = surface.boundingBox;
    const wireGeometry = over(new BufferAttribute(wireIndex(mesh.indices), 1));
    const spacing = vertexSpacing(mesh.positions, mesh.indices);
    wireGeometry.setAttribute("spacing", new BufferAttribute(spacing.spacing, 1));
    const geometries = [surface, wireGeometry, ...overlays.map(() => over(null))];
    for (const geometry of geometries) {
      // Every subset lies inside the whole mesh's bounds, so culling can use them and need not measure its own.
      geometry.boundingBox = surface.boundingBox;
      geometry.boundingSphere = surface.boundingSphere;
    }
    front.geometry = back.geometry = ghost.geometry = surface;
    wire.geometry = geometries[1];
    overlays.forEach((o, i) => {
      o.geometry = geometries[i + 2];
      o.visible = false;
    });
    current = {
      mesh,
      surface,
      geometries,
      bounds: box && !box.isEmpty() ? { min: box.min.toArray(), max: box.max.toArray() } : null,
      widestSpacing: spacing.widest,
      slim: null,
      hiding: false,
      preview: null,
    };
    group.visible = true;
    ghost.visible = ghostShown;
  }

  /** Frees the working copy's GPU buffers. Its geometries all share one position buffer, so they go together. */
  function dropMesh() {
    if (!current) return;
    for (const geometry of current.geometries) geometry.dispose();
    for (const object of [front, back, ghost, wire, ...overlays]) object.geometry = empty;
    for (const thick of [holeLines, errorLines]) setSegments(thick, null);
    current = null;
    built.clear();
    group.visible = false;
  }

  /** The slim faces of the working copy for `degrees`, kept until the angle or the mesh changes. */
  function slimOf(view: MeshView, degrees: number): Uint8Array {
    if (view.slim?.degrees !== degrees) view.slim = { degrees, mask: slimMask(view.mesh.positions, view.mesh.indices, degrees) };
    return view.slim.mask;
  }

  function applyColors(color: string) {
    front.material.color.set(color);
    back.material.color.set(color).multiplyScalar(INSIDE_SHADE);
    ghost.material.color.set(color);
  }

  /**
   * The wire's look: over the surface, a faint overlay in the theme's edge colour that fades out where its lines crowd
   * (they would only darken the shading); alone, in wireframe shading, the working copy itself, as a wireframe part is
   * drawn: in the part's `color`, opaque, every line whole. Faded, the edge colour on a dark theme's background would
   * leave almost nothing on screen.
   */
  function applyWire(shading: Shading, color: string) {
    wireOnly = shading === "wireframe";
    wire.material.color.set(wireOnly ? color : edgeColor);
    wire.material.opacity = wireOnly ? 1 : WIRE_OPACITY;
    fade.value = wireOnly ? 0 : 1;
  }

  function applyTheme(t: VpTheme) {
    edgeColor = t.edge;
    if (!wireOnly) wire.material.color.set(t.edge);
    sharpLines.material.color.set(t.accent);
    fills.slim.material.color.set(t.repairSlim);
    fills.degenerate.material.color.set(t.repairDegenerate);
    fills.inverted.material.color.set(t.backface);
    fills.intersecting.material.color.set(t.repairError);
    fills.selected.material.color.set(t.repairSelected);
    slimLines.material.color.set(t.repairSlim);
    holeLines.line.material.color.set(t.repairHole);
    errorLines.line.material.color.set(t.repairError);
    for (const thick of [holeLines, errorLines]) thick.outline.material.color.set(t.edge);
  }
  applyTheme(theme);

  function applyHidden(view: MeshView, hidden: Uint8Array | null) {
    const triangles = Math.floor(view.mesh.indices.length / 3);
    if (!hasHidden(hidden, triangles)) {
      if (!view.hiding) return;
      view.surface.setIndex(new BufferAttribute(view.mesh.indices, 1));
      wire.geometry.setIndex(new BufferAttribute(wireIndex(view.mesh.indices), 1));
      view.hiding = false;
      return;
    }
    const visible = trianglesOfMask(view.mesh.indices, shownMask(hidden as Uint8Array, triangles));
    view.surface.setIndex(new BufferAttribute(visible, 1));
    wire.geometry.setIndex(new BufferAttribute(wireIndex(visible), 1));
    view.hiding = true;
  }

  return {
    group,
    applyTheme,

    sync(state, color, shading) {
      const session = state.session;
      if (!session) {
        if (!current) return false;
        dropMesh();
        return true;
      }
      let dirty = false;
      if (session.mesh !== current?.mesh) {
        showMesh(session.mesh);
        dirty = true;
      }
      const view = current as MeshView;
      const { indices } = view.mesh;
      const { highlight, view: viewTab } = state;
      const highlights = session.highlights;
      const step = (piece: object, inputs: unknown[], apply: () => void) => {
        if (!changed(piece, [view, ...inputs])) return;
        apply();
        dirty = true;
      };

      const hidden = session.hiddenTriangles;
      step(view.surface, [hidden], () => applyHidden(view, hidden));
      step(front.material, [color], () => applyColors(color));
      step(front, [shading], () => (front.visible = back.visible = shading !== "wireframe"));
      step(wire.material, [shading, color], () => applyWire(shading, color));
      step(fills.selected, [session.selection, hidden], () =>
        setIndex(fills.selected, session.selectedCount > 0 ? trianglesOfMask(indices, maskExcept(session.selection, hidden)) : null),
      );
      step(fills.inverted, [highlight.triangles && highlights?.inverted, hidden], () =>
        setIndex(fills.inverted, highlight.triangles && highlights ? trianglesOf(indices, idsExcept(highlights.inverted, hidden)) : null),
      );
      step(fills.degenerate, [highlight.degenerate && highlights?.degenerate, hidden], () =>
        setIndex(fills.degenerate, highlight.degenerate && highlights ? trianglesOf(indices, idsExcept(highlights.degenerate, hidden)) : null),
      );
      step(fills.intersecting, [highlight.errors && session.intersecting, hidden], () =>
        setIndex(fills.intersecting, highlight.errors && session.intersecting ? trianglesOf(indices, idsExcept(session.intersecting, hidden)) : null),
      );
      step(holeLines, [highlight.holes && highlights?.border, hidden], () =>
        setSegments(holeLines, highlight.holes && highlights ? visibleSegments(view.mesh, highlights.border, hidden) : null),
      );
      step(errorLines, [highlight.errors && highlights?.nonManifold, hidden], () =>
        setSegments(errorLines, highlight.errors && highlights ? visibleSegments(view.mesh, highlights.nonManifold, hidden) : null),
      );
      step(sharpLines, [highlight.edgesFrom, hidden], () =>
        setIndex(
          sharpLines,
          highlight.edgesFrom === null ? null : sharpEdges(indices, edgeAdjacency(view.mesh), triangleNormals(view.mesh), highlight.edgesFrom, hidden),
        ),
      );
      step(fills.slim, [viewTab.slimColored && viewTab.slimAngle, hidden], () =>
        setIndex(fills.slim, viewTab.slimColored ? trianglesOfMask(indices, maskExcept(slimOf(view, viewTab.slimAngle), hidden)) : null),
      );
      step(slimLines, [viewTab.slimEdges && viewTab.slimAngle, hidden], () =>
        setIndex(slimLines, viewTab.slimEdges ? edgesOfMask(indices, maskExcept(slimOf(view, viewTab.slimAngle), hidden)) : null),
      );
      return dirty;
    },

    beforeRender(camera, height) {
      const sphere = current?.surface.boundingSphere;
      if (!current || !sphere || !(height > 0)) return;
      bufferHeight.value = height;
      if (wireOnly) {
        wire.visible = true;
        return;
      }
      camera.updateMatrixWorld();
      // The point of the bounding sphere nearest the camera (its centre when the camera is at it): nothing of the
      // working copy is drawn larger than there.
      nearest.subVectors(camera.position, sphere.center);
      if (nearest.lengthSq() > 0) nearest.setLength(Math.min(sphere.radius, nearest.length()));
      nearest.add(sphere.center);
      wire.visible = current.widestSpacing / worldPerPixel(camera, nearest, height) > WIRE_FADE[0];
    },

    bounds() {
      return current?.bounds ?? null;
    },

    previewVertex(vertex, at) {
      if (!current) return;
      const position = current.surface.getAttribute("position") as BufferAttribute;
      const original = current.mesh.positions;
      if (!current.preview) {
        if (vertex === null || !at) return;
        // Same contents as the GPU buffer, so only the ranges written below go up.
        current.preview = { positions: original.slice(), vertex: -1 };
        position.array = current.preview.positions;
      }
      const preview = current.preview;
      position.clearUpdateRanges();
      if (preview.vertex >= 0 && preview.vertex !== vertex) {
        const o = preview.vertex * 3;
        preview.positions.set(original.subarray(o, o + 3), o);
        position.addUpdateRange(o, 3);
      }
      preview.vertex = -1;
      if (vertex !== null && at && vertex * 3 + 2 < preview.positions.length) {
        preview.positions.set(at, vertex * 3);
        position.addUpdateRange(vertex * 3, 3);
        preview.vertex = vertex;
      }
      position.needsUpdate = true;
    },

    dispose() {
      unwatchClipping();
      dropMesh();
      empty.dispose();
      for (const object of [front, back, ghost, wire, ...overlays]) object.material.dispose();
      for (const thick of [holeLines, errorLines]) {
        thick.outline.geometry.dispose();
        thick.outline.material.dispose();
        thick.line.material.dispose();
      }
    },
  };
}

/**
 * Draws `indices` (null or empty: hides the overlay). They are copied into the overlay's index buffer, which is
 * reused while it is big enough, so a selection change sends only the new indices to the GPU; a bigger buffer
 * replaces it, twice the size, and the old one is left to the garbage collector. The geometry shares the working
 * copy's position buffer, so it is never disposed on its own: three.js would delete that buffer for all of them.
 */
function setIndex(object: Mesh | LineSegments, indices: Uint32Array | null) {
  object.visible = indices !== null && indices.length > 0;
  if (!indices || !object.visible) return;
  const geometry = object.geometry;
  let index = geometry.index;
  if (!index || index.array.length < indices.length) {
    index = new BufferAttribute(new Uint32Array(Math.max(indices.length, 2 * (index?.array.length ?? 0))), 1);
    index.setUsage(DynamicDrawUsage);
    geometry.setIndex(index);
  }
  (index.array as Uint32Array).set(indices);
  index.clearUpdateRanges();
  index.addUpdateRange(0, indices.length);
  index.needsUpdate = true;
  geometry.setDrawRange(0, indices.length);
}

/** Shows segment pairs (x, y, z, x, y, z per edge, as Rust sends them) as thick lines, or hides them for null or none. */
function setSegments(thick: ThickLines, segments: Float32Array | null) {
  const shown = segments !== null && segments.length >= 6;
  // Each set gets a geometry of its own, which wraps the array without copying; nothing else shares it.
  const geometry = new LineSegmentsGeometry();
  if (shown) geometry.setPositions(segments.subarray(0, segments.length - (segments.length % 6)));
  thick.outline.geometry.dispose();
  thick.outline.geometry = thick.line.geometry = geometry;
  thick.outline.visible = thick.line.visible = shown;
}

// ---------------------------------------------------------------------------------------------
// Index subsets and measures. Pure, linear time.

/**
 * Each vertex's spacing for the wireframe's fade: the length of its shortest edge with any length, in mm, which is
 * how close the lines run near it (along a cylinder of long thin triangles, the short rim edges); 0 for a vertex with
 * none. `widest` is the largest spacing.
 */
export function vertexSpacing(positions: Float32Array, indices: Uint32Array): { spacing: Float32Array; widest: number } {
  const spacing = new Float32Array(Math.floor(positions.length / 3)).fill(Infinity);
  const edge = (a: number, b: number) => {
    const dx = positions[b * 3] - positions[a * 3];
    const dy = positions[b * 3 + 1] - positions[a * 3 + 1];
    const dz = positions[b * 3 + 2] - positions[a * 3 + 2];
    // Math.sqrt of the sum, not Math.hypot: several times faster, and these lengths can neither overflow nor underflow.
    const length = Math.sqrt(dx * dx + dy * dy + dz * dz);
    if (!(length > 0)) return;
    if (length < spacing[a]) spacing[a] = length;
    if (length < spacing[b]) spacing[b] = length;
  };
  const corners = indices.length - (indices.length % 3);
  for (let c = 0; c < corners; c += 3) {
    edge(indices[c], indices[c + 1]);
    edge(indices[c + 1], indices[c + 2]);
    edge(indices[c + 2], indices[c]);
  }
  let widest = 0;
  for (let v = 0; v < spacing.length; v++) {
    if (spacing[v] === Infinity) spacing[v] = 0;
    else if (spacing[v] > widest) widest = spacing[v];
  }
  return { spacing, widest };
}

/** The index of every triangle's three edges as vertex pairs: the wireframe. An edge between two triangles comes twice. */
export function wireIndex(indices: Uint32Array): Uint32Array {
  const triangles = Math.floor(indices.length / 3);
  const out = new Uint32Array(triangles * 6);
  for (let c = 0, o = 0; c < triangles * 3; c += 3, o += 6) {
    const a = indices[c];
    const b = indices[c + 1];
    const d = indices[c + 2];
    out[o] = a;
    out[o + 1] = b;
    out[o + 2] = b;
    out[o + 3] = d;
    out[o + 4] = d;
    out[o + 5] = a;
  }
  return out;
}

/** The corners (three indices each) of the triangles `ids` names, in that order; ids past the mesh are left out. */
export function trianglesOf(indices: Uint32Array, ids: ArrayLike<number>): Uint32Array {
  const triangles = Math.floor(indices.length / 3);
  const out = new Uint32Array(ids.length * 3);
  let k = 0;
  for (let i = 0; i < ids.length; i++) {
    const t = ids[i];
    if (!(t >= 0 && t < triangles)) continue;
    out[k++] = indices[t * 3];
    out[k++] = indices[t * 3 + 1];
    out[k++] = indices[t * 3 + 2];
  }
  return k === out.length ? out : out.slice(0, k);
}

function hasHidden(hidden: Uint8Array | null, triangles: number): boolean {
  if (!hidden) return false;
  const n = Math.min(hidden.length, triangles);
  for (let t = 0; t < n; t++) if (hidden[t]) return true;
  return false;
}

function shownMask(hidden: Uint8Array, triangles: number): Uint8Array {
  const mask = new Uint8Array(triangles);
  for (let t = 0; t < triangles; t++) if (!hidden[t]) mask[t] = 1;
  return mask;
}

function visibleSegments(mesh: MeshData, segments: Float32Array, hidden: Uint8Array | null): Float32Array {
  if (!hidden || !hasHidden(hidden, Math.floor(mesh.indices.length / 3))) return segments;
  const { positions, indices } = mesh;
  const at = new Map<string, number>();
  for (let v = 0; v < positions.length / 3; v++) at.set(`${positions[v * 3]},${positions[v * 3 + 1]},${positions[v * 3 + 2]}`, v);
  const visible = new Set<string>();
  const triangles = Math.floor(indices.length / 3);
  for (let t = 0; t < triangles; t++) {
    if (hidden[t]) continue;
    for (let k = 0; k < 3; k++) {
      const a = indices[t * 3 + k];
      const b = indices[t * 3 + ((k + 1) % 3)];
      visible.add(a < b ? `${a},${b}` : `${b},${a}`);
    }
  }
  const out: number[] = [];
  for (let i = 0; i + 5 < segments.length; i += 6) {
    const va = at.get(`${segments[i]},${segments[i + 1]},${segments[i + 2]}`);
    const vb = at.get(`${segments[i + 3]},${segments[i + 4]},${segments[i + 5]}`);
    if (va === undefined || vb === undefined) {
      out.push(segments[i], segments[i + 1], segments[i + 2], segments[i + 3], segments[i + 4], segments[i + 5]);
      continue;
    }
    const key = va < vb ? `${va},${vb}` : `${vb},${va}`;
    if (visible.has(key)) out.push(segments[i], segments[i + 1], segments[i + 2], segments[i + 3], segments[i + 4], segments[i + 5]);
  }
  return Float32Array.from(out);
}

function idsExcept(ids: Uint32Array, hidden: Uint8Array | null): ArrayLike<number> {
  if (!hidden) return ids;
  const out: number[] = [];
  for (let i = 0; i < ids.length; i++) if (!hidden[ids[i]]) out.push(ids[i]);
  return out;
}

function maskExcept(mask: Uint8Array, hidden: Uint8Array | null): Uint8Array {
  if (!hidden) return mask;
  const out = mask.slice();
  const n = Math.min(out.length, hidden.length);
  for (let t = 0; t < n; t++) if (hidden[t]) out[t] = 0;
  return out;
}

/** The corners of the triangles whose mask entry is set, in triangle order. */
export function trianglesOfMask(indices: Uint32Array, mask: Uint8Array): Uint32Array {
  const triangles = Math.min(mask.length, Math.floor(indices.length / 3));
  let count = 0;
  for (let t = 0; t < triangles; t++) if (mask[t]) count++;
  const out = new Uint32Array(count * 3);
  for (let t = 0, k = 0; t < triangles; t++) {
    if (!mask[t]) continue;
    out[k++] = indices[t * 3];
    out[k++] = indices[t * 3 + 1];
    out[k++] = indices[t * 3 + 2];
  }
  return out;
}

/** The edges of the triangles whose mask entry is set, as vertex pairs (three per triangle). */
export function edgesOfMask(indices: Uint32Array, mask: Uint8Array): Uint32Array {
  return wireIndex(trianglesOfMask(indices, mask));
}

/**
 * Slim faces: a mask of the triangles whose smallest angle is below `degrees`. The smallest angle has the largest
 * cosine, so a triangle is slim when any corner's cosine exceeds cos(degrees). A triangle without area counts (its
 * smallest angle is 0°).
 */
export function slimMask(positions: Float32Array, indices: Uint32Array, degrees: number): Uint8Array {
  const triangles = Math.floor(indices.length / 3);
  const mask = new Uint8Array(triangles);
  if (!(degrees > 0)) return mask;
  const limit = Math.cos((Math.min(degrees, 180) * Math.PI) / 180);
  for (let t = 0; t < triangles; t++) {
    const a = indices[t * 3] * 3;
    const b = indices[t * 3 + 1] * 3;
    const c = indices[t * 3 + 2] * 3;
    const abx = positions[b] - positions[a], aby = positions[b + 1] - positions[a + 1], abz = positions[b + 2] - positions[a + 2];
    const acx = positions[c] - positions[a], acy = positions[c + 1] - positions[a + 1], acz = positions[c + 2] - positions[a + 2];
    const bcx = positions[c] - positions[b], bcy = positions[c + 1] - positions[b + 1], bcz = positions[c + 2] - positions[b + 2];
    const ab = abx * abx + aby * aby + abz * abz;
    const ac = acx * acx + acy * acy + acz * acz;
    const bc = bcx * bcx + bcy * bcy + bcz * bcz;
    if (ab === 0 || ac === 0 || bc === 0) {
      mask[t] = 1;
      continue;
    }
    const atA = (abx * acx + aby * acy + abz * acz) / Math.sqrt(ab * ac);
    const atB = -(abx * bcx + aby * bcy + abz * bcz) / Math.sqrt(ab * bc);
    const atC = (acx * bcx + acy * bcy + acz * bcz) / Math.sqrt(ac * bc);
    if (atA > limit || atB > limit || atC > limit) mask[t] = 1;
  }
  return mask;
}

/**
 * The Status tab's "Edges from": the edges between exactly two triangles whose normals differ by more than `degrees`,
 * as vertex pairs. Two triangles that run along their edge the same way disagree in orientation, so one normal is
 * turned around first: the angle is the surface's, whichever way its triangles face. Edges of a triangle without
 * area are left out.
 */
export function sharpEdges(indices: Uint32Array, adjacency: EdgeAdjacency, normals: Float32Array, degrees: number, hidden?: Uint8Array | null): Uint32Array {
  const { edgeStart, edgeCorners } = adjacency;
  const edges = edgeStart.length - 1;
  const limit = Math.cos((Math.max(0, degrees) * Math.PI) / 180) - ANGLE_SLACK;
  const flat = (n: number) => normals[n] === 0 && normals[n + 1] === 0 && normals[n + 2] === 0;
  const out = new Uint32Array(edges * 2);
  let k = 0;
  for (let e = 0; e < edges; e++) {
    if (edgeStart[e + 1] - edgeStart[e] !== 2) continue;
    const c1 = edgeCorners[edgeStart[e]];
    const c2 = edgeCorners[edgeStart[e] + 1];
    if (hidden && (hidden[Math.floor(c1 / 3)] || hidden[Math.floor(c2 / 3)])) continue;
    // Triangle t's normal starts at 3t, and so does its first corner.
    const n1 = c1 - (c1 % 3);
    const n2 = c2 - (c2 % 3);
    if (flat(n1) || flat(n2)) continue;
    const dot = normals[n1] * normals[n2] + normals[n1 + 1] * normals[n2 + 1] + normals[n1 + 2] * normals[n2 + 2];
    if ((indices[c1] === indices[c2] ? -dot : dot) >= limit) continue;
    out[k++] = indices[c1];
    out[k++] = indices[c1 % 3 === 2 ? c1 - 2 : c1 + 1];
  }
  return out.slice(0, k);
}
