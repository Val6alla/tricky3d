import Module, { type ManifoldToplevel } from "manifold-3d";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { MeshStats } from "../io/ipc";
import type { RepairCheck } from "../repair/types";

// Boolean operations end to end in node: the real session, store and manifold-3d wasm (through a stand-in for the
// worker client that runs booleanMeshes in place), with the Rust calls (checks, measuring, Extended repair) answered
// here from the meshes themselves.
const ipc = vi.hoisted(() => ({ analyzeMeshes: vi.fn() }));
const manifold = vi.hoisted(() => ({ manifoldBoolean: vi.fn(), manifoldSplit: vi.fn(), cancelManifold: vi.fn() }));
const repairIpc = vi.hoisted(() => ({ checkMeshes: vi.fn(), runRepair: vi.fn(), cancelRepair: vi.fn(() => Promise.resolve()) }));
const scripts = vi.hoisted(() => ({ loadScripts: vi.fn(() => Promise.resolve(true)), scriptById: vi.fn() }));

vi.mock("../io/ipc", async (importOriginal) => ({ ...(await importOriginal<typeof import("../io/ipc")>()), ...ipc }));
vi.mock("../ops/manifoldClient", () => manifold);
vi.mock("../repair/ipc", async (importOriginal) => ({ ...(await importOriginal<typeof import("../repair/ipc")>()), ...repairIpc }));
vi.mock("../repair/scripts", async (importOriginal) => ({ ...(await importOriginal<typeof import("../repair/scripts")>()), ...scripts }));
vi.mock("../viewport/api", () => ({ viewport: { zoomToBounds: vi.fn() }, registerViewport: vi.fn() }));

import { runFeature } from "../features";
import { IDENTITY, makeBox, translation } from "../mesh";
import { bakeMesh } from "../ops/bake";
import { booleanMeshes, type BooleanOp } from "../ops/manifoldOps";
import type { BooleanJob } from "../ops/manifoldClient";
import { CANCELLED } from "../repair/ipc";
import { useStore, type MeshData, type Part } from "../store";
import { useUi } from "../ui";
import {
  CHECK_SUCCESSFUL,
  CLOSE_BOOLEAN_FIRST,
  acceptBoolean,
  booleanName,
  cancelBoolean,
  checkProblem,
  closeBooleanMode,
  degenerateStep,
  emptyMessage,
  inputsChanged,
  movedTo,
  moveSelected,
  openBooleanMode,
  opProblem,
  pruned,
  repairFirst,
  runBoolean,
  selectInList,
} from "./session";
import { DEFAULT_SETTINGS, useBoolean, type BooleanSession } from "./store";

let wasm: ManifoldToplevel;

beforeAll(async () => {
  wasm = await Module();
  wasm.setup();
});

const s = () => useStore.getState();
const b = () => useBoolean.getState();
const current = () => b().session as BooleanSession;
const logText = () => current().log.map((l) => l.text);
const labels = () => s().history.past.map((e) => e.label);
const names = () => s().parts.map((p) => p.name);
const toasts = () => useUi.getState().toasts.map((t) => t.message);
const last = <T>(list: readonly T[]): T | undefined => list[list.length - 1];

/** Signed-tetrahedron volume, as Rust mesh::stats computes it. */
function volume({ positions: p, indices }: MeshData): number {
  let v = 0;
  for (let t = 0; t < indices.length; t += 3) {
    const [a, c, d] = [indices[t] * 3, indices[t + 1] * 3, indices[t + 2] * 3];
    v += p[a] * (p[c + 1] * p[d + 2] - p[c + 2] * p[d + 1]) - p[a + 1] * (p[c] * p[d + 2] - p[c + 2] * p[d]) + p[a + 2] * (p[c] * p[d + 1] - p[c + 1] * p[d]);
  }
  return v / 6;
}

/** Closed (every edge used twice) and oriented (no directed edge used twice), as repair_check reports them. */
function check({ indices }: MeshData): RepairCheck {
  const undirected = new Map<string, number>();
  const directed = new Set<string>();
  let doubled = 0;
  for (let t = 0; t < indices.length; t += 3) {
    for (const [a, c] of [[indices[t], indices[t + 1]], [indices[t + 1], indices[t + 2]], [indices[t + 2], indices[t]]]) {
      const key = a < c ? `${a},${c}` : `${c},${a}`;
      undirected.set(key, (undirected.get(key) ?? 0) + 1);
      if (directed.has(`${a},${c}`)) doubled++;
      directed.add(`${a},${c}`);
    }
  }
  const borderEdges = [...undirected.values()].filter((n) => n === 1).length;
  const nonManifoldEdges = [...undirected.values()].filter((n) => n > 2).length;
  return { closed: borderEdges === 0 && nonManifoldEdges === 0, oriented: doubled === 0, holes: borderEdges ? 1 : 0, borderEdges, nonManifoldEdges, invertedTriangles: 0 };
}

function stats(mesh: MeshData): MeshStats {
  return { triangles: mesh.indices.length / 3, vertices: mesh.positions.length / 3, volume: volume(mesh), area: 0, shells: 1, openEdges: check(mesh).borderEdges, nonManifoldEdges: 0 };
}

/** What the worker does: bake each part and run the real Boolean. */
function runInPlace(job: BooleanJob, onProgress?: (f: number, label: string) => void) {
  const world = (p: { mesh: MeshData; matrix: readonly number[] }) => bakeMesh(p.mesh, p.matrix);
  return Promise.resolve(booleanMeshes(wasm, job.kind, job.main.map(world), job.tool.map(world), job.noiseVolume, onProgress));
}

/** A 20 mm cube with its lowest corner at (x, 0, 0). */
function addCube(name: string, x: number, fields: Partial<Part> = {}): string {
  const [id] = s().addParts([{ name, mesh: makeBox([20, 20, 20]), matrix: translation(x + 10, 10, 10), ...fields }], `Add ${name}`);
  return id;
}

/** The cube without its two +Z triangles: not closed. */
function addOpenBox(name: string): string {
  const cube = makeBox([20, 20, 20]);
  const mesh = { positions: cube.positions, indices: new Uint32Array([...cube.indices.subarray(0, 6), ...cube.indices.subarray(12)]) };
  const [id] = s().addParts([{ name, mesh, matrix: translation(20, 10, 10) }], `Add ${name}`);
  return id;
}

/** Opens the mode on A (x 0..20) and B (x 10..30), with B moved to the red list. */
function openWithTwoCubes(): { a: string; b: string } {
  const a = addCube("A", 0);
  const bId = addCube("B", 10);
  s().select([a, bId]);
  openBooleanMode();
  selectInList(bId);
  moveSelected("tool");
  return { a, b: bId };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

beforeEach(() => {
  vi.stubGlobal("window", { setTimeout: () => 0, confirm: () => true });
  vi.stubGlobal("localStorage", undefined);
  // @tauri-apps/api's isTauri() reads this flag.
  vi.stubGlobal("isTauri", true);
  for (const f of [...Object.values(ipc), ...Object.values(manifold), ...Object.values(repairIpc)]) f.mockReset();
  ipc.analyzeMeshes.mockImplementation(async (meshes: MeshData[]) => meshes.map((m) => ({ stats: stats(m), edges: new Float32Array(0) })));
  manifold.manifoldBoolean.mockImplementation(runInPlace);
  repairIpc.checkMeshes.mockImplementation(async (meshes: MeshData[]) => meshes.map(check));
  repairIpc.cancelRepair.mockResolvedValue(undefined);
  useBoolean.setState({ session: null, busy: null, tool: "select", settings: DEFAULT_SETTINGS });
  s().newProject();
  useUi.setState({ toasts: [], tasks: [], dialog: null, ribbonTab: "modify" });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

// ---------------------------------------------------------------------------------------------

describe("words and lists", () => {
  it("say what an operation needs in its lists", () => {
    expect(opProblem("union", 1, 1)).toBeNull();
    expect(opProblem("union", 1, 0)).toBe("Union needs at least 2 parts in the lists.");
    expect(opProblem("difference", 2, 0)).toBe("Difference needs at least one part in the red list. Move a part there with the arrow button.");
    expect(opProblem("intersection", 0, 2)).toBe("Intersection needs at least one part in the green list.");
    expect(opProblem("difference", 1, 3)).toBeNull();
  });

  it("name the result after the operation and its parts, main list first", () => {
    expect(booleanName("union", ["A", "B"])).toBe("Union of A, B");
    expect(booleanName("difference", ["Plate", "Pin"])).toBe("Difference of Plate, Pin");
    expect(booleanName("intersection", ["A", "B", "C"])).toBe("Intersection of 3 parts");
  });

  it("name what is wrong with a part that fails the check", () => {
    const open: RepairCheck = { closed: false, oriented: true, holes: 2, borderEdges: 8, nonManifoldEdges: 0, invertedTriangles: 0 };
    expect(checkProblem("Bracket", open)).toBe("Bracket is not closed: 2 holes.");
    const flipped: RepairCheck = { closed: true, oriented: false, holes: 0, borderEdges: 0, nonManifoldEdges: 0, invertedTriangles: 0 };
    expect(checkProblem("Bracket", flipped)).toBe("Bracket is not oriented.");
    expect(emptyMessage("intersection")).toBe("The result is empty: the green and red parts do not overlap.");
  });

  it("move the selected part between the lists, dropping a result, and nothing else", () => {
    const base: BooleanSession = { main: ["a", "b"], tool: ["c"], selected: "b", log: [], result: null };
    expect(movedTo(base, "tool")).toMatchObject({ main: ["a"], tool: ["c", "b"] });
    expect(movedTo(base, "main")).toBe(base);
    expect(movedTo({ ...base, selected: null }, "tool")).toEqual({ ...base, selected: null });
    expect(movedTo({ ...base, selected: "c" }, "main")).toMatchObject({ main: ["a", "b", "c"], tool: [] });
  });

  it("take parts that are gone out of the lists, and see when inputs changed", () => {
    const a: Part = { id: "a", name: "a", mesh: makeBox([1, 1, 1]), matrix: [...IDENTITY], visible: true, transparent: false, locked: false, color: "#fff" };
    const base: BooleanSession = { main: ["a", "x"], tool: ["y"], selected: "y", log: [], result: null };
    expect(pruned(base, [a])).toMatchObject({ main: ["a"], tool: [], selected: null });
    const kept = { ...base, main: ["a"], tool: [] };
    expect(pruned(kept, [a])).toBe(kept);
    const snap = [{ id: "a", mesh: a.mesh, matrix: a.matrix }];
    expect(inputsChanged(snap, [a])).toBe(false);
    expect(inputsChanged(snap, [{ ...a, name: "renamed" }])).toBe(false);
    expect(inputsChanged(snap, [{ ...a, matrix: translation(1, 0, 0) }])).toBe(true);
    expect(inputsChanged(snap, [])).toBe(true);
  });
});

describe("entering and leaving", () => {
  it("opens with the selected parts in the green list and the Boolean operations tab, and closes back", async () => {
    const a = addCube("A", 0);
    const bId = addCube("B", 10);
    const locked = addCube("Locked", 40, { locked: true });
    s().select([a, bId, locked]);
    openBooleanMode();
    expect(current()).toMatchObject({ main: [a, bId], tool: [], selected: null, result: null });
    expect(useUi.getState().ribbonTab).toBe("boolean");
    expect(toasts()).toEqual(["1 locked part was not added to the lists."]);
    expect(await closeBooleanMode()).toBe(true);
    expect(b().session).toBeNull();
    expect(useUi.getState().ribbonTab).toBe("modify");
  });

  it("needs two usable parts", () => {
    s().select([addCube("A", 0)]);
    openBooleanMode();
    expect(b().session).toBeNull();
    expect(toasts()).toEqual(["Select at least 2 parts to combine."]);
  });

  it("blocks commands that would change the parts, and the main Undo, while open", () => {
    openWithTwoCubes();
    const before = s().parts;
    for (const id of ["modify.cut", "modify.split-shells", "context.delete", "modify.undo", "home.repair"]) {
      useUi.setState({ toasts: [] });
      runFeature(id);
      expect([id, toasts()]).toEqual([id, [CLOSE_BOOLEAN_FIRST]]);
    }
    expect(s().parts).toBe(before);
  });
});

describe("operations", () => {
  it.each<[BooleanOp, number]>([
    ["union", 12000],
    ["difference", 4000],
    ["intersection", 4000],
  ])("%s of two overlapping 20 mm cubes shows a %d mm³ result, and nothing changes until OK", async (op, mm3) => {
    openWithTwoCubes();
    const before = s().parts;
    await runBoolean(op);
    const result = current().result;
    expect(result?.op).toBe(op);
    expect(volume(result!.measured.mesh)).toBeCloseTo(mm3, 6);
    expect(result!.part.mesh).toBe(result!.measured.mesh);
    expect(logText()).toContain(CHECK_SUCCESSFUL);
    expect(last(logText())).toMatch(new RegExp(`ready: ${(mm3 / 1000).toFixed(3)} cm³`));
    expect(s().parts).toBe(before);
    expect(b().busy).toBeNull();
  });

  it("OK adds the result as one undo step named after the operation and removes the originals", async () => {
    openWithTwoCubes();
    const before = s().parts;
    const steps = labels().length;
    await runBoolean("difference");
    acceptBoolean();
    expect(names()).toEqual(["Difference of A, B"]);
    expect(volume(bakeMesh(s().parts[0].mesh, s().parts[0].matrix))).toBeCloseTo(4000, 6);
    expect(labels()).toHaveLength(steps + 1);
    expect(last(labels())).toBe("Difference");
    expect(s().selection).toEqual([s().parts[0].id]);
    expect(b().session).toBeNull();
    s().undo();
    expect(s().parts).toBe(before);
  });

  it("keeps the originals when Remove original parts is off", async () => {
    openWithTwoCubes();
    useBoolean.getState().setSettings({ removeOriginals: false });
    await runBoolean("union");
    acceptBoolean();
    expect(names()).toEqual(["A", "B", "Union of A, B"]);
  });

  it("subtracts several red parts at once", async () => {
    const plate = s().addParts([{ name: "Plate", mesh: makeBox([60, 20, 10]), matrix: translation(30, 10, 5) }])[0];
    const pins = [10, 40].map((x, i) => s().addParts([{ name: `Pin ${i + 1}`, mesh: makeBox([4, 30, 4]), matrix: translation(x, 10, 5) }])[0]);
    s().select([plate, ...pins]);
    openBooleanMode();
    for (const pin of pins) {
      selectInList(pin);
      moveSelected("tool");
    }
    await runBoolean("difference");
    // Two square holes, 4 x 4 mm through the 20 mm depth, but only where they cross the 10 mm thick plate.
    expect(volume(current().result!.measured.mesh)).toBeCloseTo(60 * 20 * 10 - 2 * 4 * 20 * 4, 4);
    acceptBoolean();
    expect(names()).toEqual(["Difference of 3 parts"]);
  });

  it("checks every input first: an open part is named with Repair first, and nothing runs", async () => {
    const a = addCube("A", 0);
    const open = addOpenBox("Open box");
    s().select([a, open]);
    openBooleanMode();
    selectInList(open);
    moveSelected("tool");
    const before = s().parts;
    await runBoolean("difference");
    const line = current().log.find((l) => l.kind === "error");
    expect(line).toMatchObject({ text: "Open box is not closed: 1 hole.", repair: open });
    expect(logText()).not.toContain(CHECK_SUCCESSFUL);
    expect(manifold.manifoldBoolean).not.toHaveBeenCalled();
    expect(current().result).toBeNull();
    expect(s().parts).toBe(before);
  });

  it("Repair first runs Extended repair, puts the repaired part in its place in the list, and the operation then runs", async () => {
    const a = addCube("A", 0);
    const open = addOpenBox("Open box");
    s().select([a, open]);
    openBooleanMode();
    selectInList(open);
    moveSelected("tool");
    await runBoolean("difference");
    const extended = { id: "builtin.extended", name: "Extended repair", actions: [{ type: "close_all_holes", enabled: true, params: {} }] };
    scripts.scriptById.mockReturnValue(extended);
    // Extended repair closes the box: a cube at x 10..30, world space.
    const closed = bakeMesh(makeBox([20, 20, 20]), translation(20, 10, 10));
    repairIpc.runRepair.mockResolvedValue({
      changed: true,
      mesh: closed,
      stats: stats(closed),
      edges: new Float32Array(0),
      selection: new Uint32Array(0),
      reports: [],
      status: { closed: true, oriented: true, holes: 0, borderEdges: 0, nonManifoldEdges: 0, invertedTriangles: 0 },
      highlights: null,
      intersecting: null,
      warnings: [],
    });
    const steps = labels().length;
    await repairFirst(open);
    const [steps0] = repairIpc.runRepair.mock.calls[0].slice(1);
    expect(steps0).toBe(extended.actions);
    expect(labels()).toHaveLength(steps + 1);
    expect(last(labels())).toBe("Repair");
    const repaired = s().parts.find((p) => p.name === "Open box")!;
    expect(repaired.mesh).toBe(closed);
    expect(current().tool).toEqual([repaired.id]);
    expect(current().selected).toBe(repaired.id);
    expect(last(logText())).toBe("Open box is closed and oriented now.");
    await runBoolean("difference");
    expect(volume(current().result!.measured.mesh)).toBeCloseTo(4000, 6);
  });

  it("Cancel stops a running operation: nothing is shown or changed", async () => {
    openWithTwoCubes();
    const before = s().parts;
    const held = deferred<never>();
    manifold.manifoldBoolean.mockReturnValue(held.promise);
    manifold.cancelManifold.mockImplementation(() => held.reject(CANCELLED));
    const run = runBoolean("difference");
    await vi.waitFor(() => expect(manifold.manifoldBoolean).toHaveBeenCalled());
    expect(b().busy?.label).toBe("Difference");
    cancelBoolean();
    expect(b().busy?.cancelling).toBe(true);
    await run;
    expect(manifold.cancelManifold).toHaveBeenCalled();
    expect(last(logText())).toBe("Difference was cancelled. Nothing changed.");
    expect(current().result).toBeNull();
    expect(b().busy).toBeNull();
    expect(s().parts).toBe(before);
    // The next run goes ahead.
    manifold.manifoldBoolean.mockImplementation(runInPlace);
    await runBoolean("difference");
    expect(current().result).not.toBeNull();
  });

  it("Cancel without a run discards the result shown, and OK then says what to do", async () => {
    openWithTwoCubes();
    await runBoolean("union");
    cancelBoolean();
    expect(current().result).toBeNull();
    expect(last(logText())).toBe("Result discarded.");
    acceptBoolean();
    expect(last(toasts())).toBe("Run Union, Difference or Intersection first.");
    expect(b().session).not.toBeNull();
  });

  it("refuses an operation the lists cannot do, in the log", async () => {
    const a = addCube("A", 0);
    s().select([a, addCube("B", 10)]);
    openBooleanMode();
    await runBoolean("difference");
    expect(last(logText())).toBe("Difference needs at least one part in the red list. Move a part there with the arrow button.");
    expect(manifold.manifoldBoolean).not.toHaveBeenCalled();
  });

  it("drops the result when an input moves, and when the lists change", async () => {
    const { a } = openWithTwoCubes();
    await runBoolean("union");
    s().setPartMatrices(new Map([[a, translation(-50, 10, 10)]]), "Move");
    expect(current().result).toBeNull();
    expect(last(logText())).toBe("The parts changed, so the result was dropped. Run the operation again.");
    await runBoolean("union");
    selectInList(a);
    moveSelected("tool");
    expect(current().result).toBeNull();
  });

  it("passes the noise filter to the worker in mm³, and removes degenerate faces in Rust when asked", async () => {
    openWithTwoCubes();
    useBoolean.getState().setSettings({ noiseVolume: 0.0002, removeDegenerate: true, degenerateTolerance: 0.003 });
    repairIpc.runRepair.mockImplementation(async (mesh: MeshData) => ({ changed: false, mesh, stats: null, edges: null, selection: new Uint32Array(0), reports: [{ message: "No degenerate faces." }], status: null, highlights: null, intersecting: null, warnings: [] }));
    await runBoolean("difference");
    expect(manifold.manifoldBoolean.mock.calls[0][0].noiseVolume).toBeCloseTo(0.2, 9);
    expect(repairIpc.runRepair.mock.calls[0][1]).toEqual([degenerateStep(0.003)]);
    expect(logText()).toContain("No degenerate faces.");
    expect(current().result).not.toBeNull();
    useBoolean.getState().setSettings({ filterNoise: false });
    await runBoolean("difference");
    expect(manifold.manifoldBoolean.mock.calls[1][0].noiseVolume).toBe(0);
  });

  it("says when the result is empty and shows none", async () => {
    const a = addCube("A", 0);
    const far = addCube("Far", 50);
    s().select([a, far]);
    openBooleanMode();
    selectInList(far);
    moveSelected("tool");
    await runBoolean("intersection");
    expect(last(logText())).toBe(emptyMessage("intersection"));
    expect(current().result).toBeNull();
  });
});
