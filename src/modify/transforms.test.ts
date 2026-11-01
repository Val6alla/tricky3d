import { beforeEach, describe, expect, it, vi } from "vitest";
import { det3 } from "../matrix";
import { boundsOf, makeBox, partBounds, translation } from "../mesh";
import { bakeMesh } from "../ops/bake";
import { useStore } from "../store";
import { useUi } from "../ui";
import { mirrorMatrices, mirrorParts, shearMatrices, shearParts } from "./transforms";

const s = () => useStore.getState();
const lastToast = () => useUi.getState().toasts[useUi.getState().toasts.length - 1]?.message;

beforeEach(() => {
  // Toasts time themselves out.
  vi.stubGlobal("window", { setTimeout: () => 0, clearTimeout: () => {} });
  s().newProject();
  useUi.setState({ toasts: [], dialog: null });
});

function part(at: number[] = translation(30, 40, 5)): string {
  const [id] = s().addParts([{ name: "Box", mesh: makeBox([20, 10, 6]), matrix: at }]);
  return id;
}

describe("Mirror", () => {
  it("flips a part across its centre: same box, turned triangles when baked, and twice is the part as it was", () => {
    const id = part();
    const before = s().parts[0];
    for (const axis of [0, 1, 2] as const) {
      const [m] = mirrorMatrices([before], { axis, pivot: "centre" }).values();
      expect(det3(m)).toBe(-1);
      const flipped = { ...before, matrix: m };
      // The box stays where it was.
      const b0 = partBounds(before)!;
      const b1 = boundsOf(flipped.mesh, flipped.matrix)!;
      for (let k = 0; k < 3; k++) {
        expect(b1.min[k]).toBeCloseTo(b0.min[k], 9);
        expect(b1.max[k]).toBeCloseTo(b0.max[k], 9);
      }
      // Baked, the triangles turn round so the surface still faces out.
      const baked = bakeMesh(flipped.mesh, flipped.matrix);
      expect(Array.from(baked.indices.slice(0, 3))).toEqual([before.mesh.indices[0], before.mesh.indices[2], before.mesh.indices[1]]);
      const [back] = mirrorMatrices([flipped], { axis, pivot: "centre" }).values();
      back.forEach((v, i) => expect(v).toBeCloseTo(before.matrix[i], 9));
    }
    s().select([id]);
    expect(mirrorParts({ axis: 2, pivot: "origin" })).toBe(1);
    // The box runs 2..8 in Z (6 mm, centred at 5): across the origin, −8..−2.
    const z = boundsOf(s().parts[0].mesh, s().parts[0].matrix)!;
    expect([z.min[2], z.max[2]]).toEqual([-8, -2]);
    expect(s().history.past.map((e) => e.label)).toEqual(["Add parts", "Mirror in Z"]);
    mirrorParts({ axis: 2, pivot: "origin" });
    expect(s().parts[0].matrix).toEqual(before.matrix);
  });

  it("leaves locked parts and says what to do without a selection", () => {
    const id = part();
    s().clearSelection();
    expect(mirrorParts({ axis: 0, pivot: "centre" })).toBe(0);
    expect(lastToast()).toBe("Select the parts to mirror first.");
    s().setPartLocked(id, true);
    s().select([id]);
    expect(mirrorParts({ axis: 0, pivot: "centre" })).toBe(0);
    expect(lastToast()).toBe("Hidden or locked parts cannot be mirrored.");
  });
});

describe("Shear", () => {
  it("leans a part about its centre without changing its volume", () => {
    part(translation(0, 0, 0));
    const box = s().parts[0];
    const [m] = shearMatrices([box], { along: 0, by: 2, angle: 45, pivot: "centre" }).values();
    expect(det3(m)).toBeCloseTo(1, 12);
    // The box runs −10..10 in X and −3..3 in Z about centre 0: at 45° the top moves +3 in X, the bottom −3.
    const b = boundsOf(box.mesh, m)!;
    expect(b.min[0]).toBeCloseTo(-13, 9);
    expect(b.max[0]).toBeCloseTo(13, 9);
    expect([b.min[2], b.max[2]]).toEqual([-3, 3]);
    s().select([box.id]);
    expect(shearParts({ along: 1, by: 1, angle: 10, pivot: "centre" })).toBe(0);
    expect(lastToast()).toBe("Pick two different axes.");
    expect(shearParts({ along: 1, by: 0, angle: -20, pivot: "origin" })).toBe(1);
    expect(s().history.past.map((e) => e.label)).toEqual(["Add parts", "Shear"]);
  });
});
