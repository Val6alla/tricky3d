import { Matrix4, Object3D, OrthographicCamera, PerspectiveCamera } from "three";
import { describe, expect, it } from "vitest";
import { makeBox } from "../mesh";
import { anyTriangleInRect, projectVertices, rectBetween, triangleTouchesRect, trianglesInRect, type ScreenRect } from "./boxSelect";

const W = 800;
const H = 600;
const R = (left: number, top: number, right: number, bottom: number): ScreenRect => ({ left, top, right, bottom });

/** Projection × view of a camera at `position` looking at the origin, Z up, as the viewport sets it. */
function viewProjection(camera: PerspectiveCamera | OrthographicCamera, position: [number, number, number]): number[] {
  Object3D.DEFAULT_UP.set(0, 0, 1);
  camera.position.set(...position);
  camera.lookAt(0, 0, 0);
  camera.updateMatrixWorld();
  camera.updateProjectionMatrix();
  return new Matrix4().multiplyMatrices(camera.projectionMatrix, camera.matrixWorldInverse).toArray();
}

describe("rectBetween", () => {
  it("orders the corners whichever way the drag went", () => {
    expect(rectBetween(50, 40, 10, 90)).toEqual(R(10, 40, 50, 90));
    expect(rectBetween(10, 90, 50, 40)).toEqual(R(10, 40, 50, 90));
  });
});

describe("triangleTouchesRect", () => {
  const r = R(100, 100, 200, 200);
  it("counts a triangle with a corner inside, one round the rectangle, and one crossing it", () => {
    expect(triangleTouchesRect(150, 150, 400, 150, 400, 400, r)).toBe(true);
    expect(triangleTouchesRect(0, 0, 1000, 0, 0, 1000, r)).toBe(true);
    // A long thin triangle through the middle, every corner outside.
    expect(triangleTouchesRect(0, 149, 1000, 150, 0, 151, r)).toBe(true);
  });

  it("leaves out triangles beside it, also ones whose box overlaps the rectangle", () => {
    expect(triangleTouchesRect(300, 300, 400, 300, 300, 400, r)).toBe(false);
    // Bounding box over the rectangle, the triangle itself past its corner.
    expect(triangleTouchesRect(150, 0, 400, 0, 400, 250, r)).toBe(false);
    expect(triangleTouchesRect(150, 0, 400, 0, 400, 250, R(100, 100, 260, 200))).toBe(true);
    expect(triangleTouchesRect(201, 0, 400, 300, 201, 300, r)).toBe(false);
  });

  it("counts touching the border as inside", () => {
    expect(triangleTouchesRect(200, 150, 300, 150, 300, 160, r)).toBe(true);
  });
});

describe("projectVertices and trianglesInRect", () => {
  const box = makeBox([20, 20, 20]);
  const faces = (ids: number[]) => [...new Set(ids.map((t) => Math.floor(t / 2)))];
  // makeBox: faces 0 bottom (-Z), 1 top (+Z), 2 front (-Y), 3 back (+Y), 4 left (-X), 5 right (+X).

  it("selects the faces turned to a perspective camera and leaves the back ones out", () => {
    const m = viewProjection(new PerspectiveCamera(45, W / H, 1, 1000), [60, -80, 70]);
    const screen = projectVertices(box.positions, m, W, H);
    const all = R(0, 0, W, H);
    // From +X, -Y, +Z: the top, front and right faces show.
    expect(faces(trianglesInRect(box.indices, screen, all, true))).toEqual([1, 2, 5]);
    expect(faces(trianglesInRect(box.indices, screen, all, false))).toEqual([0, 1, 2, 3, 4, 5]);
  });

  it("does the same in an orthographic view", () => {
    const camera = new OrthographicCamera(-40, 40, 30, -30, 1, 1000);
    const m = viewProjection(camera, [-50, 60, -40]);
    const screen = projectVertices(box.positions, m, W, H);
    // From -X, +Y, -Z: the bottom, back and left faces show.
    expect(faces(trianglesInRect(box.indices, screen, R(0, 0, W, H), true))).toEqual([0, 3, 4]);
  });

  it("selects only what lies under the rectangle, and nothing beside the part", () => {
    const camera = new OrthographicCamera(-40, 40, 30, -30, 1, 1000);
    // Straight down onto the top face: x runs right, y runs up on the canvas.
    const m = viewProjection(camera, [0, -1e-6, 100]);
    const screen = projectVertices(box.positions, m, W, H);
    // The top face covers x 300..500, y 200..400 on the canvas (10 px per mm).
    const left = trianglesInRect(box.indices, screen, R(290, 250, 320, 260), true);
    expect(left.length).toBeGreaterThan(0);
    expect(faces(left)).toEqual([1]);
    expect(trianglesInRect(box.indices, screen, R(600, 100, 700, 150), true)).toEqual([]);
    expect(anyTriangleInRect(box.indices, screen, R(600, 100, 700, 150))).toBe(false);
    expect(anyTriangleInRect(box.indices, screen, R(495, 395, 700, 500))).toBe(true);
  });

  it("leaves out triangles with a corner behind the camera, and those `keep` rules out", () => {
    const m = viewProjection(new PerspectiveCamera(45, W / H, 1, 1000), [0, 0, 5]);
    const screen = projectVertices(box.positions, m, W, H);
    // The camera is inside the box: the top's corners lie behind it.
    expect(Number.isNaN(screen[2 * 4])).toBe(true);
    const seen = trianglesInRect(box.indices, screen, R(0, 0, W, H), false);
    expect(faces(seen)).toEqual([0]);
    expect(trianglesInRect(box.indices, screen, R(0, 0, W, H), false, (t) => t !== 0)).toEqual([1]);
  });

  it("applies a part's matrix along with the camera", () => {
    const camera = new OrthographicCamera(-40, 40, 30, -30, 1, 1000);
    const view = viewProjection(camera, [0, -1e-6, 100]);
    // Moved 25 mm along +X: its top now covers x 550..750 on the canvas.
    const m = new Matrix4().fromArray(view).multiply(new Matrix4().makeTranslation(25, 0, 0)).toArray();
    const screen = projectVertices(box.positions, m, W, H);
    expect(anyTriangleInRect(box.indices, screen, R(560, 250, 570, 260))).toBe(true);
    expect(anyTriangleInRect(box.indices, screen, R(300, 250, 500, 260))).toBe(false);
  });
});
