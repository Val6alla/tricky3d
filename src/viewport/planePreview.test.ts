import { Mesh, Raycaster, Vector3, type Material } from "three";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useCutPreview } from "../cutPreview";
import type { Bounds } from "../mesh";
import type { CutPlane } from "../ops/types";
import { createPlanePreview, QUAD_MARGIN, quadPlacement } from "./planePreview";
import type { VpTheme } from "./theme";

const BOX: Bounds = { min: [0, 0, 0], max: [20, 20, 20] };
const DIAGONAL = Math.hypot(20, 20, 20);
const Z_AT_5: CutPlane = { normal: [0, 0, 1], offset: 5 };

/** Every token the viewport reads; only the accent matters here. */
const theme = (accent: string): VpTheme => ({
  platform: "#000",
  gridMinor: "#000",
  gridMajor: "#000",
  ruler: "#000",
  selected: "#000",
  backface: "#000",
  openEdge: "#000",
  edge: "#000",
  axisX: "#000",
  axisY: "#000",
  axisZ: "#000",
  gizmoCentre: "#000",
  repairHole: "#ffcf26",
  repairError: "#ff8a1c",
  repairDegenerate: "#e04ce0",
  repairSelected: "#33c25a",
  repairSlim: "#3d8bf2",
  cutPoint: "#ff5cb1",
  cutCurrent: "#d4006f",
  cutLine: "#e8202a",
  node: "#18b4f0",
  measure: "#1d6ee6",
  measureSelected: "#8b3dff",
  measurePending: "#e0368c",
  booleanMain: "#3aa655",
  booleanMainSelected: "#86e09a",
  booleanTool: "#d64545",
  booleanToolSelected: "#ff9a9a",
  surface: "#000",
  surface2: "#000",
  border: "#000",
  borderStrong: "#000",
  text: "#000",
  textMuted: "#000",
  accent,
  font: "sans-serif",
});

afterEach(() => useCutPreview.getState().setPlane(null));

describe("quadPlacement", () => {
  it("centres the quad on the box centre's projection onto the plane, a diagonal plus the margin wide", () => {
    const q = quadPlacement(Z_AT_5, BOX);
    expect(q.centre).toEqual([10, 10, 5]);
    expect(q.side).toBeCloseTo(DIAGONAL * (1 + QUAD_MARGIN), 9);
  });

  it("puts the centre on a tilted plane, at the box centre's foot", () => {
    const n = Math.SQRT1_2;
    const plane: CutPlane = { normal: [n, 0, n], offset: 4 };
    const [x, y, z] = quadPlacement(plane, BOX).centre;
    expect(n * x + n * z).toBeCloseTo(4, 9);
    // The step from the box centre to the foot runs along the normal.
    expect(x - 10).toBeCloseTo(z - 10, 9);
    expect(y).toBe(10);
  });

  it("stays visible for a box with no size", () => {
    const point: Bounds = { min: [3, 3, 3], max: [3, 3, 3] };
    expect(quadPlacement(Z_AT_5, point).side).toBeGreaterThan(0);
  });
});

describe("createPlanePreview", () => {
  it("shows the dialog's plane, follows it live, and hides it when the plane is cleared", () => {
    const render = vi.fn();
    const preview = createPlanePreview(theme("#0b8a8f"), render);
    expect(preview.group.visible).toBe(false);
    expect(preview.reach()).toBeNull();

    useCutPreview.getState().setPlane(Z_AT_5, BOX);
    expect(render).toHaveBeenCalledTimes(1);
    expect(preview.group.visible).toBe(true);
    expect(preview.group.position.toArray()).toEqual([10, 10, 5]);
    expect(preview.group.scale.x).toBeCloseTo(DIAGONAL * 1.1, 9);
    expect(new Vector3(0, 0, 1).applyQuaternion(preview.group.quaternion).toArray()).toEqual([0, 0, 1]);
    // Covers the whole quad, corners included, for the camera's near and far planes.
    expect(preview.reach()?.radius).toBeCloseTo((DIAGONAL * 1.1) / Math.SQRT2, 9);

    useCutPreview.getState().setPlane({ normal: [1, 0, 0], offset: 12 }, BOX);
    expect(render).toHaveBeenCalledTimes(2);
    expect(preview.group.position.toArray()).toEqual([12, 10, 10]);
    const facing = new Vector3(0, 0, 1).applyQuaternion(preview.group.quaternion);
    expect(facing.x).toBeCloseTo(1, 9);
    expect(facing.length()).toBeCloseTo(1, 9);

    useCutPreview.getState().setPlane(null);
    expect(render).toHaveBeenCalledTimes(3);
    expect(preview.group.visible).toBe(false);
    expect(preview.reach()).toBeNull();
    preview.dispose();
  });

  it("picks up a plane already set when the viewport is built", () => {
    useCutPreview.getState().setPlane(Z_AT_5, BOX);
    const render = vi.fn();
    const preview = createPlanePreview(theme("#0b8a8f"), render);
    expect(preview.group.visible).toBe(true);
    expect(render).not.toHaveBeenCalled();
    preview.dispose();
  });

  it("is never hit by picking", () => {
    const preview = createPlanePreview(theme("#0b8a8f"), () => {});
    useCutPreview.getState().setPlane(Z_AT_5, BOX);
    preview.group.updateMatrixWorld(true);
    const ray = new Raycaster(new Vector3(10, 10, 50), new Vector3(0, 0, -1));
    expect(ray.intersectObject(preview.group, true)).toEqual([]);
    // The same quad as a plain mesh in the same place is hit, so the ray does cross it.
    const fill = preview.group.children[0] as Mesh;
    const control = new Mesh(fill.geometry, fill.material);
    control.matrixAutoUpdate = false;
    control.matrixWorld.copy(fill.matrixWorld);
    expect(ray.intersectObject(control, false).length).toBeGreaterThan(0);
    preview.dispose();
  });

  it("draws translucent, writes no depth, ignores the clip planes and takes the accent of either theme", () => {
    const preview = createPlanePreview(theme("#0b8a8f"), () => {});
    const materials = preview.group.children.map((c) => (c as Mesh).material as Material & { color: { getHexString(): string } });
    expect(materials).toHaveLength(2);
    for (const m of materials) {
      expect(m.transparent).toBe(true);
      expect(m.depthWrite).toBe(false);
      expect(m.clippingPlanes ?? []).toEqual([]);
      expect(m.color.getHexString()).toBe("0b8a8f");
    }
    expect(materials[0].opacity).toBeCloseTo(0.25, 9);
    preview.applyTheme(theme("#2bb3b1"));
    for (const m of materials) expect(m.color.getHexString()).toBe("2bb3b1");
    preview.dispose();
  });

  it("frees its GPU objects and stops listening on dispose", () => {
    const render = vi.fn();
    const preview = createPlanePreview(theme("#0b8a8f"), render);
    const disposed = vi.fn();
    for (const child of preview.group.children) {
      const { geometry, material } = child as Mesh;
      geometry.addEventListener("dispose", disposed);
      (material as Material).addEventListener("dispose", disposed);
    }
    preview.dispose();
    expect(disposed).toHaveBeenCalledTimes(4);
    useCutPreview.getState().setPlane(Z_AT_5, BOX);
    expect(render).not.toHaveBeenCalled();
  });
});
