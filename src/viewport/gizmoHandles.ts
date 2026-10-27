import {
  BufferGeometry,
  Color,
  ConeGeometry,
  CylinderGeometry,
  DoubleSide,
  Float32BufferAttribute,
  FrontSide,
  Group,
  LineBasicMaterial,
  LineLoop,
  Matrix4,
  Mesh,
  MeshBasicMaterial,
  PlaneGeometry,
  SphereGeometry,
  SRGBColorSpace,
  TorusGeometry,
  Vector3,
  type Intersection,
  type Material,
  type Object3D,
  type Raycaster,
  type Side,
} from "three";
import { mergeGeometries } from "three/examples/jsm/utils/BufferGeometryUtils.js";
import type { Axis } from "../matrix";
import { handleVisible, planeAxes, sameHandle, type Handle } from "./gizmoMath";
import { axisColor, type VpTheme } from "./theme";
import type { SceneLayer } from "./util";

/** Arrow length on screen, CSS px. Every handle size below is a fraction of it. */
export const GIZMO_PX = 96;
/** Rotation arcs' radius, in arrow lengths. */
export const ARC_RADIUS = 0.64;

/** A rotate drag's turn so far: radians about the world axis, counter-clockwise looking from + down it (DragResult.angle). */
export interface ArcTurn {
  axis: Axis;
  angle: number;
}

export interface GizmoHandles extends SceneLayer {
  /**
   * Places the gizmo for the frame about to be drawn: at `centre`, `scale` world mm per arrow length, every handle on the
   * camera's side `signs` (facingSigns) of each axis, end-on arrows and edge-on squares and arcs hidden for `viewDir`
   * (handleVisible). `turn` turns that axis's arc, with its ball and picker, about its world axis after the mirror, so
   * during a rotate drag the grabbed ball goes round with the pointer; without it every arc is at rest.
   */
  layout(centre: Vector3, scale: number, viewDir: Vector3, signs: readonly number[], turn?: ArcTurn | null): void;
  /**
   * The visible handle under the ray and the world point hit: what is drawn there, so a press grabs what the pointer
   * is seen over. Of the drawn pieces hit, the one drawn on top wins (the highest ORDER, the nearest of equals). Only
   * where nothing is drawn, in the margin round the thin pieces, the pickers decide by RANK: centre ball, arc balls,
   * arrows, squares, arcs; nearest within a rank.
   */
  pick(raycaster: Raycaster): { handle: Handle; point: Vector3 } | null;
  /** Lights `hovered`; while `active` (a drag) is set it stays lit and every other handle fades. */
  highlight(hovered: Handle | null, active: Handle | null): void;
}

// Sizes in arrow lengths: layout() scales the whole group to `scale` mm per arrow length.

const CENTRE_RADIUS = 0.08;
/** A slim shaft from clear of the centre ball to the tip's base, where it stops so shaft and tip never overlap (see SOLID). */
const SHAFT = { radius: 0.016, from: 0.16, to: 0.78 };
const TIP = { radius: 0.055, from: 0.78, to: 1 };
/** Plane squares sit inside the arcs and away from the arrows, so all three kinds can be grabbed apart. */
const SQUARE = { from: 0.2, to: 0.36 };
const ARC_TUBE = 0.014;
/** The arcs run from 8° to 82° of their quadrant: their ends stay clear of the arrows on either side. */
const ARC_FROM = (8 * Math.PI) / 180;
const ARC_TO = (82 * Math.PI) / 180;
const ARC_BALL_RADIUS = 0.055;
/** A lit handle's shaft, tip, tube and balls draw this much thicker. */
const THICKEN = 1.3;

/**
 * Pickers: invisible, fatter stand-ins round each handle, so a 3 px shaft or tube can be grabbed without
 * pixel hunting. pick() only turns to them where no drawn piece is under the pointer: they reach past the
 * drawn pieces, over their neighbours too (an arc ball's over the middle of the square in its plane in many
 * oblique views), and must not take a press from a handle drawn there. They never grow with the lit look.
 * Squares' pickers reach `square` past each edge.
 */
const PICK = { centre: 0.11, arcBall: 0.1, arrow: 0.07, arrowFrom: 0.11, arrowTo: 1.04, square: 0.02, arc: 0.06 };

/**
 * Draw order: above the selection brackets (10, 11), and the small handles over the big ones, so no ball ends up under a
 * shaft. With the depth test off it alone decides what shows on top, so pick() goes by it among the drawn pieces hit.
 */
const ORDER = { square: 20, arc: 21, arrow: 22, arcBall: 23, centre: 24 } as const;
/**
 * Which picker wins when several are under the pointer and no drawn piece is: the small, specific handles first, the
 * long arcs last.
 */
const RANK = { centre: 0, arcBall: 1, arrow: 2, square: 3, arc: 4 } as const;

type Look = "idle" | "lit" | "faded";

/**
 * Opacity per look. At rest the handles let a little of the part behind them through; a lit handle is
 * solid; during a drag the others step back. A handle's own pieces do not run into each other (shaft and
 * tip meet end to end, the arc stops at its ball), so seen side-on no band shows where one blends over another.
 */
const SOLID: Record<Look, number> = { idle: 0.85, lit: 1, faded: 0.2 };
/** Squares' fill is a wash at rest: it reads over the selected part under it without hiding that part. */
const FILL: Record<Look, number> = { idle: 0.5, lit: 0.8, faded: 0.1 };

/**
 * sRGB lightness shift of a lit handle's colour: lighter over a dark view, deeper over a light one. A
 * fixed lift would wash the grey centre ball out against the light theme's pale background; this way the
 * lit handle gains contrast in both, on top of turning solid and thicker.
 */
const LIFT = { dark: 0.12, light: -0.1 };

const AXIS_NAMES = ["X", "Y", "Z"] as const;

/** A handle's material and how see-through it is in each look. */
interface Paint {
  material: MeshBasicMaterial | LineBasicMaterial;
  opacity: Record<Look, number>;
}

/** One handle's scene objects. */
interface HandleView {
  readonly handle: Handle;
  /**
   * Holds the drawn pieces and the pickers, in world-axis coordinates around the centre. Hidden with the
   * handle. Its scale mirrors it to the camera's side: an arrow along its axis, a square or arc across its
   * two in-plane axes. An arc's quaternion turns it with a rotate drag.
   */
  readonly object: Group;
  readonly paints: Paint[];
  /** Theme colour, and the same lifted for the lit look; set by applyTheme. */
  readonly base: Color;
  readonly lit: Color;
  /** Draws the visible pieces thicker for the lit look. */
  thicken(on: boolean): void;
}

interface Picker {
  view: HandleView;
  rank: number;
}

const unit = (axis: Axis) => new Vector3().setComponent(axis, 1);

/**
 * Moves a geometry modelled with local +Z as `axis` onto the world axes: local X becomes the first of
 * planeAxes(axis), Y the second, Z the axis itself. Right-handed (u × v = axis), so nothing mirrors.
 */
function onAxis<T extends BufferGeometry>(geometry: T, axis: Axis): T {
  const [u, v] = planeAxes(axis);
  return geometry.applyMatrix4(new Matrix4().makeBasis(unit(u), unit(v), unit(axis)));
}

/** A rod along local +Z from `from` to `to` (CylinderGeometry stands on Y; turning it +90° about X stands it on Z). */
function rod(radius: number, from: number, to: number, segments: number): CylinderGeometry {
  return new CylinderGeometry(radius, radius, to - from, segments).rotateX(Math.PI / 2).translate(0, 0, (from + to) / 2);
}

/** A cone along local +Z, its base at `from` and its point at `to`. */
function cone(radius: number, from: number, to: number): ConeGeometry {
  return new ConeGeometry(radius, to - from, 20).rotateX(Math.PI / 2).translate(0, 0, (from + to) / 2);
}

/** A square in the local XY plane spanning `from`..`to` on both axes: the (+, +) quadrant. */
function square(from: number, to: number): PlaneGeometry {
  const mid = (from + to) / 2;
  return new PlaneGeometry(to - from, to - from).translate(mid, mid, 0);
}

/** The corners of square(from, to) in order, for a LineLoop. */
function squareOutline(from: number, to: number): BufferGeometry {
  const corners = [from, from, 0, to, from, 0, to, to, 0, from, to, 0];
  return new BufferGeometry().setAttribute("position", new Float32BufferAttribute(corners, 3));
}

/** A tube `thickness` thick along the circle of ARC_RADIUS in the local XY plane, from angle `from` to `to` (radians from +X towards +Y). */
function arcTube(thickness: number, from: number, to: number, radialSegments: number, tubularSegments: number): TorusGeometry {
  return new TorusGeometry(ARC_RADIUS, thickness, radialSegments, tubularSegments, to - from).rotateZ(from);
}

/**
 * The drawn arc: two tubes from ARC_FROM and ARC_TO that stop where they reach the grip ball of
 * `ballRadius` at the middle. Run on through the see-through ball, the tube would show as a band across it.
 */
function grippedArc(thickness: number, ballRadius: number): BufferGeometry {
  // A point on the arc δ from the middle is the chord 2R·sin(δ/2) from the ball's centre.
  const gap = 2 * Math.asin(ballRadius / (2 * ARC_RADIUS));
  const middle = (ARC_FROM + ARC_TO) / 2;
  const halves = [arcTube(thickness, ARC_FROM, middle - gap, 8, 16), arcTube(thickness, middle + gap, ARC_TO, 8, 16)];
  // Two tori have the same attributes, so the merge cannot fail.
  const merged = mergeGeometries(halves)!;
  for (const half of halves) half.dispose();
  return merged;
}

/** "XY" for a square with normal Z: its two in-plane axes, in X, Y, Z order. */
function planeName(normal: Axis): string {
  return AXIS_NAMES.filter((_, a) => a !== normal).join("");
}

/**
 * The combined move and rotate gizmo's three.js objects: a grey centre ball (move in the screen plane),
 * three arrows (move along an axis), three plane squares (move in a plane) and three quarter arcs with a
 * ball on each (turn about an axis). Everything faces the camera: each arrow points to the camera's side
 * of its axis, and the squares and arcs sit in the quadrant between two arrows. So the arrows run along
 * the edges of the octant facing the viewer and the squares and arcs lie on its faces, which keeps them
 * apart on screen; only a square or arc seen nearly edge-on lies along an arrow, or a square under the ball
 * of the arc in its plane (a press takes whichever is drawn on top there, see pick()).
 * The readouts stay signed by the world axes all the same (dragging towards a −Y arrow's tip reads
 * "Y −12 mm"). Everything is world-aligned and drawn without depth test over the scene.
 * No events or store here: gizmo.ts lays it out each frame, picks with its pointer rays and drives the
 * highlight. Starts hidden; the caller shows `group` once there is a selection to lay it out on, and
 * pick() finds nothing while it is hidden.
 */
export function createGizmoHandles(theme: VpTheme): GizmoHandles {
  const geometries: BufferGeometry[] = [];
  const materials: Material[] = [];
  const pickers = new Map<Object3D, Picker>();
  /**
   * The drawn meshes and whose they are, for pick(). The squares' outlines are left out: a 1 px line has nothing to
   * press, and the fill under it stands for the square. Mesh.raycast tests the triangles in the mesh's own space, where
   * a mirrored (negative-scale) piece is not mirrored, so a closed FrontSide piece is hit where the ray enters it: on its
   * side facing the camera, as drawn. Into an arc tube's open end the ray finds nothing, and nothing is drawn there
   * either (its inside faces are culled). The square's fill is DoubleSide.
   */
  const drawnPieces = new Map<Object3D, HandleView>();
  /** Never drawn (pickers are invisible), but the raycast reads its side: both faces count. */
  const pickMaterial = keepMaterial(new MeshBasicMaterial({ side: DoubleSide }));
  // Spheres are the same for every arc, so the three arcs share them.
  const arcBall = keep(new SphereGeometry(ARC_BALL_RADIUS, 16, 12));
  const arcBallPicker = keep(new SphereGeometry(PICK.arcBall, 12, 8));
  let hovered: Handle | null = null;
  let active: Handle | null = null;

  function keep<T extends BufferGeometry>(geometry: T): T {
    geometries.push(geometry);
    return geometry;
  }

  function keepMaterial<T extends Material>(material: T): T {
    materials.push(material);
    return material;
  }

  /** A handle colour. Opacity and colour are set by the look (paint), so they start unset. */
  function solid(side: Side = FrontSide): MeshBasicMaterial {
    return keepMaterial(new MeshBasicMaterial({ side, transparent: true, depthTest: false, depthWrite: false, toneMapped: false }));
  }

  /** A drawn piece: in its slot of the draw order, and never culled, since the group moves and rescales every frame. */
  function piece<T extends Mesh | LineLoop>(object: T, order: number): T {
    object.renderOrder = order;
    object.frustumCulled = false;
    return object;
  }

  function handleView(handle: Handle, name: string, pieces: Object3D[], paints: Paint[], thicken: (on: boolean) => void): HandleView {
    const object = new Group();
    object.name = name;
    object.add(...pieces);
    const view = { handle, object, paints, base: new Color(), lit: new Color(), thicken };
    for (const p of pieces) if (p instanceof Mesh) drawnPieces.set(p, view);
    return view;
  }

  /** An invisible picker for `view` in its object: raycasting ignores `visible`, drawing does not. */
  function addPicker(view: HandleView, geometry: BufferGeometry, rank: number): Mesh {
    const mesh = new Mesh(geometry, pickMaterial);
    mesh.visible = false;
    view.object.add(mesh);
    pickers.set(mesh, { view, rank });
    return mesh;
  }

  function centreBall(): HandleView {
    const material = solid();
    const ball = piece(new Mesh(keep(new SphereGeometry(CENTRE_RADIUS, 24, 16)), material), ORDER.centre);
    const view = handleView({ kind: "free" }, "centre ball", [ball], [{ material, opacity: SOLID }], (on) => {
      ball.scale.setScalar(on ? THICKEN : 1);
    });
    addPicker(view, keep(new SphereGeometry(PICK.centre, 12, 8)), RANK.centre);
    return view;
  }

  function arrow(axis: Axis): HandleView {
    const material = solid();
    const shaft = piece(new Mesh(keep(onAxis(rod(SHAFT.radius, SHAFT.from, SHAFT.to, 12), axis)), material), ORDER.arrow);
    const tip = piece(new Mesh(keep(onAxis(cone(TIP.radius, TIP.from, TIP.to), axis)), material), ORDER.arrow);
    const view = handleView({ kind: "axis", axis }, `${AXIS_NAMES[axis]} arrow`, [shaft, tip], [{ material, opacity: SOLID }], (on) => {
      // Fatter across the axis only, so the tip stays at the arrow's end.
      for (const mesh of [shaft, tip]) mesh.scale.setScalar(on ? THICKEN : 1).setComponent(axis, 1);
    });
    addPicker(view, keep(onAxis(rod(PICK.arrow, PICK.arrowFrom, PICK.arrowTo, 8), axis)), RANK.arrow);
    return view;
  }

  /** The square in the plane with normal `axis`: a wash with a crisp outline in the normal axis's colour. */
  function planeSquare(axis: Axis): HandleView {
    const fillMaterial = solid(DoubleSide);
    const lineMaterial = keepMaterial(new LineBasicMaterial({ transparent: true, depthTest: false, depthWrite: false, toneMapped: false }));
    const fill = piece(new Mesh(keep(onAxis(square(SQUARE.from, SQUARE.to), axis)), fillMaterial), ORDER.square);
    // Its own outline goes over the fill.
    const outline = piece(new LineLoop(keep(onAxis(squareOutline(SQUARE.from, SQUARE.to), axis)), lineMaterial), ORDER.square + 0.5);
    const paints = [
      { material: fillMaterial, opacity: FILL },
      { material: lineMaterial, opacity: SOLID },
    ];
    // Lines draw 1 px wide whatever the width, so the square is lit by colour and opacity alone.
    const view = handleView({ kind: "plane", axis }, `${planeName(axis)} square`, [fill, outline], paints, () => {});
    addPicker(view, keep(onAxis(square(SQUARE.from - PICK.square, SQUARE.to + PICK.square), axis)), RANK.square);
    return view;
  }

  /** The quarter arc for turning about `axis`, in its rotation plane, with a round grip at 45°. */
  function rotationArc(axis: Axis): HandleView {
    const material = solid();
    const thin = keep(onAxis(grippedArc(ARC_TUBE, ARC_BALL_RADIUS), axis));
    const thick = keep(onAxis(grippedArc(ARC_TUBE * THICKEN, ARC_BALL_RADIUS * THICKEN), axis));
    const tube = piece(new Mesh(thin, material), ORDER.arc);
    const ball = piece(new Mesh(arcBall, material), ORDER.arcBall);
    const [u, v] = planeAxes(axis);
    const at = ARC_RADIUS * Math.SQRT1_2;
    ball.position.setComponent(u, at).setComponent(v, at);
    const view = handleView({ kind: "rotate", axis }, `${AXIS_NAMES[axis]} arc`, [tube, ball], [{ material, opacity: SOLID }], (on) => {
      // Scaling a torus would widen the arc too, so the lit look swaps in a fatter tube instead.
      tube.geometry = on ? thick : thin;
      ball.scale.setScalar(on ? THICKEN : 1);
    });
    // One picker along the whole arc: the ball's own picker outranks it around the grip.
    addPicker(view, keep(onAxis(arcTube(PICK.arc, ARC_FROM, ARC_TO, 6, 16), axis)), RANK.arc);
    addPicker(view, arcBallPicker, RANK.arcBall).position.copy(ball.position);
    return view;
  }

  const axes: Axis[] = [0, 1, 2];
  // Order is only for reading the scene graph; drawing follows ORDER, and so does picking (RANK only off the drawn pieces).
  const views = [centreBall(), ...axes.map(arrow), ...axes.map(planeSquare), ...axes.map(rotationArc)];

  const group = new Group();
  group.name = "move and rotate gizmo";
  group.add(...views.map((v) => v.object));
  group.visible = false;

  function paint(view: HandleView, look: Look) {
    for (const { material, opacity } of view.paints) {
      material.opacity = opacity[look];
      material.color.copy(look === "lit" ? view.lit : view.base);
    }
    view.thicken(look === "lit");
  }

  function repaint() {
    for (const view of views) {
      const look: Look = active ? (sameHandle(view.handle, active) ? "lit" : "faded") : sameHandle(view.handle, hovered) ? "lit" : "idle";
      paint(view, look);
    }
  }

  const hsl = { h: 0, s: 0, l: 0 };

  function applyTheme(t: VpTheme) {
    // HSL in sRGB, the space the theme's hex values are written in; the working space is linear.
    const lift = new Color(t.surface).getHSL(hsl, SRGBColorSpace).l < 0.5 ? LIFT.dark : LIFT.light;
    for (const view of views) {
      view.base.set(view.handle.kind === "free" ? t.gizmoCentre : axisColor(t, view.handle.axis));
      view.base.getHSL(hsl, SRGBColorSpace);
      view.lit.setHSL(hsl.h, hsl.s, hsl.l + lift, SRGBColorSpace);
    }
    repaint();
  }
  applyTheme(theme);

  return {
    group,
    applyTheme,

    layout(centre, scale, viewDir, signs, turn = null) {
      group.position.copy(centre);
      group.scale.setScalar(scale);
      for (const view of views) {
        const { handle, object } = view;
        object.visible = handleVisible(handle, viewDir);
        if (handle.kind === "free") continue;
        if (handle.kind === "axis") {
          // Modelled pointing to +; a −1 turns it round to point the other way.
          object.scale.setComponent(handle.axis, signs[handle.axis]);
          continue;
        }
        // Modelled in the (+, +) quadrant of their plane; a −1 on an in-plane axis mirrors them across it.
        const [u, v] = planeAxes(handle.axis);
        object.scale.setComponent(u, signs[u]).setComponent(v, signs[v]);
        // An object scales before it rotates, so the arc turns from where the mirror put it, as the parts do.
        if (handle.kind === "rotate") {
          if (turn?.axis === handle.axis) object.quaternion.setFromAxisAngle(unit(handle.axis), turn.angle);
          else object.quaternion.identity();
        }
      }
      // A pick can come before the next frame brings the world matrices up to date.
      group.updateMatrixWorld(true);
    },

    pick(raycaster) {
      if (!group.visible) return null;
      // Hits come nearest first, so the first hit of the highest order (below: of the best rank) is the nearest of those.
      // What is drawn first, as it is drawn now: a lit handle's fatter pieces count, a hidden handle's do not.
      const shown = [...drawnPieces].filter(([mesh, view]) => mesh.visible && view.object.visible).map(([mesh]) => mesh);
      let top: Intersection | null = null;
      for (const hit of raycaster.intersectObjects(shown, false)) if (!top || hit.object.renderOrder > top.object.renderOrder) top = hit;
      if (top) return { handle: drawnPieces.get(top.object)!.handle, point: top.point };

      // Nothing drawn under the ray: the pickers' margin round the thin pieces.
      const targets = [...pickers].filter(([, p]) => p.view.object.visible).map(([mesh]) => mesh);
      let best: { picker: Picker; point: Vector3 } | null = null;
      for (const hit of raycaster.intersectObjects(targets, false)) {
        const picker = pickers.get(hit.object)!;
        if (!best || picker.rank < best.picker.rank) best = { picker, point: hit.point };
      }
      return best && { handle: best.picker.view.handle, point: best.point };
    },

    highlight(nextHovered, nextActive) {
      hovered = nextHovered;
      active = nextActive;
      repaint();
      // The lit look resizes pieces, and pick() goes by them: as in layout(), a pick can come before the next frame.
      group.updateMatrixWorld(true);
    },

    dispose() {
      for (const geometry of geometries) geometry.dispose();
      for (const material of materials) material.dispose();
    },
  };
}
