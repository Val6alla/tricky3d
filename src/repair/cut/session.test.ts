import { describe, expect, it } from "vitest";
import type { Vec3 } from "../../store";
import { formatPosition, nearestCorner } from "../nodes";
import { polylineSegments } from "../../viewport/toolLayer";
import { brokenSpans, insertionIndex } from "./session";
import { DEFAULT_CUT, pathStale, type CutPoint } from "./store";

const pts = (...at: Vec3[]) => at.map((p) => ({ at: p }));

describe("insertionIndex", () => {
  const line = pts([0, 0, 0], [10, 0, 0], [10, 10, 0]);

  it("puts a point after the current one at last position, at the end when none is current", () => {
    expect(insertionIndex(line, 2, [0, 10, 0], "last", false)).toBe(3);
    expect(insertionIndex(line, 0, [0, 10, 0], "last", false)).toBe(1);
    expect(insertionIndex(line, -1, [0, 10, 0], "last", false)).toBe(3);
    expect(insertionIndex([], -1, [0, 10, 0], "nearest", false)).toBe(0);
  });

  it("puts a point where it lengthens the line least at nearest position", () => {
    // Beside the first stretch.
    expect(insertionIndex(line, 2, [5, -1, 0], "nearest", false)).toBe(1);
    // Beside the second.
    expect(insertionIndex(line, 0, [11, 5, 0], "nearest", false)).toBe(2);
    // Past either end of an open line.
    expect(insertionIndex(line, 1, [-5, 0, 0], "nearest", false)).toBe(0);
    expect(insertionIndex(line, 1, [10, 15, 0], "nearest", false)).toBe(3);
    // A closed line has no ends, but a stretch from the last point back to the first.
    expect(insertionIndex(line, 1, [4, 6, 0], "nearest", true)).toBe(3);
    // One point: after it.
    expect(insertionIndex(pts([0, 0, 0]), 0, [5, 0, 0], "nearest", false)).toBe(1);
  });
});

describe("the cut line's state", () => {
  const points: CutPoint[] = [
    { id: 1, at: [0, 0, 0], triangle: 0 },
    { id: 2, at: [1, 0, 0], triangle: 1 },
    { id: 3, at: [1, 1, 0], triangle: 2 },
  ];

  it("is stale until routed for these points and the settings that change the route", () => {
    expect(pathStale({ points, settings: DEFAULT_CUT, routedFor: null })).toBe(true);
    const routedFor = { points, settings: DEFAULT_CUT };
    expect(pathStale({ points, settings: DEFAULT_CUT, routedFor })).toBe(false);
    // Automatic update and the insert order do not change the line.
    expect(pathStale({ points, settings: { ...DEFAULT_CUT, auto: false, order: "nearest" }, routedFor })).toBe(false);
    expect(pathStale({ points, settings: { ...DEFAULT_CUT, snap: 0.1 }, routedFor })).toBe(true);
    expect(pathStale({ points: [...points], settings: DEFAULT_CUT, routedFor })).toBe(true);
  });

  it("names the pieces that found no way along the surface, once routed", () => {
    const path = { pieces: [new Float32Array(6), null], spans: [[0, 1], [1, 2]] as [number, number][], anchors: points.map((p) => p.at) };
    const routedFor = { points, settings: DEFAULT_CUT };
    expect(brokenSpans({ path, points, settings: DEFAULT_CUT, routedFor })).toEqual(["points 2 and 3"]);
    expect(brokenSpans({ path, points: points.slice(0, 2), settings: DEFAULT_CUT, routedFor })).toEqual([]);
  });
});

describe("Move nodes' helpers", () => {
  it("picks the corner nearest the point hit, and shows positions to four decimals", () => {
    const mesh = { positions: new Float32Array([0, 0, 0, 10, 0, 0, 0, 10, 0]), indices: new Uint32Array([0, 1, 2]) };
    expect(nearestCorner(mesh, 0, [1, 1, 0])).toBe(0);
    expect(nearestCorner(mesh, 0, [7, 1, 0])).toBe(1);
    expect(nearestCorner(mesh, 0, [1, 6, 0])).toBe(2);
    expect(formatPosition([1, -2.5, 1 / 3])).toBe("X 1.0000  Y -2.5000  Z 0.3333");
  });

  it("draws a polyline as segment pairs", () => {
    expect(Array.from(polylineSegments(new Float32Array([0, 0, 0, 1, 0, 0, 1, 1, 0])))).toEqual([0, 0, 0, 1, 0, 0, 1, 0, 0, 1, 1, 0]);
    expect(polylineSegments(new Float32Array([1, 2, 3])).length).toBe(0);
  });
});
