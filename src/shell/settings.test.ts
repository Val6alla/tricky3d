import { afterEach, describe, expect, it, vi } from "vitest";
import { primeMeshCaches } from "../meshCache";
import { makeBox, translation } from "../mesh";
import type { Part } from "../store";
import {
  checkFields,
  DEFAULT_FIELDS,
  estimateParts,
  estimateText,
  heavyWarning,
  KINDS,
  loadFields,
  refusalText,
  resultName,
  saveFields,
  SETTINGS_KEY,
} from "./settings";
import type { ShellEstimate } from "./types";

/** The window's storage, kept in a map. */
function fakeStorage(entries: Record<string, string> = {}) {
  const data = new Map(Object.entries(entries));
  return { data, getItem: (key: string) => data.get(key) ?? null, setItem: (key: string, value: string) => void data.set(key, value) };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the fields", () => {
  it("lists the five kinds in the dialog's order", () => {
    expect(KINDS.map((k) => k.label)).toEqual(["Hollow part", "Inner offset", "Outer offset", "Hollowing shell", "Hollow part, improved"]);
  });

  it("turns good fields into settings", () => {
    expect(checkFields(DEFAULT_FIELDS)).toEqual({ settings: { kind: "hollow-part", thickness: 2, accuracy: 0.25, smooth: false }, problem: null, bad: null });
    expect(checkFields({ ...DEFAULT_FIELDS, kind: "outer-offset", thickness: " 3,5 ", accuracy: "1.75", smooth: true }).settings).toEqual({
      kind: "outer-offset",
      thickness: 3.5,
      accuracy: 1.75,
      smooth: true,
    });
  });

  it("says what is wrong, in Rust's words, and with which field", () => {
    expect(checkFields({ ...DEFAULT_FIELDS, thickness: "0" })).toMatchObject({ settings: null, bad: "thickness", problem: "Enter a shell thickness greater than 0 and up to 1000 mm." });
    expect(checkFields({ ...DEFAULT_FIELDS, thickness: "abc" }).bad).toBe("thickness");
    expect(checkFields({ ...DEFAULT_FIELDS, thickness: "1001" }).bad).toBe("thickness");
    expect(checkFields({ ...DEFAULT_FIELDS, accuracy: "0.001" })).toMatchObject({ bad: "accuracy", problem: "Enter an accuracy of at least 0.01 mm." });
    expect(checkFields({ ...DEFAULT_FIELDS, thickness: "2", accuracy: "1.5" })).toMatchObject({
      bad: "accuracy",
      problem: "Enter an accuracy of at most half the thickness (1 mm).",
    });
    expect(checkFields({ ...DEFAULT_FIELDS, thickness: "2", accuracy: "1" }).settings).not.toBeNull();
  });
});

describe("remembered settings", () => {
  it("start from the defaults, then come back as last used", () => {
    const storage = fakeStorage();
    vi.stubGlobal("localStorage", storage);
    expect(loadFields()).toEqual(DEFAULT_FIELDS);
    saveFields({ kind: "hollow-part-improved", thickness: 1.5, accuracy: 0.3, smooth: true }, true);
    expect(JSON.parse(storage.data.get(SETTINGS_KEY) ?? "")).toEqual({ kind: "hollow-part-improved", thickness: 1.5, accuracy: 0.3, smooth: true, removeOriginal: true });
    expect(loadFields()).toEqual({ kind: "hollow-part-improved", thickness: "1.5", accuracy: "0.3", smooth: true, removeOriginal: true });
  });

  it("drop what does not fit, field by field", () => {
    for (const junk of ["", "{", "null", "7", '"x"']) {
      vi.stubGlobal("localStorage", fakeStorage({ [SETTINGS_KEY]: junk }));
      expect(loadFields()).toEqual(DEFAULT_FIELDS);
    }
    vi.stubGlobal("localStorage", fakeStorage({ [SETTINGS_KEY]: JSON.stringify({ kind: "melt", thickness: -1, accuracy: "x", smooth: "yes", removeOriginal: true }) }));
    expect(loadFields()).toEqual({ ...DEFAULT_FIELDS, removeOriginal: true });
    // No storage at all: the defaults, and saving does no harm.
    vi.stubGlobal("localStorage", undefined);
    expect(loadFields()).toEqual(DEFAULT_FIELDS);
    expect(() => saveFields({ kind: "inner-offset", thickness: 1, accuracy: 0.5, smooth: false }, false)).not.toThrow();
  });
});

describe("words", () => {
  it("names the new part after the original and what was made", () => {
    expect(resultName("Bracket", "hollow-part")).toBe("Bracket (hollow)");
    expect(resultName("Bracket", "hollow-part-improved")).toBe("Bracket (hollow)");
    expect(resultName("Bracket", "inner-offset")).toBe("Bracket (inner offset)");
    expect(resultName("Bracket", "outer-offset")).toBe("Bracket (outer offset)");
    expect(resultName("Bracket", "hollowing-shell")).toBe("Bracket (hollowing shell)");
  });

  it("says why a part is refused", () => {
    const ok = { source: 0, closed: true, oriented: true, holes: 0, borderEdges: 0, nonManifoldEdges: 0, invertedTriangles: 0 };
    expect(refusalText("Lid", { ...ok, closed: false, holes: 4, borderEdges: 30 })).toBe("“Lid” is not closed (4 holes).");
    expect(refusalText("Lid", { ...ok, closed: false, nonManifoldEdges: 1 })).toBe("“Lid” is not closed (1 open edge).");
    expect(refusalText("Lid", { ...ok, oriented: false, invertedTriangles: 1200 })).toBe("“Lid” is not oriented (1,200 inverted triangles).");
    expect(refusalText("Lid", { ...ok, closed: false, holes: 1, oriented: false })).toBe("“Lid” is not closed (1 hole) and not oriented.");
  });

  it("puts the estimate in words, and warns before a heavy run", () => {
    const e: ShellEstimate = { points: 2.14e6, measured: 1e6, megabytes: 38, seconds: 1.2, slow: false, heavy: false, refused: null };
    expect(estimateText(e)).toBe("About 2.1 million grid points: 40 MB of memory, a few seconds.");
    expect(estimateText({ ...e, points: 950_000, megabytes: 2400, seconds: 340 })).toBe("About 950,000 grid points: 2.4 GB of memory, about 6 minutes.");
    expect(estimateText({ ...e, points: 3.2e9, seconds: 7200 })).toContain("3.2 billion grid points");
    expect(heavyWarning(e)).toBeNull();
    expect(heavyWarning({ ...e, slow: true })).toMatch(/^This run will take a long time\./);
    expect(heavyWarning({ ...e, heavy: true })).toMatch(/^This run will need a lot of memory\./);
    expect(heavyWarning({ ...e, slow: true, heavy: true })).toMatch(/long time and need a lot of memory/);
  });
});

describe("the estimate's parts", () => {
  const part = (matrix: number[]): Part => ({ id: "p", name: "p", mesh: makeBox([20, 10, 4]), matrix, visible: true, transparent: false, locked: false, color: "#fff" });

  it("measure world boxes, with the area scaled as the part is", () => {
    const p = part(translation(5, 5, 5));
    // Before the stats are in, the box's surface stands in.
    expect(estimateParts([p])).toEqual([{ size: [20, 10, 4], area: 2 * (200 + 40 + 80), triangles: 12 }]);
    primeMeshCaches(p.mesh, { stats: { triangles: 12, vertices: 8, volume: 800, area: 640, shells: 1, openEdges: 0, nonManifoldEdges: 0 } });
    const doubled = part([2, 0, 0, 0, 0, 2, 0, 0, 0, 0, 2, 0, 0, 0, 0, 1]);
    doubled.mesh = p.mesh;
    expect(estimateParts([doubled])).toEqual([{ size: [40, 20, 8], area: 640 * 4, triangles: 12 }]);
  });
});
