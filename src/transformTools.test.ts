import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { multiply, rotationAbout, scaleAbout, translationMatrix } from "./matrix";
import { boundsOf, centreOf, makeBox, partBounds } from "./mesh";
import { useStore, type GizmoMode, type Part, type TransformMode, type Vec3 } from "./store";
import { useUi } from "./ui";
import {
  applyScale,
  boxSize,
  centreOnPlatform,
  centreOnPlatformMatrices,
  checkScale,
  finishScale,
  formatField,
  groupBox,
  leftOutParts,
  moveToDelta,
  parseField,
  placeOnPlatform,
  placeOnPlatformMatrices,
  proportionalSizes,
  rotateDelta,
  sizeFactors,
  startTransform,
  stepField,
  transformParts,
  transformTargets,
  type ScaleInput,
} from "./transformTools";

function part(id: string, size: Vec3, matrix: number[], locked = false, visible = true): Part {
  return { id, name: id, mesh: makeBox(size), matrix, visible, transparent: false, locked, color: "#000" };
}

function expectVec(actual: readonly number[], expected: readonly number[], eps = 1e-9) {
  actual.forEach((v, i) => expect(Math.abs(v - expected[i])).toBeLessThan(eps));
}

/** A box tilted about X and Y and floating above the platform, so its lowest point is a corner. */
const tilted = part("tilted", [20, 10, 4], multiply(translationMatrix([30, 40, 37]), multiply(rotationAbout(0, 0.6), rotationAbout(1, 0.4))));
const plain = part("plain", [10, 10, 10], translationMatrix([150, 120, -8]));

describe("transform targets", () => {
  it("takes the selected shown, unlocked parts and counts the ones left out", () => {
    const locked = part("locked", [5, 5, 5], translationMatrix([0, 0, 0]), true);
    const hidden = part("hidden", [5, 5, 5], translationMatrix([0, 0, 0]), false, false);
    const both = part("both", [5, 5, 5], translationMatrix([0, 0, 0]), true, false);
    const t = transformTargets({ parts: [tilted, plain, locked, hidden, both], selection: ["locked", "plain", "hidden", "both"] });
    expect(t.parts.map((p) => p.id)).toEqual(["plain"]);
    // A hidden locked part counts once, as locked.
    expect([t.locked, t.hidden]).toEqual([2, 1]);
    expect(leftOutParts(t)).toBe("1 hidden part and 2 locked parts");
    expect(leftOutParts({ hidden: 0, locked: 1 })).toBe("1 locked part");
    expect(leftOutParts({ hidden: 0, locked: 0 })).toBe("");
  });
});

describe("place on platform", () => {
  it("rests each rotated part on Z = 0 and changes nothing else", () => {
    const out = placeOnPlatformMatrices([tilted, plain]);
    for (const p of [tilted, plain]) {
      const m = out.get(p.id)!;
      const before = partBounds(p)!;
      const after = boundsOf(p.mesh, m)!;
      expect(Math.abs(after.min[2])).toBeLessThan(1e-9);
      expectVec([after.min[0], after.min[1], after.max[0], after.max[1]], [before.min[0], before.min[1], before.max[0], before.max[1]]);
      // Only the Z translation changes; the rotation stays exactly as it was.
      expect(m.slice(0, 12)).toEqual(p.matrix.slice(0, 12));
      expect([m[12], m[13]]).toEqual([p.matrix[12], p.matrix[13]]);
    }
  });
});

describe("centre on platform", () => {
  it("centres the combined box in X and Y, keeping Z and the parts' relative positions", () => {
    const out = centreOnPlatformMatrices([tilted, plain], [200, 180, 150]);
    const moved = [tilted, plain].map((p) => ({ ...p, matrix: out.get(p.id)! }));
    const box = groupBox(moved)!;
    const c = centreOf(box);
    expectVec([c[0], c[1]], [100, 90]);
    expectVec([box.min[2], box.max[2]], [groupBox([tilted, plain])!.min[2], groupBox([tilted, plain])!.max[2]]);
    // Both parts moved by the same amount, in X and in Y.
    for (const i of [12, 13]) {
      const d0 = out.get("tilted")![i] - tilted.matrix[i];
      const d1 = out.get("plain")![i] - plain.matrix[i];
      expect(Math.abs(d0 - d1)).toBeLessThan(1e-9);
    }
  });

  it("leaves out rounding noise, so pressing again on rotated parts changes nothing", () => {
    // This rotated part lands a hair off the platform (about 1.8e-15 mm); that is not worth another undo step.
    const odd = part("odd", [20.3, 10.7, 4.1], multiply(translationMatrix([30.123, 40.7, 37.31]), multiply(rotationAbout(0, 0.6), rotationAbout(1, 0.4))));
    const placed = { ...odd, matrix: placeOnPlatformMatrices([odd]).get("odd")! };
    expect(partBounds(placed)!.min[2]).not.toBe(0);
    expect(Math.abs(partBounds(placed)!.min[2])).toBeLessThan(1e-12);
    expect(placeOnPlatformMatrices([placed]).size).toBe(0);

    const parts = [odd, tilted, plain];
    const centredMatrices = centreOnPlatformMatrices(parts, [200, 180, 150]);
    const centred = parts.map((p) => ({ ...p, matrix: centredMatrices.get(p.id)! }));
    expect(centreOnPlatformMatrices(centred, [200, 180, 150]).size).toBe(0);
  });
});

describe("dialog maths", () => {
  it("moves the box minimum to an absolute position", () => {
    const box = partBounds(tilted)!;
    const delta = moveToDelta(box, "min", [0, 0, 0]);
    const after = boundsOf(tilted.mesh, multiply(delta, tilted.matrix))!;
    expectVec(after.min, [0, 0, 0]);
  });

  it("treats a target within rounding noise of the current position as no move", () => {
    const box = { min: [10, 20, 1.7763568394002505e-15] as Vec3, max: [30, 40, 5] as Vec3 };
    expect(moveToDelta(box, "min", [10, 20, 0])).toEqual(translationMatrix([0, 0, 0]));
  });

  it("moves the box centre to an absolute position", () => {
    const box = partBounds(plain)!;
    const delta = moveToDelta(box, "centre", [5, 6, 7]);
    expectVec(centreOf(boundsOf(plain.mesh, multiply(delta, plain.matrix))!), [5, 6, 7]);
  });

  it("rotates 90° about the selection centre, swapping the box's width and depth", () => {
    const p = part("p", [40, 10, 6], translationMatrix([50, 50, 3]));
    const box = partBounds(p)!;
    const after = boundsOf(p.mesh, multiply(rotateDelta(2, 90, centreOf(box)), p.matrix))!;
    expectVec(boxSize(after), [10, 40, 6]);
    expectVec(centreOf(after), centreOf(box));
  });

  it("scales the world box exactly by the per-axis factors, even for a rotated part", () => {
    const box = partBounds(tilted)!;
    const f: Vec3 = [2, 0.5, 1.5];
    const after = boundsOf(tilted.mesh, multiply(scaleAbout(f, centreOf(box)), tilted.matrix))!;
    const size = boxSize(box);
    expectVec(boxSize(after), [size[0] * 2, size[1] * 0.5, size[2] * 1.5], 1e-6);
    expectVec(centreOf(after), centreOf(box), 1e-6);
  });

  it("turns target sizes into factors", () => {
    expect(sizeFactors([20, 10, 5], [40, 0, 0], true, 0)).toEqual([2, 2, 2]);
    expect(sizeFactors([20, 10, 5], [40, 30, 5], false, 0)).toEqual([2, 3, 1]);
    // A flat part cannot be sized along its flat axis.
    expect(sizeFactors([20, 10, 0], [40, 10, 7], false, 0)).toEqual([2, 1, 1]);
    expect(sizeFactors([20, 10, 0], [0, 0, 7], true, 2)).toBeNull();
  });

  it("works out proportional sizes from one axis", () => {
    expect(proportionalSizes([20, 10, 5], 1, 30)).toEqual([60, 30, 15]);
    expect(proportionalSizes([20, 10, 0], 0, 40)).toEqual([40, 20, 0]);
    // Nothing to scale from: a flat axis, or a value that is not a positive number.
    expect(proportionalSizes([20, 10, 0], 2, 7)).toBeNull();
    expect(proportionalSizes([20, 10, 5], 0, 0)).toBeNull();
    expect(proportionalSizes([20, 10, 5], 0, NaN)).toBeNull();
  });
});

describe("dialog fields", () => {
  it("reads a decimal comma or point, with or without a unit", () => {
    expect(parseField("12,5")).toBe(12.5);
    expect(parseField("12.5")).toBe(12.5);
    expect(parseField(" 12.5 mm ")).toBe(12.5);
    expect(parseField("-0,25")).toBe(-0.25);
    expect(parseField("22,5°")).toBe(22.5);
    expect(parseField("50,5 %")).toBe(50.5);
    expect(parseField("1e3")).toBe(1000);
  });

  it("gives NaN for anything that is not one number", () => {
    for (const text of ["", "  ", "abc", "5O", "12,5,1", "1.2.3", "1,234.5", "--1", "12 cm", "°", "Infinity"]) {
      expect(parseField(text)).toBeNaN();
    }
  });

  it("formats up to 3 decimals without trailing zeros or -0", () => {
    expect(formatField(12.5)).toBe("12.5");
    expect(formatField(1 / 3)).toBe("0.333");
    expect(formatField(-0.0001)).toBe("0");
    expect(formatField(20)).toBe("20");
  });

  it("steps the typed value, also from a comma, and leaves text that is no number", () => {
    expect(stepField("12,5", 1)).toBe("13.5");
    expect(stepField("0.2", -1)).toBe("-0.8");
    expect(stepField("0.1 mm", 10)).toBe("10.1");
    expect(stepField("-1", 1)).toBe("0");
    expect(stepField("", 1)).toBeNull();
    expect(stepField("abc", -1)).toBeNull();
  });

  it("moves, rotates and scales by the typed comma values", () => {
    const box = partBounds(plain)!;
    const target = ["12,5", "0", "0"].map(parseField) as Vec3;
    const after = boundsOf(plain.mesh, multiply(moveToDelta(box, "min", target), plain.matrix))!;
    expectVec(after.min, [12.5, 0, 0]);

    const m = rotateDelta(2, parseField("22,5"), [0, 0, 0]);
    expect((Math.atan2(m[1], m[0]) * 180) / Math.PI).toBeCloseTo(22.5, 9);

    const current: Vec3 = [20, 10, 5];
    const percent = checkScale({ bySize: false, keep: true, percent: ["50,5", "", ""].map(parseField), size: [], driver: 0, current });
    expect(percent).toEqual({ factors: [0.505, 0.505, 0.505], error: null, bad: [] });
    const size = checkScale({ bySize: true, keep: false, percent: [], size: ["12,5 mm", "10", "5"].map(parseField), driver: 0, current });
    expect(size.factors).toEqual([0.625, 1, 1]);
    // Text that is not a number still names the field to fix.
    const bad = checkScale({ bySize: false, keep: false, percent: ["100", "5O", "100"].map(parseField), size: [], driver: 0, current });
    expect(bad).toMatchObject({ error: "Enter a number for X, Y and Z.", bad: [1] });
  });
});

describe("scale dialog check", () => {
  const base: ScaleInput = { bySize: false, keep: true, percent: [100, 100, 100], size: [20, 10, 5], driver: 0, current: [20, 10, 5] };
  const check = (patch: Partial<ScaleInput>) => checkScale({ ...base, ...patch });

  it("reads percentages, mirrored from the driver when proportions are kept", () => {
    expect(check({ percent: [200, 200, 200] })).toEqual({ factors: [2, 2, 2], error: null, bad: [] });
    expect(check({ percent: [50, 999, 999], driver: 0 }).factors).toEqual([0.5, 0.5, 0.5]);
    expect(check({ keep: false, percent: [200, 50, 100] }).factors).toEqual([2, 0.5, 1]);
  });

  it("names the percentage fields to fix", () => {
    expect(check({ keep: false, percent: [100, NaN, 100] })).toMatchObject({ error: "Enter a number for X, Y and Z.", bad: [1] });
    expect(check({ percent: [NaN, NaN, NaN] })).toMatchObject({ error: "Enter a number for the scale.", bad: [0, 1, 2] });
    expect(check({ keep: false, percent: [0, 100, 200000] })).toMatchObject({ error: "Enter a scale from 0.01 to 100000 %.", bad: [0, 2] });
  });

  it("refuses a percentage whose resulting size is over the limit, still giving the result", () => {
    const r = check({ keep: false, current: [20000, 20, 20], percent: [1000, 1000, 100] });
    expect(r.error).toBe("The new size can be at most 100000 mm.");
    expect(r.bad).toEqual([0]);
    expect(r.factors).toEqual([10, 10, 1]);
  });

  it("reads target sizes with and without the lock", () => {
    expect(check({ bySize: true, size: [40, 20, 10] })).toEqual({ factors: [2, 2, 2], error: null, bad: [] });
    expect(check({ bySize: true, keep: false, size: [40, 30, 5] }).factors).toEqual([2, 3, 1]);
    expect(check({ bySize: true, keep: false, size: [40, NaN, 5] })).toMatchObject({ error: "Enter a size for each axis.", bad: [1] });
    expect(check({ bySize: true, keep: false, size: [0, 10, 200000] })).toMatchObject({
      error: "Enter a size greater than 0 and up to 100000 mm.",
      bad: [0, 2],
    });
  });

  it("checks the sizes the lock derives, not just the typed one", () => {
    // A 20000 × 20 × 20 bar with Y typed as 2000 would make X 2000000 mm.
    const r = check({ bySize: true, current: [20000, 20, 20], size: [2000000, 2000, 2000], driver: 1 });
    expect(r.error).toBe("The new size can be at most 100000 mm.");
    expect(r.bad).toEqual([0]);
    // Exactly the limit is fine despite rounding in size × (target / size).
    expect(147 * (100000 / 147)).toBeGreaterThan(100000);
    expect(check({ bySize: true, current: [100, 120, 147], size: [0, 0, 100000], driver: 2 }).error).toBe(null);
    expect(check({ bySize: true, keep: false, current: [143.3, 151, 147], size: [100000, 100000, 100000] }).error).toBe(null);
  });

  it("handles a flat part", () => {
    const flat: Vec3 = [20, 10, 0];
    // Without the lock the flat axis is not read and keeps factor 1.
    expect(check({ bySize: true, keep: false, current: flat, size: [40, 10, NaN] }).factors).toEqual([2, 1, 1]);
    // Driving from the flat axis gives no factors.
    expect(check({ bySize: true, current: flat, size: [20, 10, 7], driver: 2 })).toMatchObject({
      factors: null,
      error: "The selection has no size to scale.",
    });
  });

  it("keeps the factor within 0.01 % to 100000 % of the current size", () => {
    const r = check({ bySize: true, keep: false, current: [0.001, 10, 10], size: [5, 10, 10] });
    expect(r.error).toBe("The new size must be 0.01 % to 100000 % of the current size.");
    expect(r.bad).toEqual([0]);
    expect(check({ bySize: true, keep: false, current: [1000, 10, 10], size: [0.01, 10, 10] }).bad).toEqual([0]);
  });
});

describe("commands on the store", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    useStore.getState().newProject();
  });

  const matrixOf = (id: string) => useStore.getState().parts.find((p) => p.id === id)!.matrix;

  it("place on platform and a dialog transform each record one undo step", () => {
    const store = useStore.getState();
    const [id] = store.addParts([{ name: "cube", mesh: makeBox([20, 20, 20]), matrix: translationMatrix([0, 0, 30]) }]);
    const past = useStore.getState().history.past.length;

    placeOnPlatform();
    expect(useStore.getState().history.past.length).toBe(past + 1);
    expect(useStore.getState().history.past[past].label).toBe("Place on platform");
    const placed = useStore.getState().parts.find((p) => p.id === id)!;
    expect(partBounds(placed)!.min[2]).toBe(0);

    expect(transformParts([id], scaleAbout([2, 2, 2], [0, 0, 10]), "Scale")).toBe(true);
    expect(useStore.getState().history.past.length).toBe(past + 2);
    expect(boxSize(partBounds(useStore.getState().parts.find((p) => p.id === id)!)!)).toEqual([40, 40, 40]);

    // A scale to nothing is refused and records nothing.
    expect(transformParts([id], scaleAbout([0, 1, 1]), "Scale")).toBe(false);
    expect(useStore.getState().history.past.length).toBe(past + 2);

    useStore.getState().undo();
    useStore.getState().undo();
    expect(matrixOf(id)).toEqual(translationMatrix([0, 0, 30]));
  });

  it("transforms only the given parts that are still selected, shown and unlocked", () => {
    const [a, b] = useStore.getState().addParts([
      { name: "a", mesh: makeBox([10, 10, 10]), matrix: translationMatrix([0, 0, 5]) },
      { name: "b", mesh: makeBox([10, 10, 10]), matrix: translationMatrix([50, 0, 5]) },
    ]);
    const [c] = useStore.getState().addParts([{ name: "c", mesh: makeBox([10, 10, 10]), matrix: translationMatrix([90, 0, 5]) }]);
    // An import selected c; a dialog opened for a must move neither a (no longer selected) nor c.
    const past = useStore.getState().history.past.length;
    expect(transformParts([a], translationMatrix([0, 0, 50]), "Move")).toBe(true);
    expect(useStore.getState().history.past.length).toBe(past);
    expect(matrixOf(a)).toEqual(translationMatrix([0, 0, 5]));
    expect(matrixOf(c)).toEqual(translationMatrix([90, 0, 5]));

    // With a and b selected and b hidden, only a moves.
    useStore.getState().setPartsVisible([b], false);
    useStore.getState().select([a, b], "replace");
    expect(transformParts([a, b], translationMatrix([0, 0, 50]), "Move")).toBe(true);
    expect(matrixOf(a)).toEqual(translationMatrix([0, 0, 55]));
    expect(matrixOf(b)).toEqual(translationMatrix([50, 0, 5]));
  });

  it("records no step and says so when a rotated part already rests on the platform", () => {
    vi.stubGlobal("window", { setTimeout: () => 0 });
    const [id] = useStore.getState().addParts([{ name: "tilted", mesh: tilted.mesh, matrix: tilted.matrix }]);
    const past = useStore.getState().history.past.length;
    placeOnPlatform();
    centreOnPlatform();
    expect(useStore.getState().history.past.length).toBe(past + 2);
    const before = useStore.getState().parts;
    useUi.setState({ toasts: [] });
    placeOnPlatform();
    centreOnPlatform();
    expect(useStore.getState().history.past.length).toBe(past + 2);
    expect(useStore.getState().parts).toBe(before);
    expect(useUi.getState().toasts.map((t) => t.message)).toEqual([
      "The selection already rests on the platform.",
      "The selection is already centred on the platform.",
    ]);
    expect(Math.abs(partBounds(useStore.getState().parts.find((p) => p.id === id)!)!.min[2])).toBeLessThan(1e-9);
  });

  it("explains why hidden or locked parts are not moved", () => {
    vi.stubGlobal("window", { setTimeout: () => 0 });
    const [a, b] = useStore.getState().addParts([
      { name: "a", mesh: makeBox([10, 10, 10]), matrix: translationMatrix([0, 0, 30]) },
      { name: "b", mesh: makeBox([10, 10, 10]), matrix: translationMatrix([50, 0, 30]) },
    ]);
    useStore.getState().setPartsVisible([b], false);
    useStore.getState().select([a, b], "replace");
    useUi.setState({ toasts: [] });
    placeOnPlatform();
    expect(useUi.getState().toasts.map((t) => t.message)).toEqual(["1 hidden part was not moved."]);
    expect(partBounds(useStore.getState().parts.find((p) => p.id === a)!)!.min[2]).toBe(0);
    expect(partBounds(useStore.getState().parts.find((p) => p.id === b)!)!.min[2]).toBe(25);

    useStore.getState().select([b], "replace");
    useUi.setState({ toasts: [] });
    centreOnPlatform();
    expect(useUi.getState().toasts.map((t) => t.message)).toEqual(["Hidden parts cannot be moved. Show them first."]);
  });
});

describe("gizmo tool around a command", () => {
  beforeEach(() => {
    // Saving the remembered setting then fails quietly, whatever the Node version offers.
    vi.stubGlobal("localStorage", undefined);
    useStore.getState().setView({ alwaysShowGizmo: true });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    useUi.getState().closeDialog();
    useStore.getState().newProject();
    useStore.getState().setView({ alwaysShowGizmo: true });
  });

  it("Move and Rotate pick the move and rotate gizmo, Scale the scale gizmo, and each opens its dialog", () => {
    useStore.getState().addParts([{ name: "cube", mesh: makeBox([10, 10, 10]) }]);
    const table: [TransformMode, GizmoMode][] = [
      ["translate", "combined"],
      ["rotate", "combined"],
      ["scale", "scale"],
    ];
    for (const [mode, gizmo] of table) {
      useStore.getState().setGizmoMode(null);
      useUi.getState().closeDialog();
      startTransform(mode);
      expect(useStore.getState().gizmoMode).toBe(gizmo);
      expect(useUi.getState().dialog).toEqual({ kind: "transform", mode });
    }
  });

  it("finishScale ends Scale only: to Select while Always show gizmo is on, else to the move and rotate gizmo", () => {
    const table: [boolean, GizmoMode | null, GizmoMode | null][] = [
      [true, "scale", null],
      [false, "scale", "combined"],
      // Outside Scale nothing changes.
      [true, "combined", "combined"],
      [false, "combined", "combined"],
      [true, null, null],
      [false, null, null],
    ];
    for (const [alwaysShowGizmo, before, after] of table) {
      useStore.getState().setView({ alwaysShowGizmo });
      useStore.getState().setGizmoMode(before);
      finishScale();
      expect(useStore.getState().gizmoMode).toBe(after);
    }
  });

  /** A selected 10 mm cube resting on the platform, in Scale mode. */
  function cubeInScale(): string {
    useStore.getState().newProject();
    const [id] = useStore.getState().addParts([{ name: "cube", mesh: makeBox([10, 10, 10]), matrix: translationMatrix([0, 0, 5]) }]);
    useStore.getState().setGizmoMode("scale");
    return id;
  }

  it("the Scale dialog's OK scales as one step and ends Scale: to Select while Always show gizmo is on, else to the move and rotate gizmo", () => {
    const table: [boolean, GizmoMode | null][] = [
      [true, null],
      [false, "combined"],
    ];
    for (const [alwaysShowGizmo, after] of table) {
      useStore.getState().setView({ alwaysShowGizmo });
      const id = cubeInScale();
      const past = useStore.getState().history.past.length;
      expect(applyScale([id], scaleAbout([2, 2, 2], [0, 0, 5]))).toBe(true);
      expect(useStore.getState().history.past.length).toBe(past + 1);
      expect(useStore.getState().history.past[past].label).toBe("Scale");
      expect(boxSize(partBounds(useStore.getState().parts[0])!)).toEqual([20, 20, 20]);
      expect(useStore.getState().gizmoMode).toBe(after);
    }
  });

  it("a refused scale (a factor of 0) changes nothing and stays in Scale, so the dialog can say why", () => {
    for (const alwaysShowGizmo of [true, false]) {
      useStore.getState().setView({ alwaysShowGizmo });
      const id = cubeInScale();
      const { parts, history } = useStore.getState();
      expect(applyScale([id], scaleAbout([0, 1, 1], [0, 0, 5]))).toBe(false);
      expect(useStore.getState().parts).toBe(parts);
      expect(useStore.getState().history).toBe(history);
      expect(useStore.getState().gizmoMode).toBe("scale");
    }
  });
});
