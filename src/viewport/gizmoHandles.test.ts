import { Box3, Color, Mesh, PerspectiveCamera, Raycaster, SRGBColorSpace, Vector2, Vector3, type BufferGeometry, type Material, type Object3D } from "three";
import { describe, expect, it } from "vitest";
import { rotationAbout, transformPoint, type Axis } from "../matrix";
import { applyView, createCameras, directionOf, FOV, PRESETS, rayFromCamera, setAspect, type ViewCamera, type ViewDirection } from "./cameras";
import { ARC_RADIUS, createGizmoHandles, GIZMO_PX, type GizmoHandles } from "./gizmoHandles";
import { facingSigns, lineOfSight, planeAxes, sameHandle, worldPerPixel, type Handle } from "./gizmoMath";
import type { VpTheme } from "./theme";

/** The light theme's values from theme.css. */
const LIGHT: VpTheme = {
  platform: "#7d8794",
  gridMinor: "#d2d7de",
  gridMajor: "#b3bbc6",
  ruler: "#5d6877",
  selected: "#f28c28",
  backface: "#d93a3a",
  openEdge: "#e02424",
  edge: "#3a4250",
  axisX: "#d6453d",
  axisY: "#3f9b4a",
  axisZ: "#3b6fd8",
  gizmoCentre: "#8a929e",
  repairHole: "#ffcf26",
  repairError: "#ff8a1c",
  repairDegenerate: "#e04ce0",
  repairSelected: "#33c25a",
  repairSlim: "#3d8bf2",
  cutPoint: "#ff5cb1",
  cutCurrent: "#d4006f",
  cutLine: "#e8202a",
  node: "#18b4f0",
  booleanMain: "#3aa655",
  booleanMainSelected: "#86e09a",
  booleanTool: "#d64545",
  booleanToolSelected: "#ff9a9a",
  surface: "#ffffff",
  surface2: "#f6f7f9",
  border: "#d3d8df",
  borderStrong: "#b9c0ca",
  text: "#1b2028",
  textMuted: "#5d6877",
  accent: "#0b8a8f",
  font: "sans-serif",
};
const DARK: VpTheme = { ...LIGHT, axisX: "#ef5a50", gizmoCentre: "#a7b0bc", surface: "#1d2128" };

const CENTRE = new Vector3(40, -25, 12);
/** World mm per arrow length. */
const SCALE = 30;
/** Looking down from above the +X, +Y, +Z side of the centre. */
const ISO = new Vector3(-1, -1.3, -0.9);
const DOWN = new Vector3(0, 0, -1);

const AXES: Axis[] = [0, 1, 2];
const NAMES = ["X", "Y", "Z"];
/** Squares by their normal. */
const PLANES = ["YZ", "XZ", "XY"];
/** Where an arc's ball sits, in arrow lengths along each of its plane axes. */
const GRIP = ARC_RADIUS * Math.SQRT1_2;
const ALL_HANDLES: Handle[] = [
  { kind: "free" },
  ...AXES.map((axis) => ({ kind: "axis", axis }) as const),
  ...AXES.map((axis) => ({ kind: "plane", axis }) as const),
  ...AXES.map((axis) => ({ kind: "rotate", axis }) as const),
];

/** A world point from gizmo units: arrow lengths from the centre along X, Y, Z. */
const at = (x: number, y: number, z: number) => new Vector3(x, y, z).multiplyScalar(SCALE).add(CENTRE);

/** Lays `gizmo` out at CENTRE for a camera looking along `viewDir`, squares and arcs in the quadrant facing it. */
function layOut(gizmo: GizmoHandles, viewDir: Vector3) {
  const dir = viewDir.clone().normalize();
  gizmo.layout(CENTRE, SCALE, dir, facingSigns(dir.clone().negate()));
}

/** A new gizmo, shown and laid out for `viewDir`. */
function shownFor(viewDir: Vector3, theme = LIGHT): GizmoHandles {
  const gizmo = createGizmoHandles(theme);
  gizmo.group.visible = true;
  layOut(gizmo, viewDir);
  return gizmo;
}

/** A ray along `dir` through `through`, starting 1000 mm before it. */
function rayThrough(through: Vector3, dir: Vector3): Raycaster {
  const d = dir.clone().normalize();
  return new Raycaster(through.clone().addScaledVector(d, -1000), d);
}

const pickedAt = (gizmo: GizmoHandles, through: Vector3, dir: Vector3) => gizmo.pick(rayThrough(through, dir))?.handle ?? null;

/** The drawn (not picker) pieces of the handle object called `name`. */
function drawn(gizmo: GizmoHandles, name: string): Object3D[] {
  const object = gizmo.group.getObjectByName(name);
  if (!object) throw new Error(`no ${name}`);
  return object.children.filter((c) => c.visible);
}

/** The scene object name of a handle. */
const nameOf = (h: Handle) =>
  h.kind === "free" ? "centre ball" : h.kind === "axis" ? `${NAMES[h.axis]} arrow` : h.kind === "plane" ? `${PLANES[h.axis]} square` : `${NAMES[h.axis]} arc`;

/** The invisible pickers of the handle object called `name`. */
function pickersOf(gizmo: GizmoHandles, name: string): Object3D[] {
  return gizmo.group.getObjectByName(name)!.children.filter((c) => !c.visible);
}

const deg = (d: number) => (d * Math.PI) / 180;

/** A real camera of either projection looking at `target` from `direction`, as applyView and the orbit controls leave it. */
function viewCamera(orthographic: boolean, direction: ViewDirection, target: Vector3): ViewCamera {
  const cameras = createCameras();
  setAspect(cameras, 1.6);
  const camera = orthographic ? cameras.orthographic : cameras.perspective;
  applyView(camera, { target, ...direction, halfHeight: 140 }, new Vector3());
  camera.lookAt(target);
  camera.updateMatrixWorld();
  return camera;
}

/** Lays `gizmo` out at `centre` as the viewport does for `camera` on an 800 px high canvas: mm per arrow length, and the line of sight. */
function layOutFor(gizmo: GizmoHandles, camera: ViewCamera, centre: Vector3): { scale: number; viewDir: Vector3 } {
  const viewDir = lineOfSight(camera, centre, new Vector3());
  const scale = worldPerPixel(camera, centre, 800) * GIZMO_PX;
  gizmo.layout(centre, scale, viewDir, facingSigns(viewDir.clone().negate()));
  return { scale, viewDir };
}

/** The pointer ray through where world point `p` shows on screen. */
function rayAt(camera: ViewCamera, p: Vector3): Raycaster {
  const ndc = p.clone().project(camera);
  const raycaster = new Raycaster();
  rayFromCamera(raycaster, new Vector2(ndc.x, ndc.y), camera);
  return raycaster;
}

/** World middle of what is drawn for `handle`: the centre ball, an arrow's tip, a square's fill, an arc's ball. */
function drawnMiddle(gizmo: GizmoHandles, handle: Handle): Vector3 {
  if (handle.kind === "free") return gizmo.group.position.clone();
  // An arrow is [shaft, tip], a square [fill, outline], an arc [tube, ball].
  const piece = drawn(gizmo, nameOf(handle))[handle.kind === "plane" ? 0 : 1];
  return new Box3().setFromObject(piece).getCenter(new Vector3());
}

/** Opacity of each material of the handle called `name`, in order. */
const opacities = (gizmo: GizmoHandles, name: string) => drawn(gizmo, name).map((c) => ((c as Mesh).material as Material).opacity);

/** sRGB lightness of the first drawn piece's colour of the handle called `name`. */
function lightness(gizmo: GizmoHandles, name: string): number {
  const color = ((drawn(gizmo, name)[0] as Mesh).material as Material & { color: Color }).color;
  return color.getHSL({ h: 0, s: 0, l: 0 }, SRGBColorSpace).l;
}
const themeLightness = (hex: string) => new Color(hex).getHSL({ h: 0, s: 0, l: 0 }, SRGBColorSpace).l;

describe("createGizmoHandles: picking", () => {
  it("picks the centre ball with a ray straight at the centre, and gives the point hit on its camera side", () => {
    const gizmo = shownFor(ISO);
    const hit = gizmo.pick(rayThrough(CENTRE, ISO));
    expect(hit?.handle).toEqual({ kind: "free" });
    const out = hit!.point.clone().sub(CENTRE);
    // On the drawn ball (a faceted sphere of 0.08 arrow lengths), towards the camera.
    expect(out.length() / SCALE).toBeGreaterThan(0.075);
    expect(out.length() / SCALE).toBeLessThanOrEqual(0.08 + 1e-6);
    expect(out.dot(ISO)).toBeLessThan(0);
  });

  it("picks each arrow by its tip, at `scale` mm per arrow length", () => {
    const gizmo = shownFor(ISO);
    for (const axis of AXES) {
      const tip = new Vector3().setComponent(axis, 0.92);
      expect(pickedAt(gizmo, at(tip.x, tip.y, tip.z), ISO)).toEqual({ kind: "axis", axis });
    }
    // Just past the tip there is nothing.
    expect(pickedAt(gizmo, at(1.1, 0, 0), ISO)).toBeNull();
  });

  it("picks each plane square at its middle", () => {
    for (const axis of AXES) {
      // Looking straight down the square's normal; its middle is 0.28 out along both in-plane axes.
      const middle = new Vector3(0.28, 0.28, 0.28).setComponent(axis, 0);
      const gizmo = shownFor(new Vector3().setComponent(axis, -1));
      expect(pickedAt(gizmo, at(middle.x, middle.y, middle.z), new Vector3().setComponent(axis, -1))).toEqual({ kind: "plane", axis });
    }
  });

  it("picks an arc by its ball and along its length, and gives the grab point on it", () => {
    const gizmo = shownFor(DOWN);
    const grip = at(GRIP, GRIP, 0);
    const byBall = gizmo.pick(rayThrough(grip, DOWN));
    expect(byBall?.handle).toEqual({ kind: "rotate", axis: 2 });
    // On the drawn ball (0.055 arrow lengths). Geometry is stored in 32-bit floats, hence the 1e-6.
    expect(byBall!.point.distanceTo(grip) / SCALE).toBeLessThanOrEqual(0.055 + 1e-6);

    const angle = (20 * Math.PI) / 180;
    const onArc = at(ARC_RADIUS * Math.cos(angle), ARC_RADIUS * Math.sin(angle), 0);
    const byTube = gizmo.pick(rayThrough(onArc, DOWN));
    expect(byTube?.handle).toEqual({ kind: "rotate", axis: 2 });
    // On the drawn tube, 0.014 arrow lengths round the arc.
    expect(byTube!.point.distanceTo(onArc) / SCALE).toBeLessThanOrEqual(0.014 + 1e-6);
    // Beside the tube, inside its picker (0.06), it still picks.
    const beside = at((ARC_RADIUS + 0.04) * Math.cos(angle), (ARC_RADIUS + 0.04) * Math.sin(angle), 0);
    expect(pickedAt(gizmo, beside, DOWN)).toEqual({ kind: "rotate", axis: 2 });
  });

  it("finds nothing until it is shown, and nothing once hidden again", () => {
    const gizmo = createGizmoHandles(LIGHT);
    layOut(gizmo, ISO);
    expect(gizmo.group.visible).toBe(false);
    expect(gizmo.pick(rayThrough(CENTRE, ISO))).toBeNull();
    gizmo.group.visible = true;
    expect(gizmo.pick(rayThrough(CENTRE, ISO))?.handle).toEqual({ kind: "free" });
    gizmo.group.visible = false;
    expect(gizmo.pick(rayThrough(CENTRE, ISO))).toBeNull();
  });
});

describe("createGizmoHandles: hidden handles", () => {
  // Rays here run square to the view, through one handle and clear of every other, so whether they find it
  // shows whether it can be picked at all.
  const ACROSS_Y = new Vector3(0, 1, 0);
  const ACROSS_X = new Vector3(1, 0, 0);

  it("skips an arrow seen end-on", () => {
    // From a camera 11° off straight above, the Z arrow points at the viewer.
    const gizmo = shownFor(new Vector3(0.2, 0, -1));
    expect(drawn(gizmo, "Z arrow")).toHaveLength(2);
    expect(gizmo.group.getObjectByName("Z arrow")!.visible).toBe(false);
    expect(pickedAt(gizmo, at(0, 0, 0.92), ACROSS_Y)).toBeNull();
    // From 27° off it shows, and the same ray picks it.
    layOut(gizmo, new Vector3(0.5, 0, -1));
    expect(gizmo.group.getObjectByName("Z arrow")!.visible).toBe(true);
    expect(pickedAt(gizmo, at(0, 0, 0.92), ACROSS_Y)).toEqual({ kind: "axis", axis: 2 });
  });

  it("skips an arc seen edge-on, ball and all", () => {
    // Looking straight down, the X arc's plane (YZ) is edge-on and the Z arc's (XY) face-on.
    const gizmo = shownFor(DOWN);
    expect(gizmo.group.getObjectByName("X arc")!.visible).toBe(false);
    expect(gizmo.group.getObjectByName("Y arc")!.visible).toBe(false);
    expect(gizmo.group.getObjectByName("Z arc")!.visible).toBe(true);
    const ball = at(0, GRIP, GRIP);
    const tube = at(0, ARC_RADIUS * Math.cos(0.35), ARC_RADIUS * Math.sin(0.35));
    expect(pickedAt(gizmo, ball, ACROSS_X)).toBeNull();
    expect(pickedAt(gizmo, tube, ACROSS_X)).toBeNull();
    // From the ISO camera the same rays find it.
    layOut(gizmo, ISO);
    expect(pickedAt(gizmo, ball, ACROSS_X)).toEqual({ kind: "rotate", axis: 0 });
    expect(pickedAt(gizmo, tube, ACROSS_X)).toEqual({ kind: "rotate", axis: 0 });
  });

  it("skips a square seen edge-on", () => {
    const gizmo = shownFor(new Vector3(-1, -1, -0.1));
    expect(gizmo.group.getObjectByName("XY square")!.visible).toBe(false);
    expect(pickedAt(gizmo, at(0.28, 0.28, 0), DOWN)).toBeNull();
    // Same quadrant, steeper camera: the square shows and picks.
    layOut(gizmo, new Vector3(-1, -1, -3));
    expect(pickedAt(gizmo, at(0.28, 0.28, 0), DOWN)).toEqual({ kind: "plane", axis: 2 });
  });
});

describe("createGizmoHandles: priority", () => {
  // Front view tilted 17° down: the camera at −Y, a little above. The Z arc (|cos| 0.29, so it shows)
  // lies in the −Y quadrant nearly edge-on, and on screen runs along the X arrow just below it.
  const FRONT_ABOVE = new Vector3(0, 1, -0.3).normalize();
  /** Screen up in this view: square to the line of sight and to X. */
  const UP = new Vector3(0, 0.3, 1).normalize();

  it("lets the drawn arrow win over the nearer arc's picker in front of it", () => {
    const gizmo = shownFor(FRONT_ABOVE);
    expect(gizmo.group.getObjectByName("Z arc")!.visible).toBe(true);
    const through = at(0.6, 0, 0);
    expect(pickedAt(gizmo, through, FRONT_ABOVE)).toEqual({ kind: "axis", axis: 0 });
    // The arc's picker is there, nearer: a ray that stops short of the arrow finds it.
    const short = rayThrough(through, FRONT_ABOVE);
    short.far = 1000 - 0.1 * SCALE;
    expect(gizmo.pick(short)?.handle).toEqual({ kind: "rotate", axis: 2 });
  });

  it("lets an arc ball's picker win over the arrow's behind it where neither is drawn", () => {
    const gizmo = shownFor(FRONT_ABOVE);
    // Just below the X arrow on screen, between it and the ball (0.13 arrow lengths below it here): clear of both drawn pieces.
    const ray = rayThrough(at(GRIP, 0, 0).addScaledVector(UP, -0.04 * SCALE), FRONT_ABOVE);
    expect(ray.intersectObjects([...drawn(gizmo, "X arrow"), ...drawn(gizmo, "Z arc")], false)).toEqual([]);
    const hit = gizmo.pick(ray);
    expect(hit?.handle).toEqual({ kind: "rotate", axis: 2 });
    // The ball sits on the camera's side, at −Y.
    expect(hit!.point.y).toBeLessThan(CENTRE.y - 0.3 * SCALE);
    // The arrow is there too, behind it: a ray that starts past the ball finds it.
    ray.near = 1000 - 0.2 * SCALE;
    expect(gizmo.pick(ray)?.handle).toEqual({ kind: "axis", axis: 0 });
  });

  it("takes the piece drawn on top over a nearer one drawn under it", () => {
    // Along the X axis from beyond its tip, a hair off the axis line: the ray meets the X arrow's tip and shaft first,
    // then the centre ball, which is drawn over them (the depth test is off, so ORDER alone decides).
    const gizmo = shownFor(ISO);
    const ray = rayThrough(at(0.5, 0.01, 0.005), new Vector3(-1, 0, 0));
    const meshes = ALL_HANDLES.flatMap((h) => drawn(gizmo, nameOf(h))).filter((o) => o instanceof Mesh);
    expect(ray.intersectObjects(meshes, false)[0].object.parent!.name).toBe("X arrow");
    const hit = gizmo.pick(ray);
    expect(hit?.handle).toEqual({ kind: "free" });
    expect(hit!.point.distanceTo(CENTRE) / SCALE).toBeLessThanOrEqual(0.08 + 1e-6);
  });

  it("takes the nearest of two handles drawn in the same order", () => {
    // Laid out for ISO, with the X and Y arrows on one ray across the view: from the +X, −Y side the X arrow
    // is nearer, from the −X, +Y side the Y arrow.
    const gizmo = shownFor(ISO);
    const towardsY = new Vector3(-1, 1, 0);
    expect(pickedAt(gizmo, at(0.6, 0, 0), towardsY)).toEqual({ kind: "axis", axis: 0 });
    const towardsX = new Vector3(1, -1, 0);
    expect(pickedAt(gizmo, at(0, 0.6, 0), towardsX)).toEqual({ kind: "axis", axis: 1 });
  });
});

describe("createGizmoHandles: the Isometric view", () => {
  // The startup view. The camera is on the (+X, −Y, +Z) side, so the Y arrow points to −Y, and on screen
  // the XZ square and the Y arc lie on the other side of the centre from it. With the Y arrow at +Y,
  // both drew over its middle and took its presses.
  const toCamera = directionOf(PRESETS.iso);
  const signs = facingSigns(toCamera);
  /** The handles' middles in arrow lengths, and what a press there must pick. */
  const targets: [Vector3, Handle][] = [
    [new Vector3(0, -0.5, 0), { kind: "axis", axis: 1 }],
    [new Vector3(0.28, 0, 0.28), { kind: "plane", axis: 1 }],
    ...AXES.map((axis): [Vector3, Handle] => {
      const [u, v] = planeAxes(axis);
      return [new Vector3().setComponent(u, GRIP * signs[u]).setComponent(v, GRIP * signs[v]), { kind: "rotate", axis }];
    }),
  ];

  it("picks the Y arrow's middle, the XZ square's middle and each arc's ball, along the preset's line of sight", () => {
    expect(signs).toEqual([1, -1, 1]);
    const view = toCamera.clone().negate();
    const gizmo = shownFor(view);
    for (const [p, handle] of targets) expect(pickedAt(gizmo, at(p.x, p.y, p.z), view)).toEqual(handle);
  });

  it("picks them through a real perspective camera at the preset, laid out as the viewport lays it out", () => {
    // The gizmo off the orbit target, so its line of sight is not the camera's forward axis.
    const target = CENTRE.clone().add(new Vector3(60, 40, -10));
    const camera = new PerspectiveCamera(FOV, 1.6, 0.1, 10000);
    camera.up.set(0, 0, 1);
    camera.position.copy(toCamera).multiplyScalar(450).add(target);
    camera.lookAt(target);
    camera.updateMatrixWorld();
    const gizmo = createGizmoHandles(LIGHT);
    gizmo.group.visible = true;
    const { scale } = layOutFor(gizmo, camera, CENTRE);
    // The pointer over each handle's middle on screen.
    for (const [p, handle] of targets) expect(gizmo.pick(rayAt(camera, p.clone().multiplyScalar(scale).add(CENTRE)))?.handle).toEqual(handle);
  });
});

describe("createGizmoHandles: what is drawn under the pointer", () => {
  // Orbited a little off the Isometric preset (15° round, 5° up), or to the nearby θ −30°, φ 50°: the XZ square shows
  // plainly (|cos| 0.38), but its middle lies inside the Y arc ball's picker (0.1 arrow lengths; the drawn ball is
  // 0.055), so with the pickers alone a press on the drawn square turned the part about Y.
  const offIso: [string, ViewDirection][] = [
    ["Isometric orbited 15° round and 5° up", { theta: PRESETS.iso.theta + deg(15), phi: PRESETS.iso.phi - deg(5) }],
    ["θ −30°, φ 50°", { theta: deg(-30), phi: deg(50) }],
  ];

  for (const [label, direction] of offIso) {
    it(`picks each square at its drawn middle, not the arc ball's picker over it: ${label}`, () => {
      for (const orthographic of [false, true]) {
        const camera = viewCamera(orthographic, direction, CENTRE);
        const gizmo = createGizmoHandles(LIGHT);
        gizmo.group.visible = true;
        layOutFor(gizmo, camera, CENTRE);
        for (const axis of AXES) {
          const square = gizmo.group.getObjectByName(`${PLANES[axis]} square`)!;
          expect(square.visible).toBe(true);
          // 0.28 out along both in-plane axes, mirrored into the quadrant facing the camera.
          const raycaster = rayAt(camera, square.localToWorld(new Vector3(0.28, 0.28, 0.28).setComponent(axis, 0)));
          expect(gizmo.pick(raycaster)?.handle).toEqual({ kind: "plane", axis });
          if (axis === 1) {
            // The case that went wrong: the Y arc ball's picker is under the pointer too, its drawn ball is not.
            expect(raycaster.intersectObjects(pickersOf(gizmo, "Y arc"), false).length).toBeGreaterThan(0);
            expect(raycaster.intersectObjects(drawn(gizmo, "Y arc"), false)).toEqual([]);
          }
        }
      }
    });
  }

  it("picks every shown handle through the middle of its drawn ball, tip or square, from all round in both projections", () => {
    // The gizmo off the orbit target, so in perspective its line of sight is not the camera's forward axis.
    const target = CENTRE.clone().add(new Vector3(25, 15, -10));
    // Aimed a hair (under 0.1 px) off each exact middle: a ray along the edge between two facets can slip between them.
    const nudge = new Vector2(1.3e-4, -0.9e-4);
    const wrong: string[] = [];
    const covered: string[] = [];
    let picks = 0;
    for (const orthographic of [false, true]) {
      for (let theta = 0; theta < 360; theta += 15) {
        for (let phi = 20; phi <= 160; phi += 10) {
          const camera = viewCamera(orthographic, { theta: deg(theta), phi: deg(phi) }, target);
          const gizmo = createGizmoHandles(LIGHT);
          gizmo.group.visible = true;
          const { viewDir } = layOutFor(gizmo, camera, CENTRE);
          for (const handle of ALL_HANDLES) {
            if (!gizmo.group.getObjectByName(nameOf(handle))!.visible) continue;
            const middle = drawnMiddle(gizmo, handle);
            const raycaster = new Raycaster();
            const ndc = middle.clone().project(camera);
            rayFromCamera(raycaster, new Vector2(ndc.x, ndc.y).add(nudge), camera);
            const hit = gizmo.pick(raycaster);
            picks++;
            // A ball or tip is hit on its side facing the camera, mirrored or not: short of its middle along the ray.
            const nearSide = handle.kind === "plane" || (!!hit && hit.point.clone().sub(middle).dot(raycaster.ray.direction) < 0);
            if (sameHandle(hit?.handle ?? null, handle) && nearSide) continue;
            const where = `${orthographic ? "ortho" : "persp"} θ ${theta}° φ ${phi}°: ${nameOf(handle)} picks ${hit ? nameOf(hit.handle) : "nothing"}`;
            // A square seen nearly edge-on along its diagonal (|cos| just over EDGE_ON) lies under the ball of the arc in
            // its plane: that ball is drawn over the square's middle, so a press there rightly takes the ball.
            const grazing = handle.kind === "plane" && Math.abs(viewDir.getComponent(handle.axis)) < 0.23;
            if (grazing && hit && raycaster.intersectObjects(drawn(gizmo, nameOf(hit.handle)), false).length) covered.push(where);
            else wrong.push(where);
          }
          gizmo.dispose();
        }
      }
    }
    expect(wrong).toEqual([]);
    // On this grid one view does that: |cos| 0.21, looking along the XY square's diagonal.
    expect(covered).toEqual(["persp θ 225° φ 100°: XY square picks Z arc"]);
    // 720 views, each with the centre ball and at least two arrows, squares and arcs.
    expect(picks).toBeGreaterThan(720 * 7);
  });

  it("still picks an arrow in the empty space beside its thin shaft, through its picker", () => {
    const gizmo = shownFor(ISO);
    // Square to both the line of sight and the X axis: across the shaft on screen, to the side away from the Z arc.
    const across = ISO.clone().cross(new Vector3(1, 0, 0)).normalize();
    // 0.04 arrow lengths off the axis: past the drawn shaft (0.016), inside the picker (0.07).
    const beside = rayThrough(at(0.5, 0, 0).addScaledVector(across, 0.04 * SCALE), ISO);
    const everythingDrawn = ALL_HANDLES.flatMap((h) => drawn(gizmo, nameOf(h)));
    expect(beside.intersectObjects(everythingDrawn, false)).toEqual([]);
    expect(gizmo.pick(beside)?.handle).toEqual({ kind: "axis", axis: 0 });
    // Past the picker there is nothing.
    expect(pickedAt(gizmo, at(0.5, 0, 0).addScaledVector(across, 0.09 * SCALE), ISO)).toBeNull();
  });
});

describe("createGizmoHandles: facing quadrant", () => {
  it("mirrors the XY square onto the camera's side of X and Y", () => {
    for (const [sx, sy] of [
      [1, 1],
      [-1, 1],
      [-1, -1],
      [1, -1],
    ]) {
      const gizmo = shownFor(new Vector3(-sx, -sy, -1.5));
      expect(pickedAt(gizmo, at(0.28 * sx, 0.28 * sy, 0), DOWN)).toEqual({ kind: "plane", axis: 2 });
      expect(pickedAt(gizmo, at(-0.28 * sx, -0.28 * sy, 0), DOWN)).toBeNull();
      // The drawn square goes with its picker.
      const fill = drawn(gizmo, "XY square").find((c) => c instanceof Mesh)!;
      const middle = new Box3().setFromObject(fill).getCenter(new Vector3());
      expect(middle.distanceTo(at(0.28 * sx, 0.28 * sy, 0))).toBeLessThan(1e-5);
    }
  });

  it("turns each arrow to the camera's side of its axis", () => {
    for (const side of [
      [1, 1, 1],
      [-1, -1, -1],
      [1, -1, 1],
      [-1, 1, -1],
    ]) {
      // Looking from the `side` octant, at ISO's slant.
      const view = new Vector3(-side[0], -1.3 * side[1], -0.9 * side[2]);
      const gizmo = shownFor(view);
      for (const axis of AXES) {
        const tip = new Vector3().setComponent(axis, 0.92 * side[axis]);
        expect(pickedAt(gizmo, at(tip.x, tip.y, tip.z), view)).toEqual({ kind: "axis", axis });
        expect(pickedAt(gizmo, at(-tip.x, -tip.y, -tip.z), view)).toBeNull();
        // The drawn tip goes with its picker.
        const cone = drawn(gizmo, `${NAMES[axis]} arrow`)[1];
        const middle = new Box3().setFromObject(cone).getCenter(new Vector3()).sub(CENTRE);
        expect(Math.sign(middle.getComponent(axis))).toBe(side[axis]);
      }
    }
  });
});

describe("createGizmoHandles: a rotate drag", () => {
  /** World position of the ball drawn on the arc called `name`. */
  const ballOf = (gizmo: GizmoHandles, name: string) => drawn(gizmo, name)[1].getWorldPosition(new Vector3());

  it("turns the dragged arc, its ball and its picker, by the angle about the arc's world axis", () => {
    // The camera on the −Y side, so the Z arc is mirrored into (+X, −Y).
    const view = new Vector3(-1, 1.3, -0.9).normalize();
    const signs = facingSigns(view.clone().negate());
    const gizmo = shownFor(view);
    const rest = ballOf(gizmo, "Z arc");
    expect(rest.distanceTo(at(GRIP, -GRIP, 0))).toBeLessThan(1e-9);
    const xBall = ballOf(gizmo, "X arc");

    const angle = (30 * Math.PI) / 180;
    gizmo.layout(CENTRE, SCALE, view, signs, { axis: 2, angle });
    // Where turning the parts by the same angle carries the ball: from −45° to −15° round +Z.
    const turned = new Vector3(...transformPoint(rotationAbout(2, angle, [CENTRE.x, CENTRE.y, CENTRE.z]), [rest.x, rest.y, rest.z]));
    expect(ballOf(gizmo, "Z arc").distanceTo(turned)).toBeLessThan(1e-9);
    const hit = gizmo.pick(rayThrough(turned, view));
    expect(hit?.handle).toEqual({ kind: "rotate", axis: 2 });
    // On the turned drawn ball (0.055 arrow lengths).
    expect(hit!.point.distanceTo(turned) / SCALE).toBeLessThanOrEqual(0.055 + 1e-6);
    // The other arcs stay where they were.
    expect(ballOf(gizmo, "X arc").distanceTo(xBall)).toBeLessThan(1e-9);

    // Laid out without the turn (the drag is over), the arc is back at rest.
    gizmo.layout(CENTRE, SCALE, view, signs, null);
    expect(ballOf(gizmo, "Z arc").distanceTo(rest)).toBeLessThan(1e-9);
    expect(pickedAt(gizmo, rest, view)).toEqual({ kind: "rotate", axis: 2 });
  });
});

describe("createGizmoHandles: highlight", () => {
  it("lights the hovered handle solid and deeper on a light theme, the rest at rest", () => {
    const gizmo = shownFor(ISO);
    gizmo.highlight({ kind: "axis", axis: 0 }, null);
    expect(opacities(gizmo, "X arrow")).toEqual([1, 1]);
    expect(opacities(gizmo, "Y arrow")).toEqual([0.85, 0.85]);
    expect(opacities(gizmo, "XY square")).toEqual([0.5, 0.85]);
    expect(lightness(gizmo, "X arrow")).toBeCloseTo(themeLightness(LIGHT.axisX) - 0.1, 4);
    expect(lightness(gizmo, "Y arrow")).toBeCloseTo(themeLightness(LIGHT.axisY), 4);
  });

  it("keeps the dragged handle lit and fades every other one, then goes back to rest", () => {
    const gizmo = shownFor(ISO);
    gizmo.highlight({ kind: "axis", axis: 0 }, { kind: "rotate", axis: 2 });
    expect(opacities(gizmo, "Z arc")).toEqual([1, 1]);
    expect(opacities(gizmo, "X arrow")).toEqual([0.2, 0.2]);
    expect(opacities(gizmo, "centre ball")).toEqual([0.2]);
    expect(opacities(gizmo, "XY square")).toEqual([0.1, 0.2]);
    gizmo.highlight(null, null);
    for (const name of ["Z arc", "X arrow", "centre ball"]) expect(opacities(gizmo, name).every((o) => o === 0.85)).toBe(true);
    expect(opacities(gizmo, "XY square")).toEqual([0.5, 0.85]);
  });

  it("lifts the lit colour on a dark theme, and keeps the look across a theme change", () => {
    const gizmo = shownFor(ISO);
    gizmo.highlight({ kind: "free" }, null);
    gizmo.applyTheme(DARK);
    expect(opacities(gizmo, "centre ball")).toEqual([1]);
    expect(lightness(gizmo, "centre ball")).toBeCloseTo(themeLightness(DARK.gizmoCentre) + 0.12, 4);
    expect(lightness(gizmo, "X arrow")).toBeCloseTo(themeLightness(DARK.axisX), 4);
  });
});

describe("createGizmoHandles: dispose", () => {
  it("frees every geometry and material it drew or picked with, the arcs' lit tubes included", () => {
    const gizmo = createGizmoHandles(LIGHT);
    const resources = new Set<BufferGeometry | Material>();
    const collect = () =>
      gizmo.group.traverse((o) => {
        if (o instanceof Mesh || "geometry" in o) {
          const { geometry, material } = o as Mesh;
          resources.add(geometry);
          for (const m of Array.isArray(material) ? material : [material]) resources.add(m);
        }
      });
    // Lighting each handle in turn swaps the arcs' fat tubes in, so they are collected too.
    for (const handle of [null, ...ALL_HANDLES]) {
      gizmo.highlight(handle, null);
      collect();
    }
    const disposed = new Set<unknown>();
    for (const r of resources) r.addEventListener("dispose", () => disposed.add(r));
    gizmo.dispose();
    expect(resources.size).toBeGreaterThan(20);
    expect([...resources].filter((r) => !disposed.has(r))).toEqual([]);
  });
});
