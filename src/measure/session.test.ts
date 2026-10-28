import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { MeasureFeature, MeasureOp } from "./types";

// Rust is a stand-in: a pick answers the feature the test queues, a fit the circle through the points, an evaluation
// the distance between the first two point features under the matrix's translation.
const ipc = vi.hoisted(() => ({ measurePick: vi.fn(), measureFit: vi.fn(), measureEvaluate: vi.fn(), forgetMeasureMeshes: vi.fn() }));
vi.mock("./ipc", () => ipc);

import { makeBox, translation } from "../mesh";
import { useStore } from "../store";
import { useUi } from "../ui";
import { readMeasurements } from "./persist";
import {
  allMeasurements,
  closeMeasure,
  deleteMeasurement,
  dropPending,
  measureClick,
  openMeasure,
  pickMeasureTool,
  refreshResults,
  resetMeasurements,
} from "./session";
import { DEFAULT_OPTIONS, useMeasure } from "./store";
import { TOOLS, chosenText, clicksFor, describe as describeTool, nextClickText, toolById } from "./tools";

const s = () => useStore.getState();
const m = () => useMeasure.getState();
const point = (x: number, y = 0, z = 0): MeasureFeature => ({ kind: "point", p: [x, y, z] });

/** Queues what the next picks answer: each feature in the owner's frame and the same in world space. */
function answer(...features: MeasureFeature[]) {
  for (const f of features) ipc.measurePick.mockResolvedValueOnce({ feature: f, world: f, outline: [] });
}

const click = (partId: string) => measureClick({ partId, triangle: 0, point: [0, 0, 0], tolerance: 1 });

beforeEach(() => {
  vi.stubGlobal("window", { setTimeout: () => 0, confirm: () => true });
  vi.stubGlobal("isTauri", true);
  ipc.forgetMeasureMeshes.mockResolvedValue(undefined);
  ipc.measureFit.mockImplementation(async (_kind: string, points: { p: number[] }[]) => ({ kind: "circle", centre: points[0].p, normal: [0, 0, 1], radius: 5 }));
  ipc.measureEvaluate.mockImplementation(async (op: MeasureOp, features: MeasureFeature[], matrix: number[]) => {
    const [a, b] = features as { p: number[] }[];
    const value = a && b && a.p && b.p ? Math.hypot(a.p[0] - b.p[0], a.p[1] - b.p[1], a.p[2] - b.p[2]) : 0;
    return { kind: "distance", value, components: [value, 0, 0], line: null, apex: null, arms: null, normal: null, anchor: [matrix[12], 0, 0], features, note: op === "distance" ? null : op };
  });
  useMeasure.setState({ session: null, results: {}, busy: false, options: DEFAULT_OPTIONS, lastTool: "point-point", lastTools: {} });
  s().newProject();
  useUi.setState({ toasts: [], dialog: null, ribbonTab: "home" });
});

afterEach(() => {
  closeMeasure();
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

describe("the measure tools", () => {
  it("are the reference tool's dropdowns, each with a sensible need list and description", () => {
    expect(TOOLS).toHaveLength(41);
    expect(new Set(TOOLS.map((t) => t.id)).size).toBe(41);
    expect(TOOLS.filter((t) => t.group === "circle").map((t) => t.label)).toEqual([
      "Distance circle to point",
      "Distance circle to circle",
      "Distance detected circle to 3-point circle",
      "Distance circle centre to circle centre",
      "Distance circle to sphere",
      "Radius circle",
    ]);
    const tool = toolById("point-edge")!;
    expect(chosenText(tool)).toBe("Distance: Point, Edge");
    expect(describeTool(tool)).toEqual(["Pick a point.", "Pick a straight edge.", "Measures the shortest distance from the point to the line the edge lies on, extended past its ends."]);
    expect(nextClickText(toolById("sphere-radius")!, 0, 2)).toBe("Click point 3 of 4 on the 4-point sphere (1 of 1).");
    expect(clicksFor("circle3")).toBe(3);
    for (const t of TOOLS) expect([t.id, t.needs.length > 0 && t.needs.length <= 3]).toEqual([t.id, true]);
  });
});

describe("Measure mode", () => {
  it("opens on the Measurement tab and makes a measurement from two clicks, as one undo step on the first part", async () => {
    const [a] = s().addParts([{ name: "A", mesh: makeBox([10, 10, 10]), matrix: translation(30, 0, 0) }]);
    const [b] = s().addParts([{ name: "B", mesh: makeBox([10, 10, 10]) }]);
    openMeasure();
    expect(useUi.getState().ribbonTab).toBe("measure");
    expect(m().session?.tool).toBe("point-point");
    const steps = s().history.past.length;

    answer(point(1), point(4));
    await click(a);
    expect(m().session?.picks).toHaveLength(1);
    expect(m().session?.owner).toBe(a);
    // The second click is on another part: the measurement still belongs to the first.
    await click(b);
    expect(ipc.measurePick.mock.calls[1][0]).toMatchObject({ kind: "point", owner: s().parts[0].matrix, matrix: s().parts[1].matrix, snap: "auto" });
    expect(m().session?.picks).toEqual([]);
    const list = allMeasurements();
    expect(list.map((e) => [e.part.id, e.measurement.tool])).toEqual([[a, "point-point"]]);
    expect(list[0].measurement.features).toEqual([point(1), point(4)]);
    expect(s().history.past).toHaveLength(steps + 1);
    expect(s().history.past[s().history.past.length - 1].label).toBe("Measure");
    expect(m().session?.selected).toBe(list[0].measurement.id);

    await vi.waitFor(() => expect(m().results[list[0].measurement.id]?.result?.value).toBe(3));
    // Moving the part works the result out again under its new matrix.
    s().setPartMatrices(new Map([[a, translation(50, 0, 0)]]), "Move");
    await vi.waitFor(() => expect(m().results[list[0].measurement.id]?.result?.anchor).toEqual([50, 0, 0]));
    // Undo takes the measurement back off the part.
    s().undo();
    s().undo();
    expect(allMeasurements()).toEqual([]);
  });

  it("fits a 3-point circle from three clicks, then takes the next feature", async () => {
    const [a] = s().addParts([{ name: "A", mesh: makeBox([10, 10, 10]) }]);
    openMeasure("circle3-point");
    answer(point(1), point(2), point(3), point(9));
    await click(a);
    await click(a);
    expect(m().session?.points).toHaveLength(2);
    await click(a);
    expect(ipc.measureFit).toHaveBeenCalledTimes(2);
    expect(m().session?.picks.map((p) => p.feature.kind)).toEqual(["circle"]);
    await click(a);
    expect(allMeasurements()[0].measurement.features.map((f) => f.kind)).toEqual(["circle", "point"]);
  });

  it("asks for an edge, a circle, a surface or a wall as the tool needs, and wall thickness never snaps", async () => {
    const [a] = s().addParts([{ name: "A", mesh: makeBox([10, 10, 10]) }]);
    useMeasure.getState().setOptions({ snap: "node" });
    openMeasure("wall");
    answer({ kind: "wall", from: [0, 0, 10], to: [0, 0, 0] });
    await click(a);
    expect(ipc.measurePick.mock.calls[0][0]).toMatchObject({ kind: "wall", snap: "free" });
    pickMeasureTool("edge-surface-angle");
    answer({ kind: "edge", a: [0, 0, 0], b: [1, 0, 0] }, { kind: "surface", point: [0, 0, 10], normal: [0, 0, 1] });
    await click(a);
    await click(a);
    expect(ipc.measurePick.mock.calls.slice(1).map((c) => [c[0].kind, c[0].snap])).toEqual([
      ["edge", "node"],
      ["surface", "node"],
    ]);
    expect(allMeasurements().map((e) => e.measurement.tool)).toEqual(["wall", "edge-surface-angle"]);
  });

  it("drops the clicks so far on Esc or another tool, says why a click found nothing, and deletes and resets", async () => {
    const [a] = s().addParts([{ name: "A", mesh: makeBox([10, 10, 10]) }]);
    openMeasure();
    answer(point(1));
    await click(a);
    expect(dropPending()).toBe(true);
    expect(m().session?.picks).toEqual([]);
    expect(dropPending()).toBe(false);
    ipc.measurePick.mockRejectedValueOnce("No round edge found here.");
    pickMeasureTool("circle-radius");
    await click(a);
    expect(useUi.getState().toasts.map((t) => t.message)).toContain("No round edge found here.");

    pickMeasureTool("point-position");
    answer(point(1), point(2));
    await click(a);
    await click(a);
    const [first, second] = allMeasurements().map((e) => e.measurement.id);
    deleteMeasurement(first);
    expect(allMeasurements().map((e) => e.measurement.id)).toEqual([second]);
    resetMeasurements();
    expect(allMeasurements()).toEqual([]);
    expect(s().history.past[s().history.past.length - 1].label).toBe("Delete measurements");
  });

  it("keeps the measurements with the parts after closing, saves them as JSON and reads back only what fits", async () => {
    const [a] = s().addParts([{ name: "A", mesh: makeBox([10, 10, 10]) }]);
    openMeasure();
    answer(point(1), point(2));
    await click(a);
    await click(a);
    closeMeasure();
    expect(useUi.getState().ribbonTab).toBe("home");
    expect(m().results).toEqual({});
    const saved = JSON.stringify(s().parts[0].measurements);
    expect(readMeasurements(saved)).toEqual(s().parts[0].measurements);
    const junk = JSON.stringify([{ id: "x", tool: "nope", features: [] }, { id: "y", tool: "point-point", features: [point(1)] }, "z"]);
    expect(readMeasurements(junk)).toBeUndefined();
    expect(readMeasurements("not json")).toBeUndefined();
    const nan = saved.replace('"p":[1,', '"p":[1e999,');
    expect(nan).not.toBe(saved);
    expect(readMeasurements(nan)).toBeUndefined();
    // Reopening works every result out again.
    openMeasure();
    await refreshResults();
    expect(Object.keys(m().results)).toHaveLength(1);
  });

  it("is refused while another mode is open, and picks the tool when opened again", async () => {
    s().addParts([{ name: "A", mesh: makeBox([10, 10, 10]) }]);
    const { openPartLibrary, closePartLibrary } = await import("../library/session");
    const library = await import("../library/store");
    library.useLibrary.setState({ catalog: [{ id: "box", label: "Box", params: [] }], session: null });
    await openPartLibrary();
    openMeasure();
    expect(m().session).toBeNull();
    expect(useUi.getState().toasts.map((t) => t.message)).toContain("Close the part library first.");
    closePartLibrary();
    openMeasure();
    expect(m().session).not.toBeNull();
    openMeasure("edge-xy");
    expect(m().session?.tool).toBe("edge-xy");
  });
});
