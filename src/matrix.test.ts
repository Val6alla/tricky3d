import { describe, expect, it } from "vitest";
import {
  applyDelta,
  det3,
  invert,
  multiply,
  rotationAbout,
  scaleAbout,
  transformPoint,
  translationMatrix,
} from "./matrix";

const I = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];

function expectClose(actual: readonly number[], expected: readonly number[], eps = 1e-9) {
  expect(actual.length).toBe(expected.length);
  actual.forEach((v, i) => expect(Math.abs(v - expected[i])).toBeLessThan(eps));
}

/** A rotated, non-uniformly scaled and translated matrix, like a part after a few edits. */
const messy = multiply(
  translationMatrix([12.5, -3, 40]),
  multiply(rotationAbout(0, 0.7), multiply(rotationAbout(2, -1.1), scaleAbout([2, 0.5, 3]))),
);

describe("matrix", () => {
  it("multiplies with the right-hand matrix applied first", () => {
    const t = translationMatrix([10, 0, 0]);
    const s = scaleAbout([2, 2, 2]);
    // Scale first, then move: (1, 0, 0) → (2, 0, 0) → (12, 0, 0).
    expect(transformPoint(multiply(t, s), [1, 0, 0])).toEqual([12, 0, 0]);
    // Move first, then scale: (1, 0, 0) → (11, 0, 0) → (22, 0, 0).
    expect(transformPoint(multiply(s, t), [1, 0, 0])).toEqual([22, 0, 0]);
  });

  it("leaves a matrix unchanged, bit for bit, when multiplied by the identity", () => {
    expect(multiply(I, messy)).toEqual(messy);
    expect(multiply(messy, I)).toEqual(messy);
  });

  it("changes only the translation, exactly, when a translation is applied", () => {
    const out = multiply(translationMatrix([1, 2, 3]), messy);
    expect(out.slice(0, 12)).toEqual(messy.slice(0, 12));
    expect(out.slice(12)).toEqual([messy[12] + 1, messy[13] + 2, messy[14] + 3, 1]);
  });

  it("inverts an affine matrix", () => {
    const inv = invert(messy)!;
    expectClose(multiply(messy, inv), I);
    expectClose(multiply(inv, messy), I);
    expectClose(transformPoint(inv, transformPoint(messy, [3, -7, 11])), [3, -7, 11]);
  });

  it("has no inverse for a matrix that flattens space", () => {
    expect(invert(scaleAbout([1, 0, 1]))).toBeNull();
  });

  it("computes the determinant of the linear part", () => {
    expect(det3(scaleAbout([2, 3, 4], [5, 5, 5]))).toBeCloseTo(24, 12);
    expect(det3(scaleAbout([-1, 1, 1]))).toBe(-1);
    expect(det3(rotationAbout(1, 0.3, [1, 2, 3]))).toBeCloseTo(1, 12);
    expect(det3(messy)).toBeCloseTo(3, 12);
  });

  it("rotates about a pivot and keeps the pivot fixed", () => {
    const pivot: [number, number, number] = [10, 20, 30];
    for (const axis of [0, 1, 2] as const) {
      const r = rotationAbout(axis, 1.234, pivot);
      expectClose(transformPoint(r, pivot), pivot);
    }
    // Counter-clockwise seen from +Z: +X turns to +Y.
    expect(transformPoint(rotationAbout(2, Math.PI / 2, pivot), [11, 20, 30])).toEqual([10, 21, 30]);
    // Seen from +X: +Y turns to +Z. Seen from +Y: +Z turns to +X.
    expect(transformPoint(rotationAbout(0, Math.PI / 2), [0, 1, 0])).toEqual([0, 0, 1]);
    expect(transformPoint(rotationAbout(1, Math.PI / 2), [0, 0, 1])).toEqual([1, 0, 0]);
  });

  it("keeps right-angle turns exact", () => {
    const quarter = rotationAbout(0, Math.PI / 2, [3, 4, 5]);
    let m = [...I];
    for (let i = 0; i < 4; i++) m = multiply(quarter, m);
    expect(m).toEqual(I);
    expect(rotationAbout(2, Math.PI)).toEqual([-1, 0, 0, 0, 0, -1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]);
  });

  it("scales along the world axes about a pivot", () => {
    const s = scaleAbout([2, 3, 0.5], [10, 10, 10]);
    expect(transformPoint(s, [10, 10, 10])).toEqual([10, 10, 10]);
    expect(transformPoint(s, [11, 11, 12])).toEqual([12, 13, 11]);
  });

  it("applies a delta to every part, keyed by id", () => {
    const parts = [
      { id: "a", matrix: I },
      { id: "b", matrix: messy },
    ];
    const delta = rotationAbout(2, 0.5, [1, 1, 1]);
    const out = applyDelta(parts, delta);
    expect([...out.keys()]).toEqual(["a", "b"]);
    expect(out.get("a")).toEqual(multiply(delta, I));
    expect(out.get("b")).toEqual(multiply(delta, messy));
  });
});
