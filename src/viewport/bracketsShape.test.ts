import { describe, expect, it } from "vitest";
import type { Bounds } from "../mesh";
import { BRACKET_FLOATS, bracketSegments, MARGIN_SHARE } from "./bracketsShape";

/** Segments as [start, end] point pairs. */
function segments(box: Bounds) {
  const a = bracketSegments(box);
  const out: { start: number[]; end: number[] }[] = [];
  for (let i = 0; i < a.length; i += 6) out.push({ start: [...a.subarray(i, i + 3)], end: [...a.subarray(i + 3, i + 6)] });
  return out;
}

const lengthOf = (s: { start: number[]; end: number[] }) => Math.hypot(...s.start.map((v, i) => s.end[i] - v));
const armAlong = (segs: ReturnType<typeof segments>, axis: number) => lengthOf(segs.find((s) => s.end[axis] !== s.start[axis])!);

describe("bracketSegments", () => {
  it("draws 3 inward arms of 22% of the side at each of the 8 corners", () => {
    const box: Bounds = { min: [10, 20, 0], max: [30, 40, 20] };
    const segs = segments(box);
    expect(bracketSegments(box)).toHaveLength(BRACKET_FLOATS);
    expect(segs).toHaveLength(24);
    const corners = new Set(segs.map((s) => s.start.join()));
    expect(corners.size).toBe(8);
    // 20 mm cube grown by 0.4 mm on every side: 22% of 20.8 mm.
    for (const s of segs) {
      expect(lengthOf(s)).toBeCloseTo(4.576, 5);
      // Exactly one coordinate changes, and it moves towards the box's inside.
      const moved = [0, 1, 2].filter((a) => s.end[a] !== s.start[a]);
      expect(moved).toHaveLength(1);
      const a = moved[0];
      const mid = (box.min[a] + box.max[a]) / 2;
      expect(Math.abs(s.end[a] - mid)).toBeLessThan(Math.abs(s.start[a] - mid));
    }
  });

  it("puts the corners just outside the box, by a share of its largest side", () => {
    const box: Bounds = { min: [0, 0, 0], max: [100, 50, 40] };
    const margin = 100 * MARGIN_SHARE;
    const corners = segments(box).map((s) => s.start);
    for (let axis = 0; axis < 3; axis++) {
      const values = new Set(corners.map((c) => +c[axis].toFixed(4)));
      expect([...values].sort((a, b) => a - b)).toEqual([box.min[axis] - margin, box.max[axis] + margin]);
    }
  });

  it("scales each arm with its own side", () => {
    // Grown by 2 mm per side: 104 × 54 × 44.
    const segs = segments({ min: [0, 0, 0], max: [100, 50, 40] });
    expect([0, 1, 2].map((a) => +armAlong(segs, a).toFixed(4))).toEqual([22.88, 11.88, 9.68]);
  });

  it("keeps thin sides visible but never lets an arm pass the middle", () => {
    // 100 × 100 × 4 plate, grown to 104 × 104 × 8: the 5.2 mm floor applies, capped at half of 8 mm.
    expect(armAlong(segments({ min: [0, 0, 0], max: [100, 100, 4] }), 2)).toBeCloseTo(4, 5);
    // 100 × 100 × 16, grown to 104 × 104 × 20: 22% would be 4.4 mm; the floor lifts it to 5.2 mm.
    expect(armAlong(segments({ min: [0, 0, 0], max: [100, 100, 16] }), 2)).toBeCloseTo(5.2, 5);
  });

  it("stays finite for a flat box", () => {
    const out = bracketSegments({ min: [0, 0, 5], max: [10, 10, 5] });
    expect(out.every(Number.isFinite)).toBe(true);
  });

  it("writes into the given buffer", () => {
    const buffer = new Float32Array(BRACKET_FLOATS);
    expect(bracketSegments({ min: [0, 0, 0], max: [1, 1, 1] }, buffer)).toBe(buffer);
  });
});
