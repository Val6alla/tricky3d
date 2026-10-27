import { Matrix4, OrthographicCamera, PerspectiveCamera, Quaternion, Ray, Raycaster, Vector2, Vector3 } from "three";
import { describe, expect, it } from "vitest";
import { rotationAbout, transformPoint, translationMatrix, type Axis } from "../matrix";
import { makeBox, translation } from "../mesh";
import type { Part, Vec3 } from "../store";
import { applyView, createCameras, directionOf, PRESETS, rayFromCamera, setAspect, type ViewDirection } from "./cameras";
import {
  dragLabel,
  dragStep,
  EDGE_ON,
  END_ON,
  facingSigns,
  groupMatrices,
  handleVisible,
  lineOfSight,
  pivotOf,
  planeAxes,
  sameHandle,
  SNAP,
  startDrag,
  transformableParts,
  worldPerPixel,
  type DragResult,
  type GizmoDrag,
  type Handle,
} from "./gizmoMath";

function cube(id: string, x: number, y: number, z: number, extra: Partial<Part> = {}): Part {
  return {
    id,
    name: id,
    mesh: makeBox([10, 10, 10]),
    matrix: translation(x, y, z),
    visible: true,
    transparent: false,
    locked: false,
    color: "#aaaaaa",
    ...extra,
  };
}

const origin = (m: number[]) => [m[12], m[13], m[14]].map((v) => +v.toFixed(9));

describe("transformableParts", () => {
  it("keeps selected parts that are visible and unlocked", () => {
    const parts = [cube("a", 0, 0, 0), cube("b", 0, 0, 0, { locked: true }), cube("c", 0, 0, 0, { visible: false }), cube("d", 0, 0, 0)];
    expect(transformableParts({ parts, selection: ["a", "b", "c"] }).map((p) => p.id)).toEqual(["a"]);
  });
});

describe("pivotOf", () => {
  it("is the centre of the combined world box", () => {
    expect(pivotOf([cube("a", 0, 0, 5), cube("b", 30, 10, 5)])).toEqual([15, 5, 5]);
  });
  it("is null without parts", () => {
    expect(pivotOf([])).toBeNull();
  });
});

describe("groupMatrices", () => {
  const parts = [cube("a", 0, 0, 5), cube("b", 30, 0, 5)];
  const starts = new Map(parts.map((p) => [p.id, new Matrix4().fromArray(p.matrix)]));
  const centre = new Vector3(15, 0, 5);
  const pivotStart = new Matrix4().makeTranslation(centre.x, centre.y, centre.z);
  const pivotStartInverse = pivotStart.clone().invert();

  it("moves every part by the pivot's offset", () => {
    const now = new Matrix4().makeTranslation(18, -2, 5);
    const out = groupMatrices(pivotStartInverse, now, starts);
    expect(origin(out.get("a")!)).toEqual([3, -2, 5]);
    expect(origin(out.get("b")!)).toEqual([33, -2, 5]);
  });

  it("turns the group about its combined centre", () => {
    const now = new Matrix4().compose(centre, new Quaternion().setFromAxisAngle(new Vector3(0, 0, 1), Math.PI / 2), new Vector3(1, 1, 1));
    const out = groupMatrices(pivotStartInverse, now, starts);
    // a sat 15 mm left of the centre; a quarter turn about Z puts it 15 mm in front (-Y).
    expect(origin(out.get("a")!)).toEqual([15, -15, 5]);
    expect(origin(out.get("b")!)).toEqual([15, 15, 5]);
  });

  it("scales the group about its combined centre", () => {
    const now = new Matrix4().compose(centre, new Quaternion(), new Vector3(2, 2, 2));
    const out = groupMatrices(pivotStartInverse, now, starts);
    expect(origin(out.get("a")!)).toEqual([-15, 0, 5]);
    expect(out.get("a")![0]).toBe(2);
  });

  it("leaves parts as they are when the pivot has not moved", () => {
    const out = groupMatrices(pivotStartInverse, pivotStart, starts);
    expect(out.get("a")).toEqual(parts[0].matrix);
    expect(out.get("b")).toEqual(parts[1].matrix);
  });
});

// Combined gizmo drags. The gizmo sits at CENTRE; rays are built by hand, so each test knows exactly
// where the pointer meets the handle's line or plane.

const AXES: Axis[] = [0, 1, 2];
const NAMES = ["X", "Y", "Z"];
const PLANES = ["YZ", "XZ", "XY"];
const CENTRE = new Vector3(10, 20, 5);
const unit = (a: Axis) => new Vector3().setComponent(a, 1);
const deg = (d: number) => (d * Math.PI) / 180;

/**
 * [x, y, z] with x moved onto axis `a` (and y, z onto the next two in turn), so a case written for X
 * fits Y and Z. A cyclic shift of components keeps dot and cross products.
 */
function cycle([x, y, z]: number[], a: Axis): Vector3 {
  return new Vector3().setComponent(a, x).setComponent((a + 1) % 3, y).setComponent((a + 2) % 3, z);
}

/** A ray along `dir` that passes through `through`, starting 300 mm before it. */
function rayThrough(through: Vector3, dir: Vector3): Ray {
  const d = dir.clone().normalize();
  return new Ray(through.clone().addScaledVector(d, -300), d);
}

/** A ray from a perspective camera at `camera` through `target`. */
function rayFrom(camera: Vector3, target: Vector3): Ray {
  return new Ray(camera.clone(), target.clone().sub(camera).normalize());
}

/**
 * Starts a drag on the gizmo at CENTRE that must start. Only the centre ball reads `forward`; by default
 * the camera looks along the press ray, as an orthographic one does.
 */
function start(handle: Handle, ray: Ray, forward = ray.direction): GizmoDrag {
  const drag = startDrag(handle, CENTRE, ray, forward);
  if (!drag) throw new Error("the drag did not start");
  return drag;
}

function expectNear(actual: readonly number[], expected: readonly number[], digits = 9) {
  expect(actual).toHaveLength(expected.length);
  actual.forEach((v, i) => expect(v).toBeCloseTo(expected[i], digits));
}

/** True for +0 only: offsets must never come back as −0. */
const isZero = (v: number) => Object.is(v, 0);

// Real cameras, set up as the viewport sets them (cameras.ts), on a 1280 × 800 px canvas: these tests
// run the same rays, projections and preset tilts the gizmo sees.

const W = 1280;
const H = 800;

/** The perspective camera looking from `direction` at `target`, with `halfHeight` mm of view above and below it. */
function perspectiveAt(direction: ViewDirection, target: Vector3, halfHeight = 140): PerspectiveCamera {
  const cameras = createCameras();
  setAspect(cameras, W / H);
  const camera = cameras.perspective;
  applyView(camera, { target, ...direction, halfHeight }, new Vector3());
  // OrbitControls.update() does this in the app.
  camera.lookAt(target);
  camera.updateMatrixWorld();
  return camera;
}

/** Canvas CSS px of a world point, y down. */
function toPx(camera: PerspectiveCamera, p: Vector3): { x: number; y: number } {
  const q = p.clone().project(camera);
  return { x: ((q.x + 1) / 2) * W, y: ((1 - q.y) / 2) * H };
}

/** The pointer ray through canvas px (x, y), aimed as the gizmo aims it. */
function rayAt(camera: PerspectiveCamera, { x, y }: { x: number; y: number }): Ray {
  const raycaster = new Raycaster();
  rayFromCamera(raycaster, new Vector2((x / W) * 2 - 1, 1 - (y / H) * 2), camera);
  return raycaster.ray;
}

describe("axis move", () => {
  // The step rays pass the axis without meeting it. `miss` is square to the axis and to the ray, so it is
  // their common perpendicular: the ray comes closest to the axis at the point it passes `miss` away from.
  const MISS = [0, 3, -4];
  const PRESS_DIR = [2, 4, 3]; // square to MISS, oblique to every axis
  const STEP_DIR = [5, 4, 3]; // square to MISS too: the pointer ray has turned, as from a perspective camera

  for (const a of AXES) {
    it(`moves along ${NAMES[a]} only, to where an oblique ray comes closest to the axis`, () => {
      const e = unit(a);
      const miss = cycle(MISS, a);
      // Pressed on the shaft 30 mm out, 5 mm off the line; dragged to 42.5 mm out, 2 mm off the other way.
      const press = rayThrough(CENTRE.clone().addScaledVector(e, 30).add(miss), cycle(PRESS_DIR, a));
      const drag = start({ kind: "axis", axis: a }, press);
      const step = rayThrough(CENTRE.clone().addScaledVector(e, 42.5).addScaledVector(miss, -0.4), cycle(STEP_DIR, a));
      const r = dragStep(drag, step, false);
      expect(r.offset[a]).toBeCloseTo(12.5, 9);
      for (const b of AXES) if (b !== a) expect(isZero(r.offset[b])).toBe(true);
      expect(r.angle).toBe(0);
      expect(r.delta).toEqual(translationMatrix(r.offset));
    });
  }

  it("lands where a brute-force search finds the ray nearest the axis", () => {
    const press = rayFrom(new Vector3(150, -220, 130), CENTRE);
    const drag = start({ kind: "axis", axis: 1 }, press);
    const step = rayFrom(new Vector3(150, -220, 130), new Vector3(31, 47, -12));
    const t = dragStep(drag, step, false).offset[1];
    const gap = (s: number) => step.distanceSqToPoint(CENTRE.clone().addScaledVector(unit(1), s));
    // A minimum: any nudge along the axis takes the point further from the ray.
    expect(gap(t)).toBeLessThan(gap(t + 1e-3));
    expect(gap(t)).toBeLessThan(gap(t - 1e-3));
  });

  it("reads the world sign on an arrow turned to −: dragging out towards its tip is a negative move", () => {
    // Seen from the −X side the X arrow points to −X. Pressed 30 mm out on it, dragged 12.5 mm further out.
    const dir = new Vector3(1, 3, -2);
    const drag = start({ kind: "axis", axis: 0 }, rayThrough(CENTRE.clone().add(new Vector3(-30, 0, 0)), dir));
    const r = dragStep(drag, rayThrough(CENTRE.clone().add(new Vector3(-42.5, 0, 0)), dir), false);
    expect(r.offset[0]).toBeCloseTo(-12.5, 9);
    expect(dragLabel({ kind: "axis", axis: 0 }, r)).toBe("X −12.5 mm");
  });
});

describe("plane move", () => {
  for (const n of AXES) {
    it(`moves in the ${PLANES[n]} plane, its normal component exactly 0`, () => {
      const [u, v] = planeAxes(n);
      const onPlane = (pu: number, pv: number) => CENTRE.clone().addScaledVector(unit(u), pu).addScaledVector(unit(v), pv);
      const drag = start({ kind: "plane", axis: n }, rayThrough(onPlane(25, 25), cycle([3, 1, 2], n)));
      // A different oblique ray, as the pointer turns: it meets the plane 3 mm further along u and 2 mm back along v.
      const r = dragStep(drag, rayThrough(onPlane(28, 23), cycle([3, -1, 2], n)), false);
      expect(r.offset[u]).toBeCloseTo(3, 9);
      expect(r.offset[v]).toBeCloseTo(-2, 9);
      expect(isZero(r.offset[n])).toBe(true);
      expect(r.delta).toEqual(translationMatrix(r.offset));
    });
  }
});

describe("free move (centre ball)", () => {
  it("moves in the screen plane, square to the camera's forward axis, with the selection off the view's centre", () => {
    // A perspective camera looking along `forward` with the centre 80 mm right of the middle of its view: the
    // line of sight to the centre is 15° off the forward axis, and the move must not tilt with it.
    const forward = new Vector3(-1, 2, -1.5).normalize();
    const right = new Vector3().crossVectors(forward, new Vector3(0, 0, 1)).normalize();
    const up = new Vector3().crossVectors(right, forward);
    const camera = CENTRE.clone().addScaledVector(forward, -300).addScaledVector(right, -80);
    expect(CENTRE.clone().sub(camera).normalize().dot(forward)).toBeLessThan(0.97);
    const drag = start({ kind: "free" }, rayFrom(camera, CENTRE), forward);
    // Then a point 20 mm right of and 7 mm below the centre, at its depth in the view.
    const target = CENTRE.clone().addScaledVector(right, 20).addScaledVector(up, -7);
    const r = dragStep(drag, rayFrom(camera, target), false);
    expect(new Vector3(...r.offset).dot(forward)).toBeCloseTo(0, 9);
    expectNear(r.offset, target.clone().sub(CENTRE).toArray());
  });

  // The Top and Front presets in perspective with the part off to one side of the orbit target. A plane
  // square to the line of sight sank such a part 13 mm into the bed in Top, and the Top preset's 1e-4 rad
  // tilt off the pole drifts the depth by a ten-thousandth of the travel unless the plane is squared to Z.
  const presets: [string, ViewDirection, Axis, Vector3, Vector3][] = [
    ["Top", PRESETS.top, 2, new Vector3(210, 60, 10), new Vector3(125, 125, 0)],
    ["Front", PRESETS.front, 1, new Vector3(40, 20, 15), new Vector3(125, 125, 60)],
  ];
  for (const [name, direction, depth, centre, target] of presets) {
    it(`keeps ${NAMES[depth]} exactly as it was in the perspective ${name} view, the part under the pointer`, () => {
      const camera = perspectiveAt(direction, target);
      const press = toPx(camera, centre);
      const drag = startDrag({ kind: "free" }, centre, rayAt(camera, press), camera.getWorldDirection(new Vector3()))!;
      for (const [dx, dy] of [
        [-200, 0],
        [0, 150],
        [-300, 200],
      ]) {
        const pointer = { x: press.x + dx, y: press.y + dy };
        expect(isZero(dragStep(drag, rayAt(camera, pointer), true).offset[depth])).toBe(true);
        const r = dragStep(drag, rayAt(camera, pointer), false);
        expect(isZero(r.offset[depth])).toBe(true);
        const moved = toPx(camera, centre.clone().add(new Vector3(...r.offset)));
        expect(moved.x).toBeCloseTo(pointer.x, 6);
        expect(moved.y).toBeCloseTo(pointer.y, 6);
      }
    });
  }
});

describe("rotation", () => {
  const R = 30;
  for (const a of AXES) {
    for (const side of [1, -1]) {
      it(`turns +90° from ${NAMES[planeAxes(a)[0]]} to ${NAMES[planeAxes(a)[1]]} about ${NAMES[a]}, seen from the ${side > 0 ? "+" : "−"} side`, () => {
        const [u, v] = planeAxes(a);
        const grab = CENTRE.clone().addScaledVector(unit(u), R);
        const mid = CENTRE.clone().addScaledVector(unit(u), R * Math.SQRT1_2).addScaledVector(unit(v), R * Math.SQRT1_2);
        const end = CENTRE.clone().addScaledVector(unit(v), R);
        // A perspective camera off to one side but facing the arc's plane (|cos| = 150 / 166), so the arc shows.
        // Seen from the − side the arc runs clockwise on screen; the turn about +axis must not change sign.
        const camera = CENTRE.clone().add(cycle([150 * side, 40, -60], a));
        expect(handleVisible({ kind: "rotate", axis: a }, CENTRE.clone().sub(camera).normalize())).toBe(true);
        const drag = start({ kind: "rotate", axis: a }, rayFrom(camera, grab));
        dragStep(drag, rayFrom(camera, mid), false);
        const r = dragStep(drag, rayFrom(camera, end), false);
        expect(r.angle).toBeCloseTo(Math.PI / 2, 9);
        expect(r.offset).toEqual([0, 0, 0]);
        expect(r.delta).toEqual(rotationAbout(a, r.angle, [CENTRE.x, CENTRE.y, CENTRE.z]));
        // Counter-clockwise looking down from +axis: the grab point goes where the pointer went.
        expectNear(transformPoint(r.delta, [grab.x, grab.y, grab.z]), end.toArray());
      });
    }
  }

  /** The point on a circle about Z through the centre, `d` degrees from +X towards +Y. */
  const onCircle = (d: number) => CENTRE.clone().add(new Vector3(R * Math.cos(deg(d)), R * Math.sin(deg(d)), 0));
  const fromAbove = (d: number) => rayThrough(onCircle(d), new Vector3(0, 0, -1));

  it("keeps counting past 180°: a full circle of steps turns 2π", () => {
    const drag = start({ kind: "rotate", axis: 2 }, fromAbove(0));
    let r: DragResult | null = null;
    for (let d = 30; d <= 360; d += 30) {
      r = dragStep(drag, fromAbove(d), false);
      // Past 180° the raw angle reads −150°; unwrapped it is +210°.
      if (d === 210) expect(r.angle).toBeCloseTo(deg(210), 9);
    }
    expect(r!.angle).toBeCloseTo(2 * Math.PI, 9);
    expect(dragStep(drag, fromAbove(390), false).angle).toBeCloseTo(deg(390), 9);
  });

  it("counts a full circle backwards to −2π", () => {
    const drag = start({ kind: "rotate", axis: 2 }, fromAbove(0));
    let r: DragResult | null = null;
    for (let d = -45; d >= -360; d -= 45) r = dragStep(drag, fromAbove(d), false);
    expect(r!.angle).toBeCloseTo(-2 * Math.PI, 9);
  });

  it("keeps the grabbed ball under the pointer in perspective with the arc's plane at a grazing angle", () => {
    // 13° above the XY plane, the part off the orbit target: |cos| between the line of sight and Z is about
    // 0.22, just above EDGE_ON, so the Z arc still shows. A pixel of travel sweeps the plane hit a long way
    // here, but the ray still meets the plane where the pointer is.
    const camera = perspectiveAt({ theta: -Math.PI / 3, phi: Math.PI / 2 - deg(13) }, new Vector3(0, 0, 0), 120);
    const centre = new Vector3(20, 10, 0);
    const viewDir = lineOfSight(camera, centre, new Vector3());
    expect(Math.abs(viewDir.z)).toBeGreaterThan(EDGE_ON);
    expect(Math.abs(viewDir.z)).toBeLessThan(0.25);
    // The Z arc's ball: 45° round the quadrant facing the camera.
    const [sx, sy] = facingSigns(viewDir.clone().negate());
    const grab = centre.clone().add(new Vector3(sx, sy, 0).multiplyScalar(R * Math.SQRT1_2));
    const drag = startDrag({ kind: "rotate", axis: 2 }, centre, rayAt(camera, toPx(camera, grab)), camera.getWorldDirection(new Vector3()))!;
    // Round to the far side of the arc and back past where it started.
    for (const d of [10, 60, 150, 200, 100, -30]) {
      // The pointer goes where turning the part by d carries the ball on screen.
      const turned = new Vector3(...transformPoint(rotationAbout(2, deg(d), [centre.x, centre.y, centre.z]), [grab.x, grab.y, grab.z]));
      expect(dragStep(drag, rayAt(camera, toPx(camera, turned)), false).angle).toBeCloseTo(deg(d), 9);
    }
  });
});

describe("snapping (Shift)", () => {
  const e = unit(0);
  const xRay = (x: number) => rayThrough(CENTRE.clone().addScaledVector(e, x), new Vector3(1, 3, -2));

  it("moves an arrow by whole mm, rounding -0.3 to 0 and never −0", () => {
    const drag = start({ kind: "axis", axis: 0 }, xRay(30));
    expect(dragStep(drag, xRay(42.4), true).offset).toEqual([12, 0, 0]);
    // The same drag without Shift: the exact value is still there.
    expect(dragStep(drag, xRay(42.4), false).offset[0]).toBeCloseTo(12.4, 9);
    const r = dragStep(drag, xRay(29.7), true);
    expect(r.offset).toEqual([0, 0, 0]);
    expect(isZero(r.offset[0])).toBe(true);
    expect(r.delta).toEqual(translationMatrix([0, 0, 0]));
  });

  it("rounds each in-plane component of a square's move", () => {
    const dir = new Vector3(1, -2, 3);
    const onXY = (x: number, y: number) => rayThrough(CENTRE.clone().add(new Vector3(x, y, 0)), dir);
    const drag = start({ kind: "plane", axis: 2 }, onXY(25, 25));
    const r = dragStep(drag, onXY(28.4, 22.4), true);
    expect(r.offset).toEqual([3, -3, 0]);
    expect(isZero(r.offset[2])).toBe(true);
  });

  it("rounds the centre ball's move on every world axis", () => {
    const viewDir = new Vector3(-1, 2, -1.5).normalize();
    const drag = start({ kind: "free" }, rayThrough(CENTRE, viewDir));
    const right = new Vector3().crossVectors(viewDir, new Vector3(0, 0, 1)).normalize();
    const r = dragStep(drag, rayThrough(CENTRE.clone().addScaledVector(right, 10.3), viewDir), true);
    // Screen right is (2, 1, 0) / √5 here: 10.3 mm along it is (9.21, 4.61, 0) before rounding, (9, 5, 0) after.
    expect(r.offset).toEqual([9, 5, 0]);
  });

  it("turns in 15° steps: 37° → 30°, 53° → 60°, −8° → −15°", () => {
    const onCircle = (d: number) => CENTRE.clone().add(new Vector3(30 * Math.cos(deg(d)), 30 * Math.sin(deg(d)), 0));
    const fromAbove = (d: number) => rayThrough(onCircle(d), new Vector3(0, 0, -1));
    const drag = start({ kind: "rotate", axis: 2 }, fromAbove(0));
    for (const [d, snapped] of [[37, 30], [53, 60], [-8, -15], [-7, 0]]) {
      const r = dragStep(drag, fromAbove(d), true);
      expect(r.angle).toBeCloseTo(deg(snapped), 12);
      expect(r.delta).toEqual(rotationAbout(2, r.angle, [CENTRE.x, CENTRE.y, CENTRE.z]));
    }
    expect(isZero(dragStep(drag, fromAbove(-7), true).angle)).toBe(true);
    expect(SNAP.rotate).toBeCloseTo(deg(15), 15);
  });
});

describe("rays that cannot drag", () => {
  it("cannot start along a ray parallel to the arrow, or in a square's, the arc's or the screen's plane", () => {
    const forward = new Vector3(1, 0, 0);
    expect(startDrag({ kind: "axis", axis: 0 }, CENTRE, rayThrough(CENTRE, new Vector3(1, 0, 0)), forward)).toBeNull();
    // Nearly parallel: within about 0.0001°.
    expect(startDrag({ kind: "axis", axis: 0 }, CENTRE, rayThrough(CENTRE, new Vector3(1, 1e-6, 0)), forward)).toBeNull();
    expect(startDrag({ kind: "plane", axis: 2 }, CENTRE, rayThrough(CENTRE, new Vector3(1, 1, 0)), forward)).toBeNull();
    expect(startDrag({ kind: "rotate", axis: 2 }, CENTRE, rayThrough(CENTRE.clone().add(new Vector3(30, 0, 0)), new Vector3(0, 1, 0)), forward)).toBeNull();
    // The centre ball's plane is square to the forward axis: a ray across the view never meets it.
    expect(startDrag({ kind: "free" }, CENTRE, rayThrough(CENTRE, new Vector3(0, 1, 0)), forward)).toBeNull();
  });

  it("keeps an axis move's last result when the ray turns parallel or passes the vanishing point", () => {
    const dir = new Vector3(1, 3, -2);
    const xRay = (x: number) => rayThrough(CENTRE.clone().add(new Vector3(x, 0, 0)), dir);
    const drag = start({ kind: "axis", axis: 0 }, xRay(0));
    const last = dragStep(drag, xRay(5.4), false);
    expect(dragStep(drag, rayThrough(CENTRE, new Vector3(1, 0, 0)), false)).toEqual(last);
    // Closest to the axis behind where the ray starts: it points away from the axis.
    const away = new Ray(CENTRE.clone().add(new Vector3(40, -50, 0)), new Vector3(0, -1, 0.2).normalize());
    expect(dragStep(drag, away, false)).toEqual(last);
    // The held measurement still follows Shift.
    expect(dragStep(drag, away, true).offset).toEqual([5, 0, 0]);
  });

  it("keeps a square's and an arc's last result when the ray misses the plane", () => {
    const dir = new Vector3(1, -2, -3);
    const onXY = (x: number, y: number) => rayThrough(CENTRE.clone().add(new Vector3(x, y, 0)), dir);
    const square = start({ kind: "plane", axis: 2 }, onXY(20, 20));
    const last = dragStep(square, onXY(24, 18), false);
    // Pointing up, away from the plane below.
    const up = new Ray(CENTRE.clone().add(new Vector3(0, 0, 50)), new Vector3(0, 0.3, 1).normalize());
    expect(dragStep(square, up, false)).toEqual(last);

    const arc = start({ kind: "rotate", axis: 2 }, onXY(30, 0));
    const turned = dragStep(arc, onXY(0, 30), false);
    expect(turned.angle).toBeCloseTo(Math.PI / 2, 9);
    expect(dragStep(arc, up, false)).toEqual(turned);
    // A hit on the axis itself has no angle: held too.
    expect(dragStep(arc, onXY(0, 0), false)).toEqual(turned);
  });
});

describe("dragLabel", () => {
  const move = (offset: Vec3): DragResult => ({ offset, angle: 0, delta: translationMatrix(offset) });
  const turn = (degrees: number): DragResult => ({ offset: [0, 0, 0], angle: deg(degrees), delta: [] });

  it("shows an arrow's move in mm with its sign, and 0 bare", () => {
    expect(dragLabel({ kind: "axis", axis: 0 }, move([12.5, 0, 0]))).toBe("X +12.5 mm");
    expect(dragLabel({ kind: "axis", axis: 1 }, move([0, -3.04, 0]))).toBe("Y −3 mm");
    expect(dragLabel({ kind: "axis", axis: 2 }, move([0, 0, -0.02]))).toBe("Z 0 mm");
    expect(dragLabel({ kind: "axis", axis: 2 }, move([0, 0, -0]))).toBe("Z 0 mm");
  });

  it("shows a square's two axes in X, Y, Z order", () => {
    expect(dragLabel({ kind: "plane", axis: 2 }, move([3, -2, 0]))).toBe("X +3  Y −2 mm");
    expect(dragLabel({ kind: "plane", axis: 1 }, move([4, 0, -0.5]))).toBe("X +4  Z −0.5 mm");
    expect(dragLabel({ kind: "plane", axis: 0 }, move([0, 7.06, 0]))).toBe("Y +7.1  Z 0 mm");
  });

  it("shows all three axes for the centre ball", () => {
    expect(dragLabel({ kind: "free" }, move([1, -2, 0.5]))).toBe("X +1  Y −2  Z +0.5 mm");
  });

  it("shows a turn in degrees, counting on past a full turn", () => {
    expect(dragLabel({ kind: "rotate", axis: 2 }, turn(30))).toBe("Z +30°");
    expect(dragLabel({ kind: "rotate", axis: 0 }, turn(-15))).toBe("X −15°");
    expect(dragLabel({ kind: "rotate", axis: 1 }, turn(390))).toBe("Y +390°");
    expect(dragLabel({ kind: "rotate", axis: 2 }, turn(0.02))).toBe("Z 0°");
    expect(dragLabel({ kind: "rotate", axis: 2 }, turn(12.345))).toBe("Z +12.3°");
  });
});

describe("worldPerPixel", () => {
  it("grows with depth along the line of sight for a perspective camera, and shrinks with zoom", () => {
    const camera = new PerspectiveCamera(90, 1.5, 0.1, 1000);
    camera.up.set(0, 0, 1);
    camera.position.set(0, -100, 0);
    camera.lookAt(0, 0, 0);
    camera.updateMatrixWorld();
    // A 90° field of view is 2 × depth tall: 200 mm over 200 px at 100 mm deep.
    expect(worldPerPixel(camera, new Vector3(0, 0, 0), 200)).toBeCloseTo(1, 12);
    // Off to the side at the same depth: the same, though further from the camera.
    expect(worldPerPixel(camera, new Vector3(60, 0, -40), 200)).toBeCloseTo(1, 12);
    expect(worldPerPixel(camera, new Vector3(0, 100, 0), 200)).toBeCloseTo(2, 12);
    camera.zoom = 2;
    expect(worldPerPixel(camera, new Vector3(0, 0, 0), 200)).toBeCloseTo(0.5, 12);
  });

  it("matches what an oblique perspective camera draws", () => {
    const camera = new PerspectiveCamera(35, 1.6, 0.1, 5000);
    camera.up.set(0, 0, 1);
    camera.position.set(130, -170, 90);
    camera.lookAt(10, 20, 5);
    camera.zoom = 1.3;
    camera.updateProjectionMatrix();
    camera.updateMatrixWorld();
    const p = new Vector3(40, 10, 25);
    // 1 mm along the camera's up at p, projected: NDC spans 2 over the 600 px height.
    const up = new Vector3().setFromMatrixColumn(camera.matrixWorld, 1).normalize();
    const a = p.clone().project(camera);
    const b = p.clone().add(up).project(camera);
    expect(worldPerPixel(camera, p, 600)).toBeCloseTo(1 / (((b.y - a.y) / 2) * 600), 9);
  });

  it("follows zoom alone for an orthographic camera", () => {
    const camera = new OrthographicCamera(-150, 150, 100, -100, -1000, 1000);
    camera.zoom = 2;
    // 200 mm tall at zoom 1, 100 mm at zoom 2, over 400 px; the same at every depth.
    expect(worldPerPixel(camera, new Vector3(0, 0, 0), 400)).toBe(0.25);
    expect(worldPerPixel(camera, new Vector3(0, 500, 30), 400)).toBe(0.25);
  });
});

describe("lineOfSight", () => {
  it("runs from a perspective camera to the point, off the forward axis away from the view's middle", () => {
    const camera = new PerspectiveCamera(35, 1.6, 0.1, 5000);
    camera.up.set(0, 0, 1);
    camera.position.set(130, -170, 90);
    camera.lookAt(10, 20, 5);
    camera.updateMatrixWorld();
    const p = new Vector3(40, 10, 25);
    const out = new Vector3();
    expect(lineOfSight(camera, p, out)).toBe(out);
    expectNear(out.toArray(), p.clone().sub(camera.position).normalize().toArray(), 12);
    expect(out.dot(camera.getWorldDirection(new Vector3()))).toBeLessThan(0.999);
  });

  it("is an orthographic camera's forward axis at every point", () => {
    const camera = new OrthographicCamera(-150, 150, 100, -100, -1000, 1000);
    camera.up.set(0, 0, 1);
    camera.position.set(100, -100, 100);
    camera.lookAt(0, 0, 0);
    camera.updateMatrixWorld();
    const forward = new Vector3(-1, 1, -1).normalize().toArray();
    // Behind the camera too: an orthographic view has no eye point to look from.
    for (const p of [new Vector3(0, 0, 0), new Vector3(300, 40, -80), new Vector3(400, -400, 400)]) {
      expectNear(lineOfSight(camera, p, new Vector3()).toArray(), forward, 12);
    }
  });
});

describe("gizmo layout helpers", () => {
  it("names the same handle the same", () => {
    expect(sameHandle({ kind: "axis", axis: 1 }, { kind: "axis", axis: 1 })).toBe(true);
    expect(sameHandle({ kind: "axis", axis: 1 }, { kind: "plane", axis: 1 })).toBe(false);
    expect(sameHandle({ kind: "rotate", axis: 0 }, { kind: "rotate", axis: 2 })).toBe(false);
    expect(sameHandle({ kind: "free" }, { kind: "free" })).toBe(true);
    expect(sameHandle({ kind: "free" }, { kind: "axis", axis: 0 })).toBe(false);
    expect(sameHandle(null, null)).toBe(true);
    expect(sameHandle({ kind: "free" }, null)).toBe(false);
  });

  it("gives each axis's plane axes in cyclic order, u × v = axis", () => {
    expect(planeAxes(0)).toEqual([1, 2]);
    expect(planeAxes(1)).toEqual([2, 0]);
    expect(planeAxes(2)).toEqual([0, 1]);
    for (const a of AXES) {
      const [u, v] = planeAxes(a);
      expect(new Vector3().crossVectors(unit(u), unit(v))).toEqual(unit(a));
    }
  });

  it("gives each axis the side of the centre the camera is on, 0 counting as +", () => {
    expect(facingSigns(new Vector3(3, -2, 0))).toEqual([1, -1, 1]);
    expect(facingSigns(new Vector3(-1, 4, -0.5))).toEqual([-1, 1, -1]);
    expect(facingSigns(new Vector3(0, 0, 0))).toEqual([1, 1, 1]);
  });

  it("counts a side within a thousandth of the way to the camera as +, so the presets' tilt off the pole does not count", () => {
    // Top and Bottom sit 1e-4 rad off the pole on the front (−Y) side; Front's X and Z are 6e-17 of rounding.
    expect(facingSigns(directionOf(PRESETS.top))).toEqual([1, 1, 1]);
    expect(facingSigns(directionOf(PRESETS.bottom))).toEqual([1, 1, -1]);
    expect(facingSigns(directionOf(PRESETS.front))).toEqual([1, -1, 1]);
    expect(facingSigns(directionOf(PRESETS.iso))).toEqual([1, -1, 1]);
    // The rule goes by the vector's length, and just past it the side counts.
    expect(facingSigns(new Vector3(-0.0009, -0.0011, 1))).toEqual([1, -1, 1]);
    expect(facingSigns(new Vector3(-0.9, -1.1, 1000))).toEqual([1, -1, 1]);
  });

  it("hides an arrow pointing at the viewer, and a square or an arc seen edge-on", () => {
    const down = new Vector3(0, 0, -1);
    expect(handleVisible({ kind: "axis", axis: 2 }, down)).toBe(false);
    expect(handleVisible({ kind: "axis", axis: 0 }, down)).toBe(true);
    expect(handleVisible({ kind: "plane", axis: 2 }, down)).toBe(true); // XY, seen square-on
    expect(handleVisible({ kind: "plane", axis: 0 }, down)).toBe(false); // YZ, edge-on
    expect(handleVisible({ kind: "rotate", axis: 0 }, down)).toBe(false); // in the YZ plane, edge-on
    expect(handleVisible({ kind: "rotate", axis: 2 }, down)).toBe(true); // in the XY plane, face-on
    expect(handleVisible({ kind: "free" }, down)).toBe(true);
  });

  it("still shows a handle exactly on END_ON or EDGE_ON", () => {
    // (0.28, 0, 0.96) and (0.2, 0, √0.96) are unit vectors.
    expect(handleVisible({ kind: "axis", axis: 2 }, new Vector3(0.28, 0, END_ON))).toBe(true);
    expect(handleVisible({ kind: "axis", axis: 2 }, new Vector3(0.27, 0, 0.963).normalize())).toBe(false);
    for (const kind of ["plane", "rotate"] as const) {
      expect(handleVisible({ kind, axis: 0 }, new Vector3(EDGE_ON, 0, Math.sqrt(0.96)))).toBe(true);
      expect(handleVisible({ kind, axis: 0 }, new Vector3(0.19, 0, 0.98).normalize())).toBe(false);
    }
  });
});
