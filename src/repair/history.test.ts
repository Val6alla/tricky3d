import { describe, expect, it } from "vitest";
import { makeBox } from "../mesh";
import type { MeshData } from "../store";
import { REPAIR_HISTORY_LIMITS, pushStep, redoStep, retainedBytes, undoStep } from "./history";
import type { RepairSnapshot } from "./types";

/** A mesh of exactly `bytes` bytes of arrays (positions only). */
const sized = (bytes: number): MeshData => ({ positions: new Float32Array(bytes / 4), indices: new Uint32Array(0) });

const state = (label: string, mesh: MeshData = makeBox([1, 1, 1])): RepairSnapshot => ({ label, mesh, selection: new Uint8Array(12) });

describe("repair history", () => {
  it("has the main history's caps", () => {
    expect(REPAIR_HISTORY_LIMITS).toEqual({ steps: 50, bytes: 2 * 1024 ** 3 });
  });

  it("counts each mesh once and leaves out the meshes the session keeps anyway", () => {
    const a = sized(100);
    const b = sized(40);
    const live = sized(1000);
    expect(retainedBytes([state("1", a), state("2", a), state("3", b), state("4", live)], [live])).toBe(140);
    expect(retainedBytes([], [live])).toBe(0);
  });

  it("drops the oldest steps beyond the step cap", () => {
    let past: RepairSnapshot[] = [];
    const live = [sized(8)];
    for (let i = 1; i <= 7; i++) past = pushStep(past, state(String(i)), live, { steps: 5, bytes: Infinity });
    expect(past.map((s) => s.label)).toEqual(["3", "4", "5", "6", "7"]);
  });

  it("drops the oldest steps while the meshes only it holds exceed the byte cap, but always keeps the newest", () => {
    const current = sized(1000);
    const limits = { steps: 50, bytes: 250 };
    let past: RepairSnapshot[] = [];
    for (const [label, bytes] of [
      ["a", 100],
      ["b", 100],
      ["c", 100],
    ] as const) {
      past = pushStep(past, state(label, sized(bytes)), [current], limits);
    }
    expect(past.map((s) => s.label)).toEqual(["b", "c"]);
    past = pushStep(past, state("huge", sized(10_000)), [current], limits);
    expect(past.map((s) => s.label)).toEqual(["huge"]);
    // The mesh on screen and the starting mesh cost the history nothing.
    past = pushStep([], state("x", current), [current], limits);
    past = pushStep(past, state("y", current), [current], limits);
    expect(past.map((s) => s.label)).toEqual(["x", "y"]);
  });

  it("undoes and redoes with each state carrying the label of its step", () => {
    const m0 = makeBox([1, 1, 1]);
    const m1 = makeBox([2, 2, 2]);
    const m2 = makeBox([3, 3, 3]);
    const sel0 = new Uint8Array(12).fill(1);
    const sel2 = new Uint8Array(12);
    // Two steps: "Close all holes" took m0 to m1, "Wrap" took m1 to m2.
    const past: RepairSnapshot[] = [
      { label: "Close all holes", mesh: m0, selection: sel0 },
      { label: "Wrap", mesh: m1, selection: new Uint8Array(12) },
    ];
    const undone = undoStep(past, [], { mesh: m2, selection: sel2 });
    expect(undone?.target.mesh).toBe(m1);
    expect(undone?.past.map((s) => s.label)).toEqual(["Close all holes"]);
    expect(undone?.future).toEqual([{ label: "Wrap", mesh: m2, selection: sel2 }]);
    if (!undone) return;
    const twice = undoStep(undone.past, undone.future, undone.target);
    expect(twice?.target).toEqual({ label: "Close all holes", mesh: m0, selection: sel0 });
    expect(twice?.future.map((s) => [s.label, s.mesh])).toEqual([
      ["Close all holes", m1],
      ["Wrap", m2],
    ]);
    if (!twice) return;
    expect(undoStep(twice.past, twice.future, twice.target)).toBeNull();
    const redone = redoStep(twice.past, twice.future, twice.target);
    expect(redone?.target.mesh).toBe(m1);
    expect(redone?.past).toEqual([{ label: "Close all holes", mesh: m0, selection: sel0 }]);
    expect(redone?.future.map((s) => s.label)).toEqual(["Wrap"]);
    expect(redoStep(past, [], { mesh: m2, selection: sel2 })).toBeNull();
  });
});
