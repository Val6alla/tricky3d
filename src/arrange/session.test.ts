import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Rust is a stand-in that answers each action the way the test says; everything else runs as in the app.
const ipc = vi.hoisted(() => ({ runArrange: vi.fn() }));
const repairIpc = vi.hoisted(() => ({ cancelRepair: vi.fn() }));
vi.mock("./ipc", () => ipc);
vi.mock("../repair/ipc", async (importOriginal) => ({ ...(await importOriginal<typeof import("../repair/ipc")>()), ...repairIpc }));

import { makeBox, translation } from "../mesh";
import { CANCELLED } from "../repair/ipc";
import { useStore } from "../store";
import { useUi } from "../ui";
import { alignParts, cancelArrange, copyNames, duplicateParts, layFlatAt, layFlatAuto, minBox, openOrient, useArrange } from "./session";
import { checkDuplicate, DEFAULT_DUPLICATE, gridExtent } from "./settings";

const s = () => useStore.getState();
const names = () => s().parts.map((p) => p.name);
const labels = () => s().history.past.map((e) => e.label);
const lastToast = () => useUi.getState().toasts[useUi.getState().toasts.length - 1]?.message;

/** A, B and C, 10 mm cubes; A and B selected. */
function parts() {
  const ids = s().addParts([
    { name: "A", mesh: makeBox([10, 10, 10]), matrix: translation(10, 10, 5) },
    { name: "B", mesh: makeBox([10, 10, 10]), matrix: translation(40, 20, 15) },
    { name: "C", mesh: makeBox([10, 10, 10]), matrix: translation(80, 80, 5) },
  ]);
  s().select([ids[0], ids[1]]);
  return ids;
}

beforeEach(() => {
  vi.stubGlobal("isTauri", true);
  vi.stubGlobal("window", { setTimeout: () => 0, clearTimeout: () => {} });
  ipc.runArrange.mockReset();
  repairIpc.cancelRepair.mockResolvedValue(undefined);
  s().newProject();
  useArrange.setState({ busy: null });
  useUi.setState({ toasts: [], dialog: null, tasks: [], viewTool: null });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("Align", () => {
  it("applies Rust's matrices to the selected parts as one undo step", async () => {
    const [a, b] = parts();
    ipc.runArrange.mockResolvedValue({ matrices: [translation(5, 10, 5), translation(5, 20, 15)] });
    await alignParts("left");
    const [action, settings, sent] = ipc.runArrange.mock.calls[0];
    expect(action).toBe("align");
    expect(settings).toEqual({ align: "left" });
    expect(sent.map((p: { name: string; target: boolean }) => [p.name, p.target])).toEqual([
      ["A", true],
      ["B", true],
    ]);
    expect(s().parts.find((p) => p.id === a)?.matrix).toEqual(translation(5, 10, 5));
    expect(s().parts.find((p) => p.id === b)?.matrix).toEqual(translation(5, 20, 15));
    expect(labels()).toEqual(["Add parts", "Align left"]);
    s().undo();
    expect(s().parts.find((p) => p.id === a)?.matrix).toEqual(translation(10, 10, 5));
  });

  it("needs two parts to line up, one to put on the platform", async () => {
    const [a] = parts();
    s().select([a]);
    await alignParts("centre-x");
    expect(lastToast()).toBe("Select at least 2 parts to align.");
    expect(ipc.runArrange).not.toHaveBeenCalled();
    ipc.runArrange.mockResolvedValue({ matrices: [translation(10, 10, 5)] });
    await alignParts("drop");
    expect(ipc.runArrange).toHaveBeenCalledOnce();
    // Nothing moved: no undo step, and it says so.
    expect(labels()).toEqual(["Add parts"]);
    expect(lastToast()).toBe("The parts are already in place.");
  });

  it("leaves locked parts alone and says why when nothing is left", async () => {
    const [a] = parts();
    s().setPartLocked(a, true);
    s().select([a]);
    await alignParts("drop");
    expect(lastToast()).toBe("Locked parts cannot be moved. Unlock them first.");
  });
});

describe("a cancelled or outdated run", () => {
  it("changes nothing when cancelled, at any moment", async () => {
    parts();
    const before = s().parts;
    ipc.runArrange.mockImplementation(async () => {
      cancelArrange();
      throw CANCELLED;
    });
    await alignParts("left");
    expect(s().parts).toBe(before);
    expect(repairIpc.cancelRepair).toHaveBeenCalledOnce();
    expect(lastToast()).toBe("Align left was cancelled. Nothing changed.");
    // Cancelled after Rust had finished: the answer is dropped.
    ipc.runArrange.mockImplementation(async () => {
      cancelArrange();
      return { matrices: [translation(0, 0, 0), translation(0, 0, 0)] };
    });
    await alignParts("left");
    expect(s().parts).toBe(before);
    expect(labels()).toEqual(["Add parts"]);
    expect(useArrange.getState().busy).toBeNull();
  });

  it("drops the answer when the parts changed while it ran", async () => {
    const [a] = parts();
    ipc.runArrange.mockImplementation(async () => {
      s().renamePart(a, "Renamed");
      return { matrices: [translation(0, 0, 0), translation(0, 0, 0)] };
    });
    await alignParts("left");
    expect(labels()).toEqual(["Add parts", "Rename"]);
    expect(lastToast()).toBe("Align left was stopped because the parts changed while it ran.");
  });

  it("sends each run its own job number, and Cancel that number", async () => {
    parts();
    let job = 0;
    ipc.runArrange.mockImplementation(async (_a: string, _s: unknown, _p: unknown, _pl: unknown, j: number) => {
      job = j;
      cancelArrange();
      throw CANCELLED;
    });
    await layFlatAuto();
    expect(repairIpc.cancelRepair).toHaveBeenCalledWith(job);
  });
});

describe("Duplicate", () => {
  it("names copies (2), (3)… and adds them as one undo step, selected", async () => {
    const [a] = parts();
    s().patchParts([a], { packing: { priority: 9, rotation: "free" } }, "Packing settings");
    ipc.runArrange.mockResolvedValue({
      matrices: [translation(10, 10, 5), translation(40, 20, 15)],
      copies: [
        { source: 0, matrix: translation(10, 10, 5) },
        { source: 1, matrix: translation(40, 20, 15) },
        { source: 0, matrix: translation(10, 10, 5) },
        { source: 1, matrix: translation(40, 20, 15) },
      ],
    });
    const made = await duplicateParts({ mode: "in-place", copies: 2 });
    expect(made).toBe(4);
    expect(names()).toEqual(["A", "B", "C", "A (2)", "B (2)", "A (3)", "B (3)"]);
    expect(labels()).toEqual(["Add parts", "Packing settings", "Duplicate in place"]);
    expect(s().selection).toEqual(s().parts.slice(3).map((p) => p.id));
    expect(lastToast()).toBe("Made 4 copies.");
    // Copies share the mesh and keep the colour and the packing settings.
    expect(s().parts[3].mesh).toBe(s().parts[0].mesh);
    expect(s().parts[3].color).toBe(s().parts[0].color);
    expect(s().parts.map((p) => p.packing?.priority)).toEqual([9, undefined, undefined, 9, undefined, 9, undefined]);
    expect(s().parts[5].packing).toEqual({ priority: 9, rotation: "free" });
    s().undo();
    expect(names()).toEqual(["A", "B", "C"]);
  });

  it("counts on from the highest number already there", () => {
    expect(copyNames(["Box", "Box (2)", "Box (5)", "Pin"], ["Box", "Box (2)", "Pin"])).toEqual(["Box (6)", "Box (7)", "Pin (2)"]);
    expect(copyNames(["Bracket (left)"], ["Bracket (left)"])).toEqual(["Bracket (left) (2)"]);
  });

  it("Fill the platform moves the originals and adds the copies in one undo step, sending the other parts too", async () => {
    const [a] = parts();
    s().select([a]);
    ipc.runArrange.mockResolvedValue({
      matrices: [translation(5, 5, 5)],
      copies: [
        { source: 0, matrix: translation(20, 5, 5) },
        { source: 0, matrix: translation(35, 5, 5) },
      ],
    });
    await duplicateParts({ mode: "fill", spacing: 5, wall: 0, limit: 1000, stack: false });
    const sent = ipc.runArrange.mock.calls[0][2] as { name: string; target: boolean }[];
    expect(sent.map((p) => [p.name, p.target])).toEqual([
      ["A", true],
      ["B", false],
      ["C", false],
    ]);
    expect(s().parts[0].matrix).toEqual(translation(5, 5, 5));
    expect(names()).toEqual(["A", "B", "C", "A (2)", "A (3)"]);
    expect(labels()).toEqual(["Add parts", "Fill the platform"]);
    s().undo();
    expect(names()).toEqual(["A", "B", "C"]);
    expect(s().parts[0].matrix).toEqual(translation(10, 10, 5));
  });
});

describe("Lay flat and the smallest box", () => {
  it("lays a clicked face flat as one undo step, but not on a locked part", async () => {
    const [a] = parts();
    ipc.runArrange.mockResolvedValue({ matrices: [translation(10, 10, 5.5)] });
    await layFlatAt({ partId: a, point: [10, 10, 10], normal: [0, 0, 1] });
    expect(ipc.runArrange.mock.calls[0][1]).toEqual({ point: [10, 10, 10], normal: [0, 0, 1] });
    expect(labels()).toEqual(["Add parts", "Lay flat"]);
    s().setPartLocked(a, true);
    await layFlatAt({ partId: a, point: [10, 10, 10], normal: [0, 0, 1] });
    expect(lastToast()).toBe("Locked parts cannot be turned. Unlock it first.");
    expect(ipc.runArrange).toHaveBeenCalledOnce();
  });

  it("rotates to the smallest box about the axes asked for", async () => {
    parts();
    ipc.runArrange.mockResolvedValue({ matrices: [translation(1, 1, 5), translation(2, 2, 15)] });
    await minBox("all");
    expect(ipc.runArrange.mock.calls[0].slice(0, 2)).toEqual(["min-box", { axes: "all" }]);
    expect(labels()).toEqual(["Add parts", "Rotate to smallest box"]);
  });
});

describe("Orient", () => {
  it("opens for one selected part only", () => {
    const [a] = parts();
    openOrient();
    expect(lastToast()).toBe("Select one part to orient.");
    s().select([a]);
    openOrient();
    expect(useUi.getState().dialog).toEqual({ kind: "orient", partId: a });
  });

  it("a preview applied is one undo step; a preview cancelled leaves no trace", () => {
    const [a] = parts();
    const before = s().snapshot();
    s().setPartMatrices(new Map([[a, translation(1, 2, 3)]]), null);
    s().setPartMatrices(new Map([[a, translation(4, 5, 6)]]), null);
    s().discardPreview(before);
    expect(s().parts).toBe(before.parts);
    expect(labels()).toEqual(["Add parts"]);
    // The next edit records no leftover "Transform" step.
    s().renamePart(a, "X");
    expect(labels()).toEqual(["Add parts", "Rename"]);

    const again = s().snapshot();
    s().setPartMatrices(new Map([[a, translation(4, 5, 6)]]), null);
    s().recordChange("Orient", again);
    expect(labels()).toEqual(["Add parts", "Rename", "Orient"]);
    expect(s().parts[0].matrix).toEqual(translation(4, 5, 6));
  });
});

describe("the Duplicate dialog's fields", () => {
  it("check into the settings Rust takes, or say what to fix", () => {
    expect(checkDuplicate("in-place", DEFAULT_DUPLICATE)).toEqual({ how: { mode: "in-place", copies: 1 }, problem: null });
    expect(checkDuplicate("in-place", { ...DEFAULT_DUPLICATE, copies: "0" }).problem).toBe("Enter 1 to 10000 copies.");
    expect(checkDuplicate("grid", DEFAULT_DUPLICATE).how).toEqual({
      mode: "grid",
      spacing: "boxes",
      axes: [
        { count: 2, gap: 1, negative: false },
        { count: 1, gap: 1, negative: false },
        // Z is off: one place.
        { count: 1, gap: 0, negative: false },
      ],
    });
    const one = { ...DEFAULT_DUPLICATE, axes: DEFAULT_DUPLICATE.axes.map((a) => ({ ...a, count: "1" })) as typeof DEFAULT_DUPLICATE.axes };
    expect(checkDuplicate("grid", one).problem).toBe("Enter a count above 1 for at least one axis.");
    expect(checkDuplicate("fill", DEFAULT_DUPLICATE).how).toEqual({ mode: "fill", spacing: 5, wall: 0, limit: 1000, stack: true });
    expect(checkDuplicate("fill", { ...DEFAULT_DUPLICATE, distance: "-1" }).problem).toBe("Enter a minimum part distance of 0 mm or more.");
  });

  it("work out the grid's size as the dialog shows it", () => {
    const f = { ...DEFAULT_DUPLICATE, axes: [{ on: true, negative: false, count: "3", gap: "2" }, DEFAULT_DUPLICATE.axes[1], DEFAULT_DUPLICATE.axes[2]] as typeof DEFAULT_DUPLICATE.axes };
    expect(gridExtent(f, [10, 20, 5])).toEqual([34, 20, 5]);
    expect(gridExtent({ ...f, spacing: "origins" }, [10, 20, 5])).toEqual([14, 20, 5]);
  });
});
