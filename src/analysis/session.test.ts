import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Rust is a stand-in that answers the way the test says; everything else runs as in the app.
const ipc = vi.hoisted(() => ({ runAnalysis: vi.fn() }));
const repairIpc = vi.hoisted(() => ({ cancelRepair: vi.fn() }));
vi.mock("./ipc", () => ipc);
vi.mock("../repair/ipc", async (importOriginal) => ({ ...(await importOriginal<typeof import("../repair/ipc")>()), ...repairIpc }));

import { infoRows } from "../info";
import { makeBox, translation } from "../mesh";
import { CANCELLED } from "../repair/ipc";
import { useStore } from "../store";
import { useUi } from "../ui";
import { barOf, compareColor, histogram, wallColor } from "./colors";
import { readAnalyses, toBase64, writeAnalyses } from "./persist";
import { resultCells, resultTables } from "./rows";
import {
  cancelAnalysis,
  closeAnalysisView,
  COMPARE_PARTS,
  COMPARE_PICK,
  probeAt,
  deleteAnalysis,
  isOutOfDate,
  newAnalysis,
  ONE_PART,
  PICK_PART,
  runPartAnalysis,
  shownAnalysis,
  useAnalysis,
} from "./session";
import { DEFAULT_WALL, type CompareSurface, type PartAnalysis, type StandardResult, type WallResult, type WallSettings } from "./types";

const s = () => useStore.getState();
const labels = () => s().history.past.map((e) => e.label);
const lastToast = () => useUi.getState().toasts[useUi.getState().toasts.length - 1]?.message;

const spread = { min: 1, max: 2, average: 1.5, deviation: 0.5 };
/** What Rust answers for a 20 mm cube at (100, 100, 10). */
const CUBE: StandardResult = {
  min: [90, 90, 0],
  max: [110, 110, 20],
  size: [20, 20, 20],
  volume: 8000,
  volumeValid: true,
  area: 2400,
  points: 8,
  triangles: 12,
  edges: 18,
  shells: 1,
  holes: 0,
  boundaryEdges: 0,
  boundaryLength: 0,
  flippedTriangles: 0,
  badEdges: 0,
  closed: true,
  orientable: true,
  edgesPerPoint: spread,
  trianglesPerEdge: { min: 2, max: 2, average: 2, deviation: 0 },
  triangleQuality: spread,
  edgeLength: spread,
};

function cube(name = "Cube") {
  return s().addParts([{ name, mesh: makeBox([20, 20, 20]), matrix: translation(100, 100, 10) }])[0];
}

beforeEach(() => {
  vi.stubGlobal("isTauri", true);
  vi.stubGlobal("window", { setTimeout: () => 0, clearTimeout: () => {} });
  ipc.runAnalysis.mockReset();
  repairIpc.cancelRepair.mockResolvedValue(undefined);
  s().newProject();
  useAnalysis.setState({ busy: null, shown: null });
  useUi.setState({ toasts: [], dialog: null, tasks: [], viewTool: null, ribbonTab: "home" });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("starting an analysis", () => {
  it("needs exactly one selected part, and says so otherwise", () => {
    newAnalysis("standard");
    expect(lastToast()).toBe(PICK_PART);
    const a = cube("A");
    const b = cube("B");
    s().select([a, b]);
    newAnalysis("gravity");
    expect(lastToast()).toBe(ONE_PART);
    expect(ipc.runAnalysis).not.toHaveBeenCalled();
  });

  it("asks for the settings of the kinds that have them", () => {
    const id = cube();
    newAnalysis("skin");
    expect(useUi.getState().dialog).toEqual({ kind: "analysis-settings", analysis: "skin", partId: id });
    useUi.setState({ dialog: null });
    newAnalysis("support");
    expect(useUi.getState().dialog).toEqual({ kind: "analysis-settings", analysis: "support", partId: id });
    expect(ipc.runAnalysis).not.toHaveBeenCalled();
  });
});

describe("a finished analysis", () => {
  it("goes on its part as one undo step, shows, and replaces an earlier one of its kind", async () => {
    const id = cube();
    ipc.runAnalysis.mockResolvedValue({ result: CUBE, drawing: {} });
    const first = await runPartAnalysis("standard", id, null);
    const [kind, settings, sent] = ipc.runAnalysis.mock.calls[0];
    expect([kind, settings, sent[0].name, sent[0].matrix]).toEqual(["standard", null, "Cube", translation(100, 100, 10)]);
    expect(s().parts[0].analyses?.map((a) => a.id)).toEqual([first]);
    expect(labels()).toEqual(["Add parts", "Standard analysis"]);
    expect(useAnalysis.getState().shown).toEqual({ partId: id, id: first });
    expect(useUi.getState().ribbonTab).toBe("analysis");

    ipc.runAnalysis.mockResolvedValue({ result: { area: 400 }, drawing: { triangles: new Float32Array(9) } });
    const shadow = await runPartAnalysis("shadow", id, null);
    ipc.runAnalysis.mockResolvedValue({ result: { ...CUBE, triangles: 99 }, drawing: {} });
    const again = await runPartAnalysis("standard", id, null);
    // Re-running replaces the old result in its place.
    expect(s().parts[0].analyses?.map((a) => [a.kind, a.id])).toEqual([
      ["standard", again],
      ["shadow", shadow],
    ]);
    expect(again).not.toBe(first);

    // OK leaves the view, back to the tab before; the analysis stays.
    closeAnalysisView();
    expect(useAnalysis.getState().shown).toBeNull();
    expect(useUi.getState().ribbonTab).toBe("home");
    // Undo takes the newest result back off.
    s().undo();
    expect(s().parts[0].analyses?.find((a) => a.kind === "standard")?.id).toBe(first);
  });

  it("is deleted from its part as one undo step, and the view closes", async () => {
    const id = cube();
    ipc.runAnalysis.mockResolvedValue({ result: { area: 400 }, drawing: {} });
    const done = (await runPartAnalysis("shadow", id, null)) as string;
    deleteAnalysis(id, done);
    expect(s().parts[0].analyses).toBeUndefined();
    expect(useAnalysis.getState().shown).toBeNull();
    expect(labels()[labels().length - 1]).toBe("Delete Shadow area");
    s().undo();
    expect(s().parts[0].analyses?.length).toBe(1);
  });

  it("is out of date once its part moves, and goes when the part does", async () => {
    const id = cube();
    ipc.runAnalysis.mockResolvedValue({ result: { area: 400 }, drawing: {} });
    await runPartAnalysis("shadow", id, null);
    const found = shownAnalysis();
    expect(found && isOutOfDate(found.part, found.analysis)).toBe(false);
    s().setPartMatrices(new Map([[id, translation(0, 0, 10)]]), "Move");
    const moved = shownAnalysis();
    expect(moved && isOutOfDate(moved.part, moved.analysis)).toBe(true);
    s().deleteParts([id]);
    expect(useAnalysis.getState().shown).toBeNull();
  });

  it("leaves nothing behind when cancelled or when Rust says why it cannot run", async () => {
    const id = cube();
    ipc.runAnalysis.mockImplementation(async () => {
      cancelAnalysis();
      throw CANCELLED;
    });
    await runPartAnalysis("standard", id, null);
    expect(lastToast()).toBe("Standard analysis was cancelled. Nothing changed.");
    expect(s().parts[0].analyses).toBeUndefined();
    expect(labels()).toEqual(["Add parts"]);
    ipc.runAnalysis.mockRejectedValue("Shadow area needs a closed, sound mesh, and \"Cube\" has 1 hole. Repair the part first.");
    await runPartAnalysis("shadow", id, null);
    expect(lastToast()).toContain("has 1 hole");
    expect(s().parts[0].analyses).toBeUndefined();
    expect(useAnalysis.getState().busy).toBeNull();
  });
});

describe("results", () => {
  const standard = (r: StandardResult): PartAnalysis => ({ id: "x", kind: "standard", settings: null, result: r, matrix: translation(100, 100, 10), drawing: {} });

  it("show volume, area and size as the Information panel does for the same part", () => {
    const part = { ...s().parts[0], ...{ id: "p", name: "Cube", mesh: makeBox([20, 20, 20]), matrix: translation(100, 100, 10), visible: true, transparent: false, locked: false, color: "#fff" } };
    const info = infoRows([part], () => ({ triangles: 12, vertices: 8, volume: 8000, area: 2400, shells: 1, openEdges: 0, nonManifoldEdges: 0 })).rows;
    const text = (label: string) => info.find((r) => r.label === label)?.text;
    const tables = resultTables(standard(CUBE));
    const value = (label: string) => tables[1].rows.find((r) => r[0] === label)?.[1];
    expect(value("Volume (cm³)")).toBe(text("Volume (cm³)"));
    expect(value("Area (cm²)")).toBe(text("Area (cm²)"));
    expect(tables[0].rows[2]).toEqual(["Size", text("Length (mm)"), text("Width (mm)"), text("Height (mm)")]);
    expect(value("Triangles")).toBe(text("Triangles"));
    expect(value("Shells")).toBe(text("Shells"));
    // Open edges: no volume, as the panel shows none.
    expect(resultTables(standard({ ...CUBE, volumeValid: false }))[1].rows[0]).toEqual(["Volume (cm³)", "—"]);
  });

  it("copy and CSV name the part and the analysis, then every table", () => {
    const cells = resultCells(standard(CUBE), "Cube");
    expect(cells.slice(0, 4)).toEqual([["Part", "Cube"], ["Analysis", "Standard analysis"], [], ["Position (mm)", "X", "Y", "Z"]]);
    expect(cells).toContainEqual(["Surface is closed", "Yes"]);
    expect(cells).toContainEqual(["Triangles per edge", "2.00", "2.00", "2.00", "0.00"]);
    expect(cells).toContainEqual(["Mesh", "Min", "Max", "Avg", "Dev"]);
  });
});

describe("a project", () => {
  it("keeps the analyses with their drawings, and drops what does not fit", () => {
    const skin: PartAnalysis = {
      id: "s1",
      kind: "skin",
      settings: { upskins: true, upAngle: 45, downskins: false, downAngle: 30, minComponent: 10, filterSmall: true },
      result: { up: { analyzed: true, angle: 45, area: 400, components: 1 }, down: { analyzed: false, angle: 30, area: 0, components: 0 } },
      matrix: translation(1, 2, 3),
      drawing: { classes: Uint8Array.from([0, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 2]) },
    };
    const shadow: PartAnalysis = {
      id: "h1",
      kind: "shadow",
      settings: null,
      result: { area: 200.5 },
      matrix: translation(0, 0, 0),
      drawing: { triangles: Float32Array.from([0, 0, 0, 1, 0, 0, 0, 1, 0]), lines: Float32Array.from([0, 0, 0, 1.5, 0, 0]) },
    };
    const back = readAnalyses(writeAnalyses([skin, shadow]), 12);
    expect(back).toEqual([skin, shadow]);
    // Colours for a different mesh are left out; the rest of the analysis stays.
    expect(readAnalyses(writeAnalyses([skin]), 10)?.[0].drawing).toEqual({});
    // A damaged entry is left out, the others kept.
    const raw = JSON.parse(writeAnalyses([skin, shadow])) as Record<string, unknown>[];
    raw[0].result = { up: "lots" };
    expect(readAnalyses(JSON.stringify(raw), 12)?.map((a) => a.id)).toEqual(["h1"]);
    expect(readAnalyses("not json", 12)).toBeUndefined();
    expect(readAnalyses(undefined, 12)).toBeUndefined();
  });
});

describe("mesh compare", () => {
  const SIDE = { maxPositive: 1, maxNegative: -1, averageSigned: 0, maxUnsigned: 1, averageUnsigned: 0.5, points: 3 };
  const surface = (role: "reference" | "comparison"): CompareSurface => ({
    role,
    positions: Float32Array.from([0, 0, 0, 1, 0, 0, 0, 1, 0]),
    indices: Uint32Array.from([0, 1, 2]),
    values: Float32Array.from([-1, 0, 1]),
  });

  it("takes the reference first and the comparison second, and refuses more parts", () => {
    const a = cube("A");
    const b = cube("B");
    const c = cube("C");
    s().select([a, b]);
    newAnalysis("compare");
    expect(useUi.getState().dialog).toEqual({ kind: "analysis-settings", analysis: "compare", partId: a, otherId: b });
    useUi.setState({ dialog: null });
    s().select([a, b, c]);
    newAnalysis("compare");
    expect(lastToast()).toBe(COMPARE_PARTS);
    s().select([]);
    newAnalysis("compare");
    expect(lastToast()).toBe(COMPARE_PICK);
  });

  it("runs on both parts, stays with the reference, and is out of date once the comparison moves", async () => {
    const a = cube("A");
    const b = cube("B");
    ipc.runAnalysis.mockResolvedValue({ result: { forward: SIDE, backward: SIDE, fit: null }, drawing: { surfaces: [surface("reference"), surface("comparison")] } });
    const id = await runPartAnalysis("compare", a, { comparisonId: b, comparisonName: "", comparisonMatrix: [], direction: "both", align: false });
    const [, settings, sent] = ipc.runAnalysis.mock.calls[0];
    expect(sent.map((p: { name: string }) => p.name)).toEqual(["A", "B"]);
    expect(settings.comparisonName).toBe("B");
    expect(settings.comparisonMatrix).toEqual(translation(100, 100, 10));
    expect(s().parts[0].analyses?.[0].id).toBe(id);
    expect(s().parts[1].analyses).toBeUndefined();
    const found = shownAnalysis();
    expect(found && isOutOfDate(found.part, found.analysis)).toBe(false);
    s().setPartMatrices(new Map([[b, translation(0, 0, 10)]]), "Move");
    const moved = shownAnalysis();
    expect(moved && isOutOfDate(moved.part, moved.analysis)).toBe(true);
    // A part is never compared with itself.
    await runPartAnalysis("compare", a, { comparisonId: a, comparisonName: "", comparisonMatrix: [], direction: "both", align: false });
    expect(lastToast()).toBe("Pick a comparison part other than the reference.");
  });

  it("adds and removes measure points with their distances, one undo step each", async () => {
    const a = cube("A");
    const b = cube("B");
    ipc.runAnalysis.mockResolvedValue({ result: { forward: SIDE, backward: null, fit: null }, drawing: { surfaces: [surface("reference")] } });
    await runPartAnalysis("compare", a, { comparisonId: b, comparisonName: "B", comparisonMatrix: [], direction: "reference-to-comparison", align: false });
    probeAt({ point: [1, 2, 3], value: 0.25 });
    probeAt({ point: [5, 5, 5], value: -0.5 });
    const probes = () => (shownAnalysis()?.analysis as Extract<PartAnalysis, { kind: "compare" }>).probes ?? [];
    expect(probes().map((p) => p.value)).toEqual([0.25, -0.5]);
    probeAt(null, [4, 4, 4]);
    expect(probes().map((p) => p.value)).toEqual([0.25]);
    expect(labels().slice(-3)).toEqual(["Add measure point", "Add measure point", "Remove measure point"]);
    const analysis = shownAnalysis()?.analysis as PartAnalysis;
    expect(resultCells(analysis, "A")).toContainEqual(["Point 1", "1.00", "2.00", "3.00", "0.250"]);
    s().undo();
    expect(probes().length).toBe(2);
  });
});

describe("wall thickness", () => {
  const WALL: WallResult = {
    threshold: 2,
    acceptance: 5,
    passed: false,
    thinPercent: 50,
    thinArea: 1200,
    totalArea: 2400,
    minThickness: 1,
    minFrom: [0, 0, 0],
    minTo: [1, 0, 0],
    complete: true,
    unmeasured: 0,
  };

  it("makes a coloured copy of the part when it closes, if asked", async () => {
    const id = cube();
    const values = Float32Array.from([1, 1, 3, 3, 3, 3, 3, 3, 3, 3, NaN, 3]);
    ipc.runAnalysis.mockResolvedValue({ result: WALL, drawing: { values } });
    const settings: WallSettings = { ...DEFAULT_WALL, threshold: 2, colouredPart: true };
    const done = (await runPartAnalysis("wall", id, settings)) as string;
    deleteAnalysis(id, done);
    expect(s().parts.map((p) => p.name)).toEqual(["Cube", "Cube (wall thickness)"]);
    const colors = s().parts[1].colors as Uint8Array;
    expect(colors.length).toBe(36);
    // Thin triangles red, the rest green, the unmeasured one grey.
    expect([...colors.slice(0, 3)]).toEqual([214, 69, 65]);
    expect([...colors.slice(6, 9)]).toEqual([58, 167, 87]);
    expect([...colors.slice(30, 33)]).toEqual([163, 169, 177]);
    expect(labels().slice(-2)).toEqual(["Delete Wall thickness", "Coloured part from wall thickness"]);
  });

  it("colours by thickness three ways", () => {
    const s2 = { threshold: 2 };
    expect(wallColor(1.9, { ...s2, colors: "red-green" })).not.toEqual(wallColor(2, { ...s2, colors: "red-green" }));
    const mid = wallColor(2, { ...s2, colors: "red-green-gradient" });
    // At the threshold the gradient is yellow.
    expect(mid[0]).toBeGreaterThan(0.9);
    expect(mid[1]).toBeGreaterThan(0.7);
    expect(wallColor(0, { ...s2, colors: "blue-yellow-gradient" })[2]).toBeGreaterThan(0.8);
    expect(wallColor(10, { ...s2, colors: "blue-yellow-gradient" })[2]).toBeLessThan(0.3);
  });
});

describe("compare colours and the bar chart", () => {
  it("are green within the tolerance, red above, blue below, and grey outside the picked bars", () => {
    const view = { tolerance: 0.1, gradient: false, bars: null };
    const [green, red, blue] = [compareColor(0.05, view, 1), compareColor(0.5, view, 1), compareColor(-0.5, view, 1)];
    expect(green[1]).toBeGreaterThan(green[0]);
    expect(red[0]).toBeGreaterThan(red[2]);
    expect(blue[2]).toBeGreaterThan(blue[0]);
    const h = histogram([{ role: "reference", positions: new Float32Array(9), indices: new Uint32Array(3), values: Float32Array.from([-1, 0, 1]) }], 4);
    expect(h).toEqual({ lo: -1, hi: 1, counts: [1, 0, 1, 1] });
    expect(barOf(-1, h.lo, h.hi, 4)).toBe(0);
    expect(barOf(1, h.lo, h.hi, 4)).toBe(3);
    expect(compareColor(0.5, { ...view, bars: [0] }, 1, h)).toEqual(compareColor(0, { tolerance: 0, gradient: false, bars: [3] }, 1, h));
  });
});

describe("a project with Part B results", () => {
  it("keeps wall thicknesses with gaps, compared surfaces and measure points", () => {
    const wall: PartAnalysis = {
      id: "w",
      kind: "wall",
      settings: DEFAULT_WALL,
      result: { threshold: 0.4, acceptance: 5, passed: true, thinPercent: 0, thinArea: 0, totalArea: 10, minThickness: null, minFrom: null, minTo: null, complete: true, unmeasured: 1 },
      matrix: translation(0, 0, 0),
      drawing: { values: Float32Array.from([1, NaN, 2]) },
    };
    const compare: PartAnalysis = {
      id: "c",
      kind: "compare",
      settings: { comparisonId: "p2", comparisonName: "B", comparisonMatrix: translation(1, 0, 0), direction: "both", align: true },
      result: {
        forward: { maxPositive: 1, maxNegative: -1, averageSigned: 0, maxUnsigned: 1, averageUnsigned: 0.5, points: 3 },
        backward: null,
        fit: { rotation: [[1, 0, 0], [0, 1, 0], [0, 0, 1]], translation: [0, 0, 1], angle: 0, rms: 0.01 },
      },
      matrix: translation(0, 0, 0),
      drawing: {
        surfaces: [{ role: "reference", positions: Float32Array.from([0, 0, 0, 1, 0, 0, 0, 1, 0]), indices: Uint32Array.from([0, 1, 2]), values: Float32Array.from([0, 0.5, 1]) }],
      },
      probes: [{ id: "p", point: [0, 0, 0], value: 0.1 }],
    };
    const back = readAnalyses(writeAnalyses([wall, compare]), 3) as PartAnalysis[];
    expect(back[0].drawing.values?.[0]).toBe(1);
    expect(Number.isNaN(back[0].drawing.values?.[1])).toBe(true);
    expect(back[1]).toEqual(compare);
    // A surface whose indices point past its points is left out.
    const raw = JSON.parse(writeAnalyses([compare])) as { drawing: { surfaces: { indices: string }[] } }[];
    raw[0].drawing.surfaces[0].indices = toBase64(new Uint8Array(Uint32Array.from([0, 1, 9]).buffer));
    expect(readAnalyses(JSON.stringify(raw), 3)?.[0].drawing.surfaces).toBeUndefined();
  });
});
