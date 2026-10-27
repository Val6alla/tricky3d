import { describe, expect, it } from "vitest";
import {
  actionCountText,
  closedMark,
  cm2,
  cm3,
  formatDuration,
  formatNumber,
  formatParam,
  mark,
  orientedMark,
  runSummary,
  runWarnings,
  selectionInfoText,
  statusCount,
  statusRows,
  stepParamsText,
} from "./format";
import type { ActionReport, ActionSpec, ParamSpec, RepairStatus } from "./types";

const tolerance: ParamSpec = { id: "tolerance", label: "Tolerance", kind: "float", unit: "mm", decimals: 3, min: 0, default: 0.01 };
const degenerateTolerance: ParamSpec = { ...tolerance, decimals: 4, default: 0.002 };
const ignore: ParamSpec = { id: "ignore_orientation", label: "Ignore orientation", kind: "bool", default: false };
const mode: ParamSpec = {
  id: "mode",
  label: "Tiny-shell detection",
  kind: "choice",
  default: "closed_volume_open_area",
  choices: [
    { id: "closed_volume_open_area", label: "Delete closed shells below minimum volume and open shells below minimum area" },
    { id: "volume", label: "Delete shells below minimum volume" },
  ],
};

const status: RepairStatus = {
  edges: 1_204_356,
  triangles: 802_904,
  vertices: 401_454,
  shells: 27,
  borderEdges: 12,
  nonManifoldEdges: 0,
  invertedTriangles: 80_437,
  holes: 4,
  degenerateTriangles: 3,
  closed: false,
  oriented: true,
  volume: 12000,
  area: 2400,
};

const report = (over: Partial<ActionReport> = {}): ActionReport => ({
  action: "close_all_holes",
  label: "Close all holes",
  message: "Closed 4 holes, added 212 triangles.",
  warnings: [],
  resolved: [],
  changed: true,
  millis: 120,
  ...over,
});

describe("parameter values", () => {
  it("show the spec's decimals and unit, Yes/No, and choice labels", () => {
    expect(formatParam(tolerance, 0.01)).toBe("0.010 mm");
    expect(formatParam(degenerateTolerance, 0.002)).toBe("0.0020 mm");
    expect(formatParam(tolerance, 1234.5)).toBe("1,234.500 mm");
    expect(formatParam({ ...tolerance, unit: "mm³", decimals: 4 }, 0.0001)).toBe("0.0001 mm³");
    expect(formatParam(tolerance, undefined)).toBe("0.010 mm");
    expect(formatParam(ignore, true)).toBe("Yes");
    expect(formatParam(ignore, false)).toBe("No");
    expect(formatParam(mode, "volume")).toBe("Delete shells below minimum volume");
    expect(formatParam(mode, undefined)).toBe(mode.choices?.[0].label);
    expect(formatParam(mode, "nonsense")).toBe("nonsense");
    expect(formatParam({ ...tolerance, unit: undefined, decimals: undefined }, 2.5)).toBe("2.5");
  });

  it("summarise a step's parameters in one line", () => {
    const stitch: ActionSpec = {
      id: "stitch_triangles",
      label: "Stitch triangles",
      implemented: true,
      inScripts: true,
      needsSelection: false,
      repairType: 1,
      hint: "",
      params: [tolerance, ignore],
    };
    expect(stepParamsText({ type: "stitch_triangles", enabled: true, params: { tolerance: 0.01, ignore_orientation: true } }, stitch)).toBe(
      "Tolerance 0.010 mm, Ignore orientation Yes",
    );
    expect(stepParamsText({ type: "stitch_triangles", enabled: true, params: {} }, stitch)).toBe("Tolerance 0.010 mm, Ignore orientation No");
    expect(stepParamsText({ type: "wrap_part_surface", enabled: true, params: {} }, { ...stitch, params: [] })).toBe("");
  });

  it("format numbers with thousands separators", () => {
    expect(formatNumber(1234567)).toBe("1,234,567");
    expect(formatNumber(0.5, 2)).toBe("0.50");
    expect(formatNumber(1 / 3)).toBe("0.333333");
    expect(formatNumber(-0, 3)).toBe("0.000");
    expect(formatNumber(-0.0001, 3)).toBe("0.000");
    expect(formatNumber(-0.0006, 3)).toBe("-0.001");
    expect(formatNumber(-1234.5, 1)).toBe("-1,234.5");
  });
});

describe("status", () => {
  it("marks closed and oriented, with ? before the status is computed", () => {
    expect([mark(true), mark(false), mark(null), mark(undefined)]).toEqual(["✓", "✗", "?", "?"]);
    expect(closedMark(status)).toBe("✗");
    expect(orientedMark(status)).toBe("✓");
    expect(closedMark(null)).toBe("?");
    expect(orientedMark(null)).toBe("?");
  });

  it("shows counts with thousands separators, or ?", () => {
    expect(statusCount(80437)).toBe("80,437");
    expect(statusCount(0)).toBe("0");
    expect(statusCount(null)).toBe("?");
    expect(statusRows(status)).toEqual([
      { label: "Edges", text: "1,204,356" },
      { label: "Triangles", text: "802,904" },
      { label: "Shells", text: "27" },
      { label: "Border edges", text: "12" },
      { label: "Inv. orientation", text: "80,437" },
      { label: "Holes", text: "4" },
      { label: "Non-manifold edges", text: "0" },
    ]);
    expect(statusRows(null).every((r) => r.text === "?")).toBe(true);
  });

  it("converts shell areas and volumes to cm² and cm³", () => {
    expect(cm2(2400)).toBe("24.000");
    expect(cm3(12000)).toBe("12.000");
    expect(cm3(8_000_000)).toBe("8,000.000");
  });
});

describe("messages", () => {
  it("say what is selected", () => {
    expect(selectionInfoText({ count: 0, area: 0, shells: 0 })).toBe("No triangles selected.");
    expect(selectionInfoText({ count: 1204, area: 1234.5, shells: 2 })).toBe("1,204 triangles selected, area 12.345 cm², in 2 shells");
    expect(selectionInfoText({ count: 1, area: 0.2, shells: 1 })).toBe("1 triangle selected, area 0.002 cm², in 1 shell");
  });

  it("count a script's actions", () => {
    expect(actionCountText(11)).toBe("11 repair actions");
    expect(actionCountText(1)).toBe("1 repair action");
  });

  it("format durations", () => {
    expect(formatDuration(850.4)).toBe("850 ms");
    expect(formatDuration(12_345)).toBe("12.3 s");
    expect(formatDuration(125_000)).toBe("2 min 5 s");
  });

  it("sum up a run: one action's own report, or a script's changed count and time", () => {
    expect(runSummary("Close all holes", [report()])).toBe("Closed 4 holes, added 212 triangles.");
    expect(runSummary("Close all holes", [report({ message: "No holes to close.", changed: false })])).toBe("No holes to close.");
    expect(runSummary("Wrap part surface", [report({ message: "" })])).toBe("Wrap part surface finished.");
    const script = [report({ millis: 3000 }), report({ changed: false, millis: 1200 }), report({ millis: 0 })];
    expect(runSummary("Extended repair", script)).toBe("Extended repair finished in 4.2 s: 2 of 3 actions changed the mesh.");
    expect(runSummary("Simple repair", [report({ changed: false }), report({ changed: false })])).toBe(
      "Simple repair finished in 240 ms: nothing needed repairing.",
    );
    expect(runSummary("Empty", [])).toBe("Empty had no actions to run.");
  });

  it("gather a run's warnings once each", () => {
    const reports = [report({ warnings: ["2 holes could not be closed."] }), report({ warnings: ["2 holes could not be closed.", "A"] })];
    expect(runWarnings(reports, ["B", "A"])).toEqual(["2 holes could not be closed.", "A", "B"]);
    expect(runWarnings([], [])).toEqual([]);
  });
});
