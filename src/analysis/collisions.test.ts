import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Rust is a stand-in that answers the way the test says; everything else runs as in the app.
const ipc = vi.hoisted(() => ({ runAnalysis: vi.fn() }));
const repairIpc = vi.hoisted(() => ({ cancelRepair: vi.fn() }));
vi.mock("./ipc", () => ipc);
vi.mock("../repair/ipc", async (importOriginal) => ({ ...(await importOriginal<typeof import("../repair/ipc")>()), ...repairIpc }));

import { makeBox, translation } from "../mesh";
import { useStore } from "../store";
import { useUi } from "../ui";
import { collisionStatus } from "./CollisionPanel";
import {
  closeCollisions,
  interlockingTest,
  nudgeSelected,
  openCollisions,
  partCollision,
  setCollisionSettings,
  updateCollisions,
  useCollisions,
} from "./collisions";
import { newAnalysis, runPartAnalysis, useAnalysis } from "./session";

const s = () => useStore.getState();
const lastToast = () => useUi.getState().toasts[useUi.getState().toasts.length - 1]?.message;

function three() {
  return s().addParts([
    { name: "A", mesh: makeBox([10, 10, 10]), matrix: translation(10, 10, 5) },
    { name: "B", mesh: makeBox([10, 10, 10]), matrix: translation(15, 10, 5) },
    { name: "C", mesh: makeBox([10, 10, 10]), matrix: translation(80, 80, 5) },
  ]);
}

/** Answers a check: parts 0 and 1 collide. */
const collideFirstTwo = () => Promise.resolve({ result: { pairs: [{ a: 0, b: 1, kind: "collide", distance: 0 }] }, drawing: { triangles: new Float32Array(9) } });

beforeEach(() => {
  vi.stubGlobal("isTauri", true);
  vi.stubGlobal("window", { setTimeout: () => 0, clearTimeout: () => {} });
  ipc.runAnalysis.mockReset();
  repairIpc.cancelRepair.mockResolvedValue(undefined);
  s().newProject();
  closeCollisions();
  setCollisionSettings({ autoUpdate: true, whileDragging: false, display: true, checkHidden: true, minDistance: 0, moveStep: 1 });
  useAnalysis.setState({ busy: null, shown: null });
  useUi.setState({ toasts: [], dialog: null, tasks: [], viewTool: null });
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("collision detection", () => {
  it("checks every part when it opens and marks each one", async () => {
    const [a, b, c] = three();
    ipc.runAnalysis.mockImplementation(collideFirstTwo);
    openCollisions();
    await vi.waitFor(() => expect(useCollisions.getState().result).not.toBeNull());
    const [kind, settings, sent] = ipc.runAnalysis.mock.calls[0];
    expect([kind, settings, sent.map((p: { name: string }) => p.name)]).toEqual(["collisions", { minDistance: 0 }, ["A", "B", "C"]]);
    expect([partCollision(a), partCollision(b), partCollision(c)]).toEqual(["collide", "collide", "clear"]);
    expect(collisionStatus(useCollisions.getState().result?.pairs ?? [])).toBe("1 collision");
    closeCollisions();
    expect(partCollision(a)).toBeNull();
  });

  it("checks again after a move once it settles, and sends the minimum distance", async () => {
    vi.useFakeTimers();
    const [a] = three();
    ipc.runAnalysis.mockImplementation(() => Promise.resolve({ result: { pairs: [] }, drawing: {} }));
    openCollisions();
    await vi.waitFor(() => expect(useCollisions.getState().result).not.toBeNull());
    expect(ipc.runAnalysis).toHaveBeenCalledTimes(1);
    s().setPartMatrices(new Map([[a, translation(40, 40, 5)]]), "Move");
    expect(useCollisions.getState().outdated).toBe(true);
    expect(ipc.runAnalysis).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(400);
    expect(ipc.runAnalysis).toHaveBeenCalledTimes(2);
    await vi.waitFor(() => expect(useCollisions.getState().outdated).toBe(false));
    setCollisionSettings({ minDistance: 1 });
    await vi.waitFor(() => expect(ipc.runAnalysis).toHaveBeenCalledTimes(3));
    expect(ipc.runAnalysis.mock.calls[2][1]).toEqual({ minDistance: 1 });
    // Without Auto-update a move only marks the result out of date.
    setCollisionSettings({ autoUpdate: false });
    s().setPartMatrices(new Map([[a, translation(60, 40, 5)]]), "Move");
    await vi.advanceTimersByTimeAsync(1000);
    expect(ipc.runAnalysis).toHaveBeenCalledTimes(3);
    expect(useCollisions.getState().outdated).toBe(true);
  });

  it("leaves hidden parts out when asked, and says how many pairs are too close", async () => {
    const [, b] = three();
    s().setPartsVisible([b], false);
    ipc.runAnalysis.mockImplementation(() => Promise.resolve({ result: { pairs: [{ a: 0, b: 1, kind: "near", distance: 0.5 }] }, drawing: {} }));
    setCollisionSettings({ checkHidden: false });
    openCollisions();
    await vi.waitFor(() => expect(useCollisions.getState().result).not.toBeNull());
    expect(ipc.runAnalysis.mock.calls[0][2].map((p: { name: string }) => p.name)).toEqual(["A", "C"]);
    expect(partCollision(b)).toBeNull();
    expect(collisionStatus(useCollisions.getState().result?.pairs ?? [])).toBe("1 pair too close");
    await updateCollisions();
    expect(collisionStatus([])).toBe("No collisions");
  });
});

describe("the interlocking test", () => {
  it("tests the selected parts and shows which cannot be lifted out", async () => {
    const [a, b, c] = three();
    s().select([a, c]);
    ipc.runAnalysis.mockResolvedValue({ result: { rounds: [[1]], trapped: [0] }, drawing: {} });
    await interlockingTest();
    expect(ipc.runAnalysis.mock.calls[0][0]).toBe("interlock");
    expect(ipc.runAnalysis.mock.calls[0][2].map((p: { name: string }) => p.name)).toEqual(["A", "C"]);
    expect(useUi.getState().dialog).toEqual({ kind: "interlock-result", rounds: [[c]], trapped: [a] });
    void b;
    // One part is not enough.
    useUi.setState({ dialog: null });
    s().select([a]);
    await interlockingTest();
    expect(lastToast()).toContain("needs two parts or more");
  });
});

describe("Z-removability", () => {
  it("asks for the minimum distance, then runs on the selected part and keeps the result on it", async () => {
    const [a] = three();
    s().select([a]);
    ipc.runAnalysis.mockResolvedValue({
      result: { minDistance: 0.5, removable: false, bodies: 2, trappedBodies: 1, trappedArea: 300 },
      drawing: { classes: new Uint8Array(12) },
    });
    newAnalysis("removability");
    expect(useUi.getState().dialog).toEqual({ kind: "analysis-settings", analysis: "removability", partId: a });
    expect(ipc.runAnalysis).not.toHaveBeenCalled();
    await runPartAnalysis("removability", a, { minDistance: 0.5 });
    expect(ipc.runAnalysis.mock.calls[0][0]).toBe("removability");
    expect(ipc.runAnalysis.mock.calls[0][1]).toEqual({ minDistance: 0.5 });
    const kept = s().parts[0].analyses?.[0];
    expect(kept?.kind).toBe("removability");
    expect(kept?.settings).toEqual({ minDistance: 0.5 });
  });
});

describe("Move by", () => {
  it("moves the selected parts by the step along one axis as one undo step, locked parts staying", () => {
    const [a, b, c] = three();
    s().setPartLocked(c, true);
    s().select([a, b, c]);
    setCollisionSettings({ moveStep: 2.5 });
    const steps = s().history.past.length;
    expect(nudgeSelected(0, 1)).toBe(2);
    expect(s().history.past.length).toBe(steps + 1);
    const at = (id: string) => s().parts.find((p) => p.id === id)!.matrix.slice(12, 15);
    expect(at(a)).toEqual([12.5, 10, 5]);
    expect(at(b)).toEqual([17.5, 10, 5]);
    expect(at(c)).toEqual([80, 80, 5]);
    nudgeSelected(2, -1);
    expect(at(a)).toEqual([12.5, 10, 2.5]);
    s().undo();
    expect(at(a)).toEqual([12.5, 10, 5]);
  });

  it("says what to do when nothing can move", () => {
    const [, , c] = three();
    s().clearSelection();
    expect(nudgeSelected(1, 1)).toBe(0);
    expect(lastToast()).toBe("Select the parts to move first.");
    s().setPartLocked(c, true);
    s().select([c]);
    expect(nudgeSelected(1, 1)).toBe(0);
    expect(lastToast()).toBe("Locked parts do not move.");
  });
});
