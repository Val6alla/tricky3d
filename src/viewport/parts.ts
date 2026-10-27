import {
  BackSide,
  Box3,
  BufferAttribute,
  BufferGeometry,
  DoubleSide,
  FrontSide,
  Group,
  LineBasicMaterial,
  LineSegments,
  Mesh,
  MeshBasicMaterial,
  MeshStandardMaterial,
  Plane,
  type Material,
  type Object3D,
  type Side,
} from "three";
import { isBoxCut, type PlaneSpec } from "../clip";
import { partBounds } from "../mesh";
import { cachedProblemEdges } from "../meshCache";
import type { MeshData, Part, Shading } from "../store";
import { featureEdgePositions, problemEdgePositions } from "./edges";
import type { VpTheme } from "./theme";

const TRANSPARENT_OPACITY = 0.35;
/** Opacity of the clipped-away part of a part when "Transparent cuts" is on. */
export const GHOST_OPACITY = 0.14;
const FEATURE_EDGE_ANGLE = 20;
/** Opacity of the parts a mode dims (PartsLook.dimOthers). */
const DIMMED_OPACITY = 0.25;
/**
 * Render order of dimmed parts: after everything the repair layer draws (up to 9), so a dimmed part in front of the
 * working copy blends over its wireframe and highlight lines too.
 */
const DIMMED_ORDER = 20;

/** GPU-side data derived from one MeshData, shared by every part that uses it. */
interface GeometryEntry {
  mesh: MeshData;
  surface: BufferGeometry;
  openEdges: BufferGeometry;
  featureEdges: BufferGeometry | null;
  users: number;
}

interface PartView {
  part: Part;
  entry: GeometryEntry;
  root: Group;
  front: Mesh<BufferGeometry, MeshStandardMaterial>;
  back: Mesh<BufferGeometry, MeshStandardMaterial>;
  wire: Mesh<BufferGeometry, MeshBasicMaterial>;
  openEdges: LineSegments<BufferGeometry, LineBasicMaterial>;
  featureEdges: LineSegments<BufferGeometry, LineBasicMaterial> | null;
  /** Never drawn; double-sided so clicks hit inner faces too. */
  pick: Mesh;
  /** The clipped-away region, drawn faintly for "Transparent cuts". Created the first time it is shown. */
  ghost: Mesh<BufferGeometry, MeshStandardMaterial> | null;
  /** The inside shows as red back faces; see showsRedInside. */
  redInside: boolean;
}

/**
 * How a mode changes the drawing. Repair mode hides its part (the repair layer draws its working copy) and dims the
 * rest; Boolean operations draws its two lists green and red and dims the rest.
 */
export interface PartsLook {
  /** Not drawn here. */
  hidden: string | null;
  /** Parts drawn in these colours instead of their own or the selection colour. */
  colors: ReadonlyMap<string, string>;
  /** Every other drawn part is dimmed: see-through and without edges. */
  dimOthers: boolean;
  /** The parts a click can hit; null for every drawn part. */
  pickable: ReadonlySet<string> | null;
}

export const PLAIN_LOOK: PartsLook = { hidden: null, colors: new Map(), dimOthers: false, pickable: null };

export interface PartsLayer {
  group: Group;
  /** Brings the scene in line with the store and the mode's look. Cheap to call: unchanged parts only get their materials re-set. */
  sync(parts: Part[], selection: string[], shading: Shading, look: PartsLook): void;
  applyTheme(theme: VpTheme): void;
  /** Raycast targets for the visible, pickable parts; `userData.partId` names the part. */
  pickTargets(): Object3D[];
  /** Union of the drawn parts' world boxes (loose for rotated parts). Empty when none are drawn. */
  visibleBox(out: Box3): Box3;
  /**
   * Clips every part drawing (surfaces, red back faces, wireframe, edges) with world-space `planes`,
   * which keep the points where dot(normal, p) + constant >= 0. With `ghost`, the removed region is drawn
   * faintly. Parts added later get the same clipping. The planes are copied.
   */
  setClipping(planes: readonly PlaneSpec[], ghost: boolean): void;
  /**
   * Lets another layer clip like the parts: calls `listener` now and after every setClipping with the planes in
   * three.js form, the same planes negated (with clipIntersection they keep the removed region, for a ghost), and
   * whether the removed region is drawn as a ghost. The two arrays are edited in place, so a material that holds
   * them follows every later change by itself. Returns the function that stops the calls.
   */
  watchClipping(listener: (planes: Plane[], ghostPlanes: Plane[], ghost: boolean) => void): () => void;
  dispose(): void;
}

/** A part surface's material: flat shaded, pushed back a little so edges drawn on it do not z-fight. */
export const surfaceMaterial = (side: Side) =>
  new MeshStandardMaterial({
    side,
    flatShading: true,
    roughness: 0.8,
    metalness: 0,
    // Pushes surfaces back a little so edges drawn on them do not z-fight.
    polygonOffset: true,
    polygonOffsetFactor: 1,
    polygonOffsetUnits: 1,
  });

export function createPartsLayer(theme: VpTheme): PartsLayer {
  const group = new Group();
  const views = new Map<string, PartView>();
  // Keyed by MeshData so duplicates and transform-only updates reuse one geometry. Entries outlive
  // their GPU buffers while the MeshData is alive (an undo can bring the part back).
  const entries = new WeakMap<MeshData, GeometryEntry>();
  const shared = {
    back: surfaceMaterial(BackSide),
    /** Back faces of see-through parts while a plane cuts them, so the cut shows red there too. */
    backSeeThrough: surfaceMaterial(BackSide),
    openEdge: new LineBasicMaterial(),
    featureEdge: new LineBasicMaterial(),
    pick: new MeshBasicMaterial({ side: DoubleSide }),
  };
  const state = { theme, selection: new Set<string>(), shading: "shaded" as Shading, look: PLAIN_LOOK };
  const scratch = new Box3();
  const clipWatchers = new Set<(planes: Plane[], ghostPlanes: Plane[], ghost: boolean) => void>();
  // Every clipped material holds one of these two arrays, so setClipping() edits them in place.
  // three.js re-reads them each frame and recompiles a shader by itself when the plane count changes.
  const clip = {
    planes: [] as Plane[],
    /** The planes negated, with clipIntersection: keeps exactly the union of the removed half-spaces. */
    ghostPlanes: [] as Plane[],
    ghost: false,
    /** The planes as given, to test part boxes against. */
    specs: [] as PlaneSpec[],
  };
  for (const material of [shared.back, shared.backSeeThrough, shared.openEdge, shared.featureEdge]) {
    material.clippingPlanes = clip.planes;
  }
  setTransparent(shared.backSeeThrough, true);

  function acquire(mesh: MeshData): GeometryEntry {
    let entry = entries.get(mesh);
    if (!entry) {
      // The attributes wrap the store's typed arrays directly; nothing is copied.
      const surface = new BufferGeometry();
      surface.setAttribute("position", new BufferAttribute(mesh.positions, 3));
      surface.setIndex(new BufferAttribute(mesh.indices, 1));
      surface.computeBoundingBox();
      surface.computeBoundingSphere();
      // Imported meshes arrive with their problem edges from Rust; computing them here would stall big imports.
      const edges = cachedProblemEdges(mesh) ?? problemEdgePositions(mesh.positions, mesh.indices);
      const openEdges = segmentGeometry(edges);
      entry = { mesh, surface, openEdges, featureEdges: null, users: 0 };
      entries.set(mesh, entry);
    }
    entry.users++;
    return entry;
  }

  function release(entry: GeometryEntry) {
    entry.users--;
    if (entry.users > 0) return;
    // Frees GPU buffers only; three.js re-uploads them if the geometry is used again.
    entry.surface.dispose();
    entry.openEdges.dispose();
    entry.featureEdges?.dispose();
  }

  function createView(part: Part): PartView {
    const entry = acquire(part.mesh);
    const root = new Group();
    root.matrixAutoUpdate = false;
    const front = new Mesh(entry.surface, surfaceMaterial(FrontSide));
    front.material.clippingPlanes = clip.planes;
    const back = new Mesh(entry.surface, shared.back);
    const wire = new Mesh(entry.surface, new MeshBasicMaterial({ wireframe: true, clippingPlanes: clip.planes }));
    const openEdges = new LineSegments(entry.openEdges, shared.openEdge);
    openEdges.renderOrder = 1; // drawn over feature edges along the same segment
    const pick = new Mesh(entry.surface, shared.pick);
    pick.visible = false;
    pick.userData.partId = part.id;
    for (const child of [front, back, wire, openEdges, pick]) child.matrixAutoUpdate = false;
    root.add(front, back, wire, openEdges, pick);
    group.add(root);
    return { part, entry, root, front, back, wire, openEdges, featureEdges: null, pick, ghost: null, redInside: true };
  }

  function swapMesh(view: PartView, mesh: MeshData) {
    const entry = acquire(mesh);
    release(view.entry);
    view.entry = entry;
    for (const m of [view.front, view.back, view.wire, view.pick, view.ghost]) if (m) m.geometry = entry.surface;
    view.openEdges.geometry = entry.openEdges;
    if (view.featureEdges) {
      view.root.remove(view.featureEdges);
      view.featureEdges = null;
    }
  }

  /** Feature edges are costly on big meshes, so they are built the first time "shaded-edges" needs them. */
  function addFeatureEdges(view: PartView) {
    const entry = view.entry;
    entry.featureEdges ??= segmentGeometry(featureEdgePositions(entry.mesh.positions, entry.mesh.indices, FEATURE_EDGE_ANGLE));
    const lines = new LineSegments(entry.featureEdges, shared.featureEdge);
    lines.matrixAutoUpdate = false;
    view.root.add(lines);
    view.featureEdges = lines;
  }

  const colorOf = (part: Part) => state.look.colors.get(part.id) ?? (state.selection.has(part.id) ? state.theme.selected : part.color);
  /** A part the mode does not draw in its own colours, while it dims the others. */
  const isDimmed = (part: Part) => state.look.dimOthers && !state.look.colors.has(part.id);
  const isSeeThrough = (part: Part) => part.transparent || isDimmed(part);

  /**
   * A see-through part shows its far walls in its own colour rather than as red back faces, except while
   * a plane cuts it (leaves part of its world box on the removed side): then its back faces turn
   * see-through red so the cut stays visible. The box is cached per Part object, so a slider drag only
   * checks box corners; hidden parts are skipped rather than measured.
   */
  function showsRedInside(part: Part): boolean {
    return !isSeeThrough(part) || (clip.specs.length > 0 && part.visible && isBoxCut(clip.specs, partBounds(part)));
  }

  /** Shows or hides the ghost of the clipped-away region, creating it on first use. */
  function applyGhost(view: PartView) {
    const shown = clip.ghost && clip.planes.length > 0;
    if (!shown && !view.ghost) return;
    if (!view.ghost) {
      const ghost = new Mesh(
        view.entry.surface,
        new MeshStandardMaterial({
          side: DoubleSide,
          flatShading: true,
          roughness: 0.8,
          metalness: 0,
          transparent: true,
          opacity: GHOST_OPACITY,
          depthWrite: false,
          clippingPlanes: clip.ghostPlanes,
          clipIntersection: true,
        }),
      );
      ghost.matrixAutoUpdate = false;
      view.root.add(ghost);
      view.ghost = ghost;
    }
    view.ghost.visible = shown;
    view.ghost.material.color.set(colorOf(view.part));
    setWireframe(view.ghost.material, state.shading === "wireframe");
  }

  function applyPart(view: PartView, part: Part) {
    if (part.mesh !== view.part.mesh) swapMesh(view, part.mesh);
    view.part = part;
    view.root.matrix.fromArray(part.matrix);
    view.root.matrixWorldNeedsUpdate = true;
    view.root.visible = part.visible && part.id !== state.look.hidden;

    const color = colorOf(part);
    const dimmed = isDimmed(part);
    const seeThrough = isSeeThrough(part);
    const opacity = dimmed ? DIMMED_OPACITY : TRANSPARENT_OPACITY;
    view.front.material.color.set(color);
    view.wire.material.color.set(color);
    setTransparent(view.front.material, seeThrough, opacity);
    setTransparent(view.wire.material, seeThrough, opacity);
    view.redInside = showsRedInside(part);
    setSide(view.front.material, view.redInside ? FrontSide : DoubleSide);
    view.back.material = seeThrough ? shared.backSeeThrough : shared.back;
    // See-through back faces are blended before any see-through front face, so far walls sit behind near ones.
    view.back.renderOrder = dimmed ? DIMMED_ORDER - 1 : seeThrough ? -1 : 0;
    view.front.renderOrder = view.wire.renderOrder = dimmed ? DIMMED_ORDER : 0;

    const shaded = state.shading !== "wireframe";
    view.front.visible = shaded;
    view.back.visible = shaded && view.redInside;
    view.wire.visible = !shaded;
    // Dimmed parts are only context for the mode: their problem and feature edges stay out of the way.
    view.openEdges.visible = !dimmed;
    const edges = state.shading === "shaded-edges" && !dimmed;
    if (edges && !view.featureEdges) addFeatureEdges(view);
    if (view.featureEdges) view.featureEdges.visible = edges;
    applyGhost(view);
  }

  function removeView(view: PartView) {
    group.remove(view.root);
    view.front.material.dispose();
    view.wire.material.dispose();
    view.ghost?.material.dispose();
    release(view.entry);
  }

  function applyTheme(t: VpTheme) {
    state.theme = t;
    shared.back.color.set(t.backface);
    shared.backSeeThrough.color.set(t.backface);
    shared.openEdge.color.set(t.openEdge);
    shared.featureEdge.color.set(t.edge);
    for (const view of views.values()) applyPart(view, view.part);
  }
  applyTheme(theme);

  return {
    group,
    applyTheme,

    sync(parts, selection, shading, look) {
      state.selection = new Set(selection);
      state.shading = shading;
      state.look = look;
      const seen = new Set<string>();
      for (const part of parts) {
        seen.add(part.id);
        let view = views.get(part.id);
        if (!view) {
          view = createView(part);
          views.set(part.id, view);
        }
        applyPart(view, part);
      }
      for (const [id, view] of views) {
        if (seen.has(id)) continue;
        removeView(view);
        views.delete(id);
      }
    },

    pickTargets() {
      const out: Object3D[] = [];
      const pickable = state.look.pickable;
      for (const view of views.values()) if (view.root.visible && (!pickable || pickable.has(view.part.id))) out.push(view.pick);
      return out;
    },

    visibleBox(out) {
      out.makeEmpty();
      for (const view of views.values()) {
        const box = view.entry.surface.boundingBox;
        if (!view.root.visible || !box) continue;
        out.union(scratch.copy(box).applyMatrix4(view.root.matrix));
      }
      return out;
    },

    setClipping(planes, ghost) {
      clip.specs = [...planes];
      copyPlanes(clip.planes, planes, false);
      copyPlanes(clip.ghostPlanes, planes, true);
      clip.ghost = ghost;
      // A see-through part that a plane starts or stops cutting changes how it draws its back faces.
      for (const view of views.values()) {
        if (showsRedInside(view.part) !== view.redInside) applyPart(view, view.part);
        else applyGhost(view);
      }
      for (const listener of clipWatchers) listener(clip.planes, clip.ghostPlanes, clip.ghost);
    },

    watchClipping(listener) {
      clipWatchers.add(listener);
      listener(clip.planes, clip.ghostPlanes, clip.ghost);
      return () => clipWatchers.delete(listener);
    },

    dispose() {
      clipWatchers.clear();
      for (const view of views.values()) removeView(view);
      views.clear();
      for (const material of Object.values(shared)) material.dispose();
    },
  };
}

/** Line-segment geometry over pairs of points (x, y, z, x, y, z, ...). */
function segmentGeometry(positions: Float32Array): BufferGeometry {
  const geometry = new BufferGeometry();
  geometry.setAttribute("position", new BufferAttribute(positions, 3));
  return geometry;
}

function setTransparent(material: Material, transparent: boolean, opacity = TRANSPARENT_OPACITY) {
  // Opacity is a uniform: changing it needs no new shader.
  material.opacity = transparent ? opacity : 1;
  if (material.transparent === transparent) return;
  material.transparent = transparent;
  material.depthWrite = !transparent;
  material.needsUpdate = true;
}

function setWireframe(material: Material & { wireframe: boolean }, wireframe: boolean) {
  if (material.wireframe === wireframe) return;
  material.wireframe = wireframe;
  material.needsUpdate = true; // flat shading is compiled in only for surfaces
}

/** Makes `out` the three.js form of `planes` (negated when asked), reusing its Plane objects. */
function copyPlanes(out: Plane[], planes: readonly PlaneSpec[], negate: boolean) {
  out.length = planes.length;
  for (let i = 0; i < planes.length; i++) {
    const plane = (out[i] ??= new Plane());
    plane.normal.fromArray(planes[i].normal);
    plane.constant = planes[i].constant;
    if (negate) plane.negate();
  }
}

function setSide(material: Material, side: Side) {
  if (material.side === side) return;
  material.side = side;
  material.needsUpdate = true;
}
