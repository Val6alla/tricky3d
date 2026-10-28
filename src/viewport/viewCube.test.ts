import { BufferAttribute, Matrix3, Mesh, MeshBasicMaterial, PerspectiveCamera, Raycaster, Vector3, type BufferGeometry } from "three";
import { afterEach, describe, expect, it } from "vitest";
import { runFeature } from "../features";
import { registerViewport, type ViewPreset, type ViewportApi } from "./api";
import { createCameraMotion } from "./cameraMotion";
import { applyView, directionOf, PRESETS, type ViewState } from "./cameras";
import { FACES, cubeGeometry } from "./viewCube";

// The View tab's standard views and the ViewCube: which way the camera looks after each, which way is up on screen,
// and which view each face of the cube picks.

const v = (x: number, y: number, z: number) => new Vector3(x, y, z).normalize();

/** For each view: from the target towards the camera, and what points up on screen. */
const EXPECTED: Record<ViewPreset, { towardsCamera: Vector3; up: Vector3 }> = {
  front: { towardsCamera: v(0, -1, 0), up: v(0, 0, 1) },
  back: { towardsCamera: v(0, 1, 0), up: v(0, 0, 1) },
  left: { towardsCamera: v(-1, 0, 0), up: v(0, 0, 1) },
  right: { towardsCamera: v(1, 0, 0), up: v(0, 0, 1) },
  top: { towardsCamera: v(0, 0, 1), up: v(0, 1, 0) },
  bottom: { towardsCamera: v(0, 0, -1), up: v(0, -1, 0) },
  iso: { towardsCamera: v(1, -1, 1), up: v(-1, 1, 2) },
};

const ALL = Object.keys(EXPECTED) as ViewPreset[];
const target = new Vector3(100, 80, 20);

/** A camera placed for `view` as the viewport does: applyView, then OrbitControls' lookAt with Z up. */
function cameraFor(view: ViewState) {
  const camera = new PerspectiveCamera(35, 1.5, 0.1, 10000);
  camera.up.set(0, 0, 1);
  const orbit = new Vector3();
  applyView(camera, view, orbit);
  camera.lookAt(orbit);
  camera.updateMatrixWorld();
  const right = new Vector3().setFromMatrixColumn(camera.matrixWorld, 0);
  const up = new Vector3().setFromMatrixColumn(camera.matrixWorld, 1);
  const forward = camera.getWorldDirection(new Vector3());
  return { camera, orbit, right, up, forward };
}

/** The view the viewport ends on after a preset command: its camera move from `from`, run to the end. */
function afterMove(from: ViewState, preset: ViewPreset): ViewState {
  let last: ViewState | null = null;
  const motion = createCameraMotion((view) => (last = view));
  motion.start(from, { ...from, ...PRESETS[preset] });
  while (motion.step(performance.now() + 1000));
  return last as unknown as ViewState;
}

afterEach(() => registerViewport(null));

describe("standard views", () => {
  it("each look at the scene from its own side, with the expected side up on screen", () => {
    for (const preset of ALL) {
      const want = EXPECTED[preset];
      expect([preset, directionOf(PRESETS[preset]).angleTo(want.towardsCamera)]).toEqual([preset, expect.closeTo(0, 3)]);
      const { forward, up, orbit } = cameraFor({ target: target.clone(), ...PRESETS[preset], halfHeight: 50 });
      expect(orbit.equals(target)).toBe(true);
      expect([preset, forward.angleTo(want.towardsCamera.clone().negate())]).toEqual([preset, expect.closeTo(0, 3)]);
      expect([preset, up.angleTo(want.up.clone().normalize())]).toEqual([preset, expect.closeTo(0, 3)]);
    }
  });

  it("end on that direction from wherever the camera was, keeping the target and the zoom", () => {
    // Starting views all round, including one turned more than a full circle.
    const starts = [PRESETS.iso, PRESETS.top, PRESETS.bottom, { theta: 7.5, phi: 2.9 }, { theta: -9, phi: 0.4 }];
    for (const start of starts) {
      for (const preset of ALL) {
        const from: ViewState = { target: target.clone(), ...start, halfHeight: 37 };
        const end = afterMove(from, preset);
        expect(end.target.equals(target)).toBe(true);
        expect(end.halfHeight).toBe(37);
        const { forward } = cameraFor(end);
        expect([start, preset, forward.angleTo(EXPECTED[preset].towardsCamera.clone().negate())]).toEqual([start, preset, expect.closeTo(0, 3)]);
      }
    }
  });

  it("are what the View tab's buttons ask the viewport for", () => {
    const asked: ViewPreset[] = [];
    const api = { setViewPreset: (p: ViewPreset) => asked.push(p) } as unknown as ViewportApi;
    registerViewport(api);
    for (const id of ["view.iso", "view.top", "view.bottom", "view.front", "view.back", "view.left", "view.right"]) runFeature(id);
    expect(asked).toEqual(["iso", "top", "bottom", "front", "back", "left", "right"]);
  });
});

/** The cube as the ViewCube builds it: one material per face, in FACES order. */
function cube() {
  const geometry = cubeGeometry();
  return { geometry, mesh: new Mesh(geometry, FACES.map(() => new MeshBasicMaterial())) };
}

/** The world directions in which a face's texture runs right (u) and up (v): its label's reading direction and its up. */
function labelAxes(geometry: BufferGeometry, face: number) {
  const group = geometry.groups.find((g) => g.materialIndex === face)!;
  const index = geometry.index!;
  const pos = geometry.getAttribute("position") as BufferAttribute;
  const uv = geometry.getAttribute("uv") as BufferAttribute;
  const [a, b, c] = [0, 1, 2].map((k) => index.getX(group.start + k));
  const p = (i: number) => new Vector3().fromBufferAttribute(pos, i);
  const e1 = p(b).sub(p(a));
  const e2 = p(c).sub(p(a));
  const du1 = uv.getX(b) - uv.getX(a);
  const dv1 = uv.getY(b) - uv.getY(a);
  const du2 = uv.getX(c) - uv.getX(a);
  const dv2 = uv.getY(c) - uv.getY(a);
  // Solve [e1 e2] = [U V] [[du1 du2], [dv1 dv2]] for U = dP/du and V = dP/dv.
  const inv = new Matrix3().set(du1, du2, 0, dv1, dv2, 0, 0, 0, 1).invert();
  const [m11, m21, , m12, m22] = inv.elements;
  const right = e1.clone().multiplyScalar(m11).addScaledVector(e2, m21).normalize();
  const up = e1.clone().multiplyScalar(m12).addScaledVector(e2, m22).normalize();
  return { right, up };
}

describe("the ViewCube", () => {
  it("has six faces, one per standard view other than isometric, labelled with its name", () => {
    expect(FACES.map((f) => [f.label, f.preset])).toEqual([
      ["RIGHT", "right"],
      ["LEFT", "left"],
      ["TOP", "top"],
      ["BOTTOM", "bottom"],
      ["FRONT", "front"],
      ["BACK", "back"],
    ]);
    expect(cubeGeometry().groups.map((g) => g.materialIndex)).toEqual([0, 1, 2, 3, 4, 5]);
  });

  it("picks the view that looks at the face clicked head-on: the face a view sees in the middle is that view's", () => {
    const { mesh } = cube();
    const raycaster = new Raycaster();
    for (const { preset } of FACES) {
      // The middle of the cube as the view sees it, and four points around it on the same face.
      const towards = directionOf(PRESETS[preset]);
      const { right, up } = cameraFor({ target: new Vector3(), ...PRESETS[preset], halfHeight: 1 });
      for (const [sx, sy] of [[0, 0], [0.3, 0.3], [-0.3, 0.3], [0.3, -0.3], [-0.3, -0.3]]) {
        const origin = towards.clone().multiplyScalar(4).addScaledVector(right, sx).addScaledVector(up, sy);
        raycaster.set(origin, towards.clone().negate());
        const hit = raycaster.intersectObject(mesh, false)[0];
        expect([preset, sx, sy, FACES[hit.face!.materialIndex].preset]).toEqual([preset, sx, sy, preset]);
      }
    }
  });

  it("shows each face's label upright and left to right in that face's view", () => {
    const { geometry } = cube();
    FACES.forEach(({ preset }, face) => {
      const label = labelAxes(geometry, face);
      const view = cameraFor({ target: new Vector3(), ...PRESETS[preset], halfHeight: 1 });
      expect([preset, label.up.angleTo(view.up), label.right.angleTo(view.right)]).toEqual([preset, expect.closeTo(0, 3), expect.closeTo(0, 3)]);
    });
  });
});
