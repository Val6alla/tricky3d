import { beforeEach, describe, expect, it } from "vitest";
import {
  activePlanes,
  DEFAULT_CUSTOM_NORMAL,
  formatMm,
  isBoxCut,
  isPointClipped,
  normalized,
  offsetRange,
  planeMargin,
  reanchorOffset,
  rowOffset,
  sceneBounds,
  sliderStep,
  useClip,
  type ClipAxis,
  type ClipRow,
  type ClipSettings,
  type PlaneSpec,
} from "./clip";
import { makeBox, partBounds, translation, type Bounds } from "./mesh";
import type { Part, Vec3 } from "./store";

function cube(id: string, size: number, at: Vec3, visible = true): Part {
  return {
    id,
    name: id,
    mesh: makeBox([size, size, size]),
    matrix: translation(...at),
    visible,
    transparent: false,
    locked: false,
    color: "#aaa",
  };
}

const off: ClipRow = { enabled: false, offset: null, flipped: false };

function settings(rows: Partial<Record<ClipAxis, Partial<ClipRow>>>, customNormal: Vec3 = [0, 0, 1]): ClipSettings {
  const row = (axis: ClipAxis): ClipRow => ({ ...off, ...rows[axis] });
  return {
    rows: { x: row("x"), y: row("y"), z: row("z"), custom: row("custom") },
    customNormal,
    transparentCuts: false,
    cutScope: "selected",
  };
}

const BOX: Bounds = { min: [0, 0, 0], max: [100, 50, 20] };
const corners = (b: Bounds): Vec3[] =>
  [0, 1, 2, 3, 4, 5, 6, 7].map((i) => [i & 1 ? b.max[0] : b.min[0], i & 2 ? b.max[1] : b.min[1], i & 4 ? b.max[2] : b.min[2]]);

describe("sceneBounds", () => {
  it("is the platform box when there are no parts", () => {
    expect(sceneBounds([200, 150, 100], [])).toEqual({ min: [0, 0, 0], max: [200, 150, 100] });
  });

  it("grows to visible parts outside the platform and ignores hidden ones", () => {
    const parts = [cube("a", 20, [-30, 10, 10]), cube("b", 10, [500, 500, 500], false), cube("c", 4, [50, 50, 300])];
    expect(sceneBounds([200, 200, 200], parts)).toEqual({ min: [-40, 0, 0], max: [200, 200, 302] });
  });
});

describe("offsetRange", () => {
  it("is the box's extent along an axis", () => {
    expect(offsetRange([1, 0, 0], BOX)).toEqual([0, 100]);
    expect(offsetRange([0, 0, 1], BOX)).toEqual([0, 20]);
  });

  it("matches projecting the 8 corners for any direction", () => {
    const b: Bounds = { min: [-12, 3, -7], max: [40, 19, 33] };
    for (const d of [[0.6, -0.8, 0], DEFAULT_CUSTOM_NORMAL, [-1, 0, 0], normalized([-2, 1, -3])!] as Vec3[]) {
      const dots = corners(b).map((c) => d[0] * c[0] + d[1] * c[1] + d[2] * c[2]);
      const [lo, hi] = offsetRange(d, b);
      expect(lo).toBeCloseTo(Math.min(...dots), 10);
      expect(hi).toBeCloseTo(Math.max(...dots), 10);
    }
  });
});

describe("rowOffset", () => {
  it("starts in the middle and clamps to the range", () => {
    expect(rowOffset(off, [0, 100])).toBe(50);
    expect(rowOffset({ ...off, offset: 130 }, [0, 100])).toBe(100);
    expect(rowOffset({ ...off, offset: -5 }, [0, 100])).toBe(0);
    expect(rowOffset({ ...off, offset: 12.5 }, [0, 100])).toBe(12.5);
  });
});

describe("activePlanes", () => {
  it("is empty when no row is enabled", () => {
    expect(activePlanes(settings({ x: { offset: 10 } }), BOX)).toEqual([]);
  });

  it("keeps coordinate <= offset by default and the other side when flipped", () => {
    const [plane] = activePlanes(settings({ x: { enabled: true, offset: 30 } }), BOX);
    expect(isPointClipped([plane], [29, 0, 0])).toBe(false);
    expect(isPointClipped([plane], [30, 0, 0])).toBe(false);
    expect(isPointClipped([plane], [31, 0, 0])).toBe(true);

    const [flipped] = activePlanes(settings({ x: { enabled: true, offset: 30, flipped: true } }), BOX);
    expect(isPointClipped([flipped], [29, 0, 0])).toBe(true);
    expect(isPointClipped([flipped], [31, 0, 0])).toBe(false);
  });

  it("uses the middle of the range for a row without an offset", () => {
    const [plane] = activePlanes(settings({ z: { enabled: true } }), BOX);
    expect(plane.normal).toEqual([-0, -0, -1]);
    expect(plane.constant).toBe(10 + planeMargin(BOX));
  });

  it("cuts at the clamped offset the panel shows", () => {
    const [plane] = activePlanes(settings({ y: { enabled: true, offset: 500 } }), BOX);
    expect(plane.constant).toBe(50 + planeMargin(BOX));
  });

  it("keeps the side the custom normal points away from", () => {
    const n = normalized([1, 1, 0])!;
    const planes = activePlanes(settings({ custom: { enabled: true, offset: 0 } }, n), { min: [-10, -10, -10], max: [10, 10, 10] });
    expect(isPointClipped(planes, [-1, -1, 5])).toBe(false);
    expect(isPointClipped(planes, [1, 1, 5])).toBe(true);
    expect(isPointClipped(planes, [3, -3, 0])).toBe(false); // on the plane

    const flipped = activePlanes(settings({ custom: { enabled: true, offset: 0, flipped: true } }, n), BOX);
    expect(isPointClipped(flipped, [-1, -1, 5])).toBe(true);
    expect(isPointClipped(flipped, [1, 1, 5])).toBe(false);
  });

  it("keeps a face lying on the plane whole, even with the GPU's rounding error", () => {
    // Test cubes at X 90..110 and 115..135 on the default platform; sliders move in whole mm.
    const scene = sceneBounds([200, 200, 200], [cube("a", 20, [100, 100, 10]), cube("b", 20, [125, 100, 10])]);
    const gpuError = 1e-6 * 350; // about 1e-6 of the scene's size, see planeMargin
    const [keepBelow] = activePlanes(settings({ x: { enabled: true, offset: 115 } }), scene);
    expect(isPointClipped([keepBelow], [115 + gpuError, 100, 10])).toBe(false);
    expect(isPointClipped([keepBelow], [115.01, 100, 10])).toBe(true);
    const [keepAbove] = activePlanes(settings({ x: { enabled: true, offset: 110, flipped: true } }), scene);
    expect(isPointClipped([keepAbove], [110 - gpuError, 100, 10])).toBe(false);
    expect(isPointClipped([keepAbove], [109.99, 100, 10])).toBe(true);
  });

  it("moves the planes by less than the 0.01 mm the panel shows on a usual platform", () => {
    expect(planeMargin(sceneBounds([200, 200, 200], []))).toBeGreaterThan(1e-3);
    expect(planeMargin(sceneBounds([200, 200, 200], []))).toBeLessThan(0.005);
    expect(planeMargin(sceneBounds([2000, 2000, 2000], []))).toBeCloseTo(10 * planeMargin(sceneBounds([200, 200, 200], [])), 12);
  });

  it("hides a point when any enabled plane removes it", () => {
    const planes = activePlanes(settings({ x: { enabled: true, offset: 50 }, z: { enabled: true, offset: 10 } }), BOX);
    expect(planes).toHaveLength(2);
    expect(isPointClipped(planes, [40, 0, 5])).toBe(false);
    expect(isPointClipped(planes, [60, 0, 5])).toBe(true);
    expect(isPointClipped(planes, [40, 0, 15])).toBe(true);
    expect(isPointClipped(planes, [60, 0, 15])).toBe(true);
  });
});

describe("isBoxCut", () => {
  const scene: Bounds = { min: [0, 0, 0], max: [200, 200, 200] };
  const z150 = activePlanes(settings({ z: { enabled: true, offset: 150 } }), scene);
  const cube20 = partBounds(cube("a", 20, [100, 100, 10])); // 90..110, 90..110, 0..20

  it("is false for a box the planes do not reach and true for one they cut", () => {
    expect(isBoxCut(z150, cube20)).toBe(false);
    expect(isBoxCut(activePlanes(settings({ z: { enabled: true, offset: 10 } }), scene), cube20)).toBe(true);
    // One plane that misses and one that cuts: the box is cut.
    const both = activePlanes(settings({ x: { enabled: true, offset: 105 }, z: { enabled: true, offset: 150 } }), scene);
    expect(isBoxCut(both, cube20)).toBe(true);
  });

  it("counts a box entirely on the removed side as cut and a face on the plane as not", () => {
    expect(isBoxCut(activePlanes(settings({ x: { enabled: true, offset: 50 } }), scene), cube20)).toBe(true);
    expect(isBoxCut(activePlanes(settings({ x: { enabled: true, offset: 110 } }), scene), cube20)).toBe(false);
    expect(isBoxCut(activePlanes(settings({ x: { enabled: true, offset: 90, flipped: true } }), scene), cube20)).toBe(false);
  });

  it("is false with no planes or no box", () => {
    expect(isBoxCut([], cube20)).toBe(false);
    expect(isBoxCut(z150, null)).toBe(false);
  });

  it("matches testing the 8 corners against each plane", () => {
    const b: Bounds = { min: [-12, 3, -7], max: [40, 19, 33] };
    const cornerTest = (planes: PlaneSpec[]) => corners(b).some((c) => isPointClipped(planes, c));
    for (const normal of [[1, 0, 0], [0, -1, 0], DEFAULT_CUSTOM_NORMAL, normalized([-2, 1, -3])!] as Vec3[]) {
      for (const constant of [-60, -20, -5, 0, 5, 20, 60]) {
        const planes: PlaneSpec[] = [{ normal, constant }];
        expect(isBoxCut(planes, b)).toBe(cornerTest(planes));
      }
    }
  });
});

describe("reanchorOffset", () => {
  it("turns the plane about the point it passes nearest the centre", () => {
    const b: Bounds = { min: [0, 0, 0], max: [20, 20, 20] }; // centre (10, 10, 10)
    // z = 14 passes nearest the centre at (10, 10, 14); along +X that point is at x = 10.
    expect(reanchorOffset(14, [0, 0, 1], [1, 0, 0], b)).toBeCloseTo(10, 10);
    // Reversing the normal describes the same plane.
    expect(reanchorOffset(14, [0, 0, 1], [0, 0, -1], b)).toBeCloseTo(-14, 10);
    const d = normalized([1, 0, 1])!;
    expect(reanchorOffset(14, [0, 0, 1], d, b)).toBeCloseTo((10 + 14) / Math.SQRT2, 10);
  });
});

describe("helpers", () => {
  it("normalises and rejects zero vectors", () => {
    expect(normalized([0, 3, 4])).toEqual([0, 0.6, 0.8]);
    expect(normalized([0, 0, 0])).toBeNull();
    expect(normalized([NaN, 0, 1])).toBeNull();
  });

  it("formats offsets with 2 decimals", () => {
    expect(formatMm(12.345)).toBe("12.35");
    expect(formatMm(-0.001)).toBe("0.00");
    expect(formatMm(-7)).toBe("-7.00");
  });

  it("picks 1-2-5 slider steps", () => {
    expect(sliderStep(200)).toBe(1);
    expect(sliderStep(248)).toBe(1);
    expect(sliderStep(500)).toBe(2);
    expect(sliderStep(1000)).toBe(5);
    expect(sliderStep(40)).toBeCloseTo(0.2, 12);
    expect(sliderStep(0)).toBe(1);
  });
});

describe("useClip", () => {
  beforeEach(() => {
    useClip.setState({ customNormal: DEFAULT_CUSTOM_NORMAL, cutScope: "selected" });
    useClip.getState().reset();
  });

  it("starts a newly enabled plane in the middle of its range and keeps it after", () => {
    const s = useClip.getState();
    s.setEnabled("x", true, [0, 200]);
    expect(useClip.getState().rows.x).toEqual({ enabled: true, offset: 100, flipped: false });
    s.setOffset("x", 42);
    s.setEnabled("x", false, [0, 200]);
    s.setEnabled("x", true, [0, 300]);
    expect(useClip.getState().rows.x.offset).toBe(42);
  });

  it("reset turns everything off and forgets offsets, keeping the direction and scope", () => {
    const s = useClip.getState();
    s.setEnabled("z", true, [0, 10]);
    s.setFlipped("z", true);
    s.setTransparentCuts(true);
    s.setCutScope("all");
    s.setCustomNormal([1, 0, 0], BOX);
    s.reset();
    const after = useClip.getState();
    expect(after.rows.z).toEqual(off);
    expect(after.transparentCuts).toBe(false);
    expect(after.cutScope).toBe("all");
    expect(after.customNormal).toEqual([1, 0, 0]);
  });

  it("normalises the custom normal, ignores zero, and pivots a placed plane", () => {
    const s = useClip.getState();
    expect(s.setCustomNormal([0, 0, 0], BOX)).toBe(false);
    expect(useClip.getState().customNormal).toEqual(DEFAULT_CUSTOM_NORMAL);

    expect(s.setCustomNormal([0, 0, 5], BOX)).toBe(true);
    expect(useClip.getState().customNormal).toEqual([0, 0, 1]);
    s.setEnabled("custom", true, [0, 20]);
    s.setOffset("custom", 15);
    s.setCustomNormal([-1, 0, 0], BOX);
    // z = 15 passes nearest the centre (50, 25, 10) at (50, 25, 15): x = 50, so -x = -50.
    expect(useClip.getState().rows.custom.offset).toBeCloseTo(-50, 10);
  });
});
