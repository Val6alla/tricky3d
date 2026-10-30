import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Rust is a stand-in that answers the pack the way the test says; everything else runs as in the app.
const ipc = vi.hoisted(() => ({ runArrange: vi.fn() }));
const repairIpc = vi.hoisted(() => ({ cancelRepair: vi.fn() }));
vi.mock("./ipc", () => ipc);
vi.mock("../repair/ipc", async (importOriginal) => ({ ...(await importOriginal<typeof import("../repair/ipc")>()), ...repairIpc }));

import { makeBox, translation } from "../mesh";
import { CANCELLED } from "../repair/ipc";
import { useStore } from "../store";
import { useUi } from "../ui";
import { checkPack, DEFAULT_PACK, loadPack, packTargets, PACK_KEY, runPack, savePack, withoutPreset, withPreset, type PackFields } from "./pack";
import { cancelArrange, useArrange } from "./session";

const s = () => useStore.getState();
const labels = () => s().history.past.map((e) => e.label);
const lastToast = () => useUi.getState().toasts[useUi.getState().toasts.length - 1];

function fakeStorage(entries: Record<string, string> = {}) {
  const data = new Map(Object.entries(entries));
  return { data, getItem: (key: string) => data.get(key) ?? null, setItem: (key: string, value: string) => void data.set(key, value) };
}

/** A, B, C and D; C locked, D hidden; A selected. */
function parts() {
  const ids = s().addParts([
    { name: "A", mesh: makeBox([10, 10, 10]), matrix: translation(10, 10, 5) },
    { name: "B", mesh: makeBox([10, 10, 10]), matrix: translation(40, 20, 5) },
    { name: "C", mesh: makeBox([10, 10, 10]), matrix: translation(80, 80, 5), locked: true },
    { name: "D", mesh: makeBox([10, 10, 10]), matrix: translation(120, 80, 5), visible: false },
  ]);
  s().select([ids[0]]);
  return ids;
}

beforeEach(() => {
  vi.stubGlobal("isTauri", true);
  vi.stubGlobal("window", { setTimeout: () => 0, clearTimeout: () => {} });
  vi.stubGlobal("localStorage", fakeStorage());
  ipc.runArrange.mockReset();
  repairIpc.cancelRepair.mockResolvedValue(undefined);
  s().newProject();
  useArrange.setState({ busy: null });
  useUi.setState({ toasts: [], dialog: null, tasks: [] });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the Pack dialog's fields", () => {
  it("open on the reference's defaults for each method", () => {
    expect(checkPack(DEFAULT_PACK["bounding-box"], 200).settings).toEqual({
      method: "bounding-box",
      distance: 1,
      voxel: 5,
      wall: 0,
      floor: 0,
      ceiling: 0,
      stack: true,
    });
    expect(checkPack(DEFAULT_PACK.scanline2d, 200).settings).toEqual({
      method: "scanline2d",
      performance: 5,
      align: true,
      zSteps: 12,
      wall: 4,
      spacing: 1,
      voxel: 1,
    });
    expect(checkPack(DEFAULT_PACK.scanline3d, 200).settings).toMatchObject({ method: "scanline3d", voxel: 2, zSteps: 12, flip: true, ceiling: 0 });
    expect(checkPack(DEFAULT_PACK["monte-carlo"], 200).settings).toMatchObject({ method: "monte-carlo", distance: 1, rotation: "z", seed: 1 });
  });

  it("turn a height limit into a distance to the ceiling, and say what to fix", () => {
    const mc: PackFields = { ...DEFAULT_PACK["monte-carlo"], limitHeight: true, height: "60" };
    expect(checkPack(mc, 200).settings).toMatchObject({ ceiling: 140 });
    expect(checkPack({ ...mc, height: "300" }, 200).problem).toBe("Enter a height limit above 0 and up to 200 mm.");
    expect(checkPack({ ...mc, seed: "1.5" }, 200).problem).toBe("Enter a whole number of 0 or more for the seed.");
    expect(checkPack({ ...DEFAULT_PACK.scanline2d, voxel: "4" }, 200).problem).toBe("Pick a voxel size from 0.5 to 2.5 mm.");
    expect(checkPack({ ...DEFAULT_PACK["bounding-box"], wall: "-1" }, 200).problem).toBe("Enter a distance to the side walls of 0 mm or more.");
  });

  it("are remembered per method, and a damaged entry falls back to the defaults", () => {
    vi.stubGlobal("localStorage", fakeStorage({ [PACK_KEY]: JSON.stringify({ method: "monte-carlo", fields: { "monte-carlo": { seed: "9", distance: 3 } } }) }));
    const f = loadPack();
    expect(f.method).toBe("monte-carlo");
    expect(f.seed).toBe("9");
    // A number where text belongs is left out.
    expect(f.distance).toBe("1");
    // What Monte Carlo used never leaks into another method, whose voxel range differs.
    savePack({ ...DEFAULT_PACK["monte-carlo"], voxel: "5", wall: "0" });
    expect(loadPack("scanline2d")).toEqual(DEFAULT_PACK.scanline2d);
    savePack({ ...DEFAULT_PACK.scanline2d, voxel: "2" });
    expect(loadPack("scanline2d").voxel).toBe("2");
    expect(loadPack().method).toBe("scanline2d");
    vi.stubGlobal("localStorage", fakeStorage({ [PACK_KEY]: "{oops" }));
    expect(loadPack("scanline2d")).toEqual(DEFAULT_PACK.scanline2d);
  });

  it("keep presets by method and name", () => {
    const f = { ...DEFAULT_PACK.scanline3d, voxel: "3" };
    const one = withPreset({}, "Coarse", f);
    expect(one.scanline3d.Coarse.voxel).toBe("3");
    expect("method" in one.scanline3d.Coarse).toBe(false);
    const two = withPreset(one, "Coarse", { ...f, voxel: "4" });
    expect(two.scanline3d.Coarse.voxel).toBe("4");
    expect(withoutPreset(two, "scanline3d", "Coarse")).toEqual({ scanline3d: {} });
  });
});

describe("Pack", () => {
  it("packs the shown, unlocked parts round the locked and hidden ones", () => {
    const [a, b, c, d] = parts();
    const all = packTargets(s().parts, s().selection, false);
    expect(all.targets.map((p) => p.id)).toEqual([a, b]);
    expect(all.others.map((p) => p.id)).toEqual([c, d]);
    const only = packTargets(s().parts, s().selection, true);
    expect(only.targets.map((p) => p.id)).toEqual([a]);
    expect(only.others.map((p) => p.id)).toEqual([b, c, d]);
  });

  it("moves the parts in one undo step, sends each part's priority and rotation, and names those that do not fit", async () => {
    const [a] = parts();
    s().patchEach(new Map([[a, { packing: { priority: 9, rotation: "none" } }]]), "Packing settings");
    ipc.runArrange.mockResolvedValue({ matrices: [translation(5, 5, 5), translation(300, 0, 5)], fits: [true, false], height: 10, density: 0.0125 });
    const result = await runPack(DEFAULT_PACK["monte-carlo"]);
    const [action, settings, sent] = ipc.runArrange.mock.calls[0];
    expect(action).toBe("pack");
    expect(settings).toEqual({ pack: checkPack(DEFAULT_PACK["monte-carlo"], 200).settings });
    expect(sent.map((p: { name: string; target: boolean; priority: number; rotation: string }) => [p.name, p.target, p.priority, p.rotation])).toEqual([
      ["A", true, 9, "none"],
      ["B", true, 5, "default"],
      ["C", false, 5, "default"],
      ["D", false, 5, "default"],
    ]);
    expect(labels()).toEqual(["Add parts", "Packing settings", "Pack"]);
    expect(s().parts[0].matrix).toEqual(translation(5, 5, 5));
    // The locked part never moves.
    expect(s().parts[2].matrix).toEqual(translation(80, 80, 5));
    expect(result).toEqual({ packed: 1, aside: ["B"], height: 10, density: 0.0125 });
    expect(lastToast()).toEqual({
      id: expect.any(Number),
      kind: "warn",
      message: "Packed 1 part: 10.0 mm high, 1.3 % of the room up to there. 1 part does not fit and is beside the platform: B.",
    });
    s().undo();
    expect(s().parts[0].matrix).toEqual(translation(10, 10, 5));
  });

  it("leaves the project as it was when cancelled or when Rust reports a collision", async () => {
    parts();
    const before = s().parts;
    ipc.runArrange.mockImplementation(async () => {
      cancelArrange();
      throw CANCELLED;
    });
    expect(await runPack(DEFAULT_PACK.scanline3d)).toBeNull();
    expect(s().parts).toBe(before);
    expect(lastToast().message).toBe("Pack was cancelled. Nothing changed.");
    ipc.runArrange.mockRejectedValue('Packing failed: "A" and "B" would collide. Nothing was changed.');
    expect(await runPack(DEFAULT_PACK.scanline3d)).toBeNull();
    expect(s().parts).toBe(before);
    expect(lastToast()).toMatchObject({ kind: "error", message: 'Packing failed: "A" and "B" would collide. Nothing was changed.' });
    expect(labels()).toEqual(["Add parts"]);
  });

  it("says so when there is nothing to pack", async () => {
    parts();
    s().clearSelection();
    expect(await runPack({ ...DEFAULT_PACK.scanline2d, onlySelected: true })).toBeNull();
    expect(lastToast().message).toBe("Select the shown, unlocked parts to pack first.");
    expect(ipc.runArrange).not.toHaveBeenCalled();
  });
});

describe("Packing settings", () => {
  it("are one undo step for several parts, and a lock set there is the part's lock", () => {
    const [a, b] = parts();
    s().patchEach(
      new Map([
        [a, { packing: { priority: 2, rotation: "z" }, locked: true }],
        [b, { locked: true }],
      ]),
      "Packing settings",
    );
    expect(labels()).toEqual(["Add parts", "Packing settings"]);
    expect(s().parts[0]).toMatchObject({ locked: true, packing: { priority: 2, rotation: "z" } });
    expect(s().parts[1].locked).toBe(true);
    expect(s().parts[1].packing).toBeUndefined();
    // Nothing different: no step.
    s().patchEach(new Map([[b, { locked: true }]]), "Packing settings");
    expect(labels()).toEqual(["Add parts", "Packing settings"]);
  });
});
