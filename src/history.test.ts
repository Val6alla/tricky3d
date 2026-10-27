import { describe, expect, it } from "vitest";
import { emptyHistory, pushEntry, redoStep, retainedBytes, undoStep, type HistoryEntry } from "./history";
import type { Part } from "./store";

function part(id: string, bytes: number): Part {
  return {
    id,
    name: id,
    mesh: { positions: new Float32Array(bytes / 8), indices: new Uint32Array(bytes / 8) },
    matrix: [],
    visible: true,
    transparent: false,
    locked: false,
    color: "#000",
  };
}

const entry = (label: string, before: Part[], after: Part[]): HistoryEntry => ({
  label,
  before: { parts: before, selection: [] },
  after: { parts: after, selection: [] },
});

describe("history", () => {
  it("undoes and redoes in order, and a new step clears redo", () => {
    const a = part("a", 64);
    let h = pushEntry(emptyHistory(), entry("Add", [], [a]), [a]);
    h = pushEntry(h, entry("Delete", [a], []), []);
    const u1 = undoStep(h)!;
    expect(u1.entry.label).toBe("Delete");
    const u2 = undoStep(u1.history)!;
    expect(u2.entry.label).toBe("Add");
    expect(undoStep(u2.history)).toBeNull();
    const r = redoStep(u2.history)!;
    expect(r.entry.label).toBe("Add");
    expect(pushEntry(r.history, entry("X", [a], [a]), [a]).future).toEqual([]);
  });

  it("keeps at most `steps` entries", () => {
    let h = emptyHistory();
    for (let i = 0; i < 60; i++) h = pushEntry(h, entry(String(i), [], []), []);
    expect(h.past.length).toBe(50);
    expect(h.past[0].label).toBe("10");
  });

  it("drops the oldest steps when meshes only the history holds exceed the byte budget", () => {
    const big = [part("p0", 800), part("p1", 800), part("p2", 800)];
    const limits = { steps: 50, bytes: 1700 };
    let h = emptyHistory();
    for (const p of big) h = pushEntry(h, entry(`del ${p.id}`, [p], []), [], limits);
    // Each step keeps one 800-byte mesh alive; two fit in 1700 bytes.
    expect(h.past.map((e) => e.label)).toEqual(["del p1", "del p2"]);
    // A mesh still in the scene costs the history nothing.
    expect(retainedBytes(h.past, [big[1]])).toBe(800);
  });
});
