import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AnalyzedMesh, CutPiece, MeshStats } from "../io/ipc";

// The geometry runs in Rust and in the manifold worker; here both are stand-ins, so the tests check what this module
// decides and how it commits: targets, names, colours, one undo step, staleness, one operation at a time.
const ipc = vi.hoisted(() => ({
  splitShells: vi.fn(),
  mergeMeshes: vi.fn(),
  cutMeshes: vi.fn(),
  analyzeMeshes: vi.fn(),
  fetchStats: vi.fn(),
}));
const manifold = vi.hoisted(() => ({ manifoldBoolean: vi.fn(), manifoldSplit: vi.fn(), cancelManifold: vi.fn() }));
// Repair first opens the real repair mode; its calls to Rust never answer, except the status run, which is cancelled.
const repairIpc = vi.hoisted(() => ({
  runRepair: vi.fn(() => Promise.reject("cancelled")),
  fetchCatalog: vi.fn(() => new Promise(() => {})),
  loadStoredScripts: vi.fn(() => new Promise(() => {})),
}));

vi.mock("../io/ipc", async (importOriginal) => ({ ...(await importOriginal<typeof import("../io/ipc")>()), ...ipc }));
vi.mock("./manifoldClient", () => manifold);
vi.mock("../repair/ipc", async (importOriginal) => ({ ...(await importOriginal<typeof import("../repair/ipc")>()), ...repairIpc }));

import { useClip } from "../clip";
import { registryProblems, runFeature } from "../features";
import { IDENTITY, makeBox, translation } from "../mesh";
import { cachedStats } from "../meshCache";
import { closeRepair } from "../repair/session";
import { useRepair } from "../repair/store";
import { PART_COLORS, useStore, type MeshData, type Part } from "../store";
import { useUi } from "../ui";
import {
  BUSY_DELAY_MS,
  CUT_WORDS,
  MANY_PARTS,
  MERGE_WORDS,
  OPEN_CUT_WARNING,
  SPLIT_WORDS,
  WAIT_MESSAGE,
  badIndices,
  crossedSides,
  cutName,
  cutNeedsWarning,
  cutTargets,
  freshColors,
  isStale,
  keptSingleMessage,
  leftOutMessage,
  mergeName,
  mergePartsCommand,
  noCutMessage,
  openCutDialog,
  madeOf,
  planCut,
  planSplit,
  runCut,
  scopeTargets,
  shellName,
  singleShellMessage,
  splitShellsCommand,
  meshOperationRunning,
  staleMessage,
  targetProblem,
  unionShellMessage,
  unitCutPlane,
} from "./meshOps";
import type { CutPlane } from "./types";

const s = () => useStore.getState();
const toasts = () => useUi.getState().toasts.map((t) => t.message);
const toastKinds = () => useUi.getState().toasts.map((t) => t.kind);
const labels = () => s().history.past.map((e) => e.label);
const lastLabel = () => labels()[labels().length - 1];
const lastToast = () => toasts()[toasts().length - 1];
const names = () => s().parts.map((p) => p.name);
const byId = (id: string) => s().parts.find((p) => p.id === id) as Part;

function stats(over: Partial<MeshStats> = {}): MeshStats {
  return { triangles: 12, vertices: 8, volume: 8000, area: 2400, shells: 1, openEdges: 0, nonManifoldEdges: 0, ...over };
}

/** A measured result mesh as Rust returns it; each call is a new mesh object. */
function made(over: Partial<MeshStats> = {}): AnalyzedMesh {
  return { mesh: makeBox([10, 10, 10]), stats: stats(over), edges: new Float32Array(6) };
}

function piece(side: "below" | "above"): CutPiece {
  return { ...made(), side };
}

/** Adds one part (selected afterwards like any added part) and returns its id. */
function addPart(name: string, fields: { color?: string; locked?: boolean; visible?: boolean; matrix?: number[] } = {}): string {
  const [id] = s().addParts([{ name, mesh: makeBox([20, 20, 20]), ...fields }], `Add ${name}`);
  return id;
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

function part(id: string, fields: Partial<Part> = {}): Part {
  return { id, name: id, mesh: makeBox([1, 1, 1]), matrix: [...IDENTITY], visible: true, transparent: false, locked: false, color: PART_COLORS[0], ...fields };
}

const Z10: CutPlane = { normal: [0, 0, 1], offset: 10 };

// ---------------------------------------------------------------------------------------------

describe("targets and their messages", () => {
  const parts = [part("a"), part("b", { locked: true }), part("c", { visible: false }), part("d")];

  it("takes the selected shown, unlocked parts, or every shown, unlocked part for Cut all parts", () => {
    const sel = scopeTargets({ parts, selection: ["d", "b", "c"] }, "selected");
    expect(sel.parts.map((p) => p.id)).toEqual(["d"]);
    expect([sel.locked, sel.hidden]).toEqual([1, 1]);
    const all = scopeTargets({ parts, selection: [] }, "all");
    expect(all.parts.map((p) => p.id)).toEqual(["a", "d"]);
    expect([all.locked, all.hidden]).toEqual([1, 1]);
  });

  it("explain an empty or too small target list in the transform commands' words", () => {
    const none = { parts: [], locked: 0, hidden: 0 };
    expect(targetProblem(none, SPLIT_WORDS, "selected")).toEqual({ message: "Select the parts to split first.", kind: "info" });
    expect(targetProblem(none, CUT_WORDS, "all")?.message).toBe("There are no parts to cut.");
    expect(targetProblem({ parts: [], locked: 2, hidden: 0 }, CUT_WORDS, "selected")).toEqual({
      message: "Locked parts cannot be cut. Unlock them first.",
      kind: "warn",
    });
    expect(targetProblem({ parts: [], locked: 0, hidden: 1 }, MERGE_WORDS, "selected", 2)?.message).toBe("Hidden parts cannot be merged. Show them first.");
    expect(targetProblem({ parts: [], locked: 1, hidden: 1 }, SPLIT_WORDS, "selected")?.message).toBe("Hidden or locked parts cannot be split.");
    expect(targetProblem({ parts: [parts[0]], locked: 0, hidden: 0 }, MERGE_WORDS, "selected", 2)?.message).toBe("Select at least 2 parts to merge.");
    expect(targetProblem({ parts: [parts[0]], locked: 1, hidden: 0 }, MERGE_WORDS, "selected", 2)?.message).toBe(
      "Select at least 2 parts to merge. Locked parts cannot be merged. Unlock them first.",
    );
    expect(targetProblem({ parts: [parts[0], parts[3]], locked: 1, hidden: 0 }, MERGE_WORDS, "selected", 2)).toBeNull();
  });

  it("name the parts left out", () => {
    expect(leftOutMessage({ hidden: 1, locked: 0 }, "cut")).toBe("1 hidden part was not cut.");
    expect(leftOutMessage({ hidden: 1, locked: 2 }, "united")).toBe("1 hidden part and 2 locked parts were not united.");
    expect(leftOutMessage({ hidden: 0, locked: 0 }, "cut")).toBeNull();
  });

  it("say what happened in the designed words", () => {
    expect(singleShellMessage(1)).toBe("Part has only one shell.");
    expect(singleShellMessage(3)).toBe("Each selected part has only one shell.");
    expect(keptSingleMessage(1)).toBe("1 part has only one shell and was left as it is.");
    expect(keptSingleMessage(2)).toBe("2 parts have only one shell and were left as they are.");
    expect(noCutMessage("selected")).toBe("The plane does not cut any of the selected parts.");
    expect(noCutMessage("all")).toBe("The plane does not cut any of the parts.");
    expect(staleMessage("Cut")).toBe("Cut was stopped because the parts changed while it ran.");
    expect(OPEN_CUT_WARNING).toBe("Part was not closed. Some cut surfaces could not be filled.");
  });

  it("report several shells after a union as not touching, or partly touching", () => {
    expect(unionShellMessage(1, [1, 1])).toBeNull();
    expect(unionShellMessage(2, [1, 1])).toBe("Parts don't touch. The result has 2 separate shells.");
    expect(unionShellMessage(2, [1, 1, 1])).toBe("Some parts don't touch. The result has 2 separate shells.");
    // A part that is two separate solids on its own, touching the other part: every part touches, so only the count.
    expect(unionShellMessage(2, [2, 1])).toBe("The result has 2 separate shells.");
    expect(unionShellMessage(3, [2, 1])).toBe("Parts don't touch. The result has 3 separate shells.");
  });
});

describe("names, colours and staleness", () => {
  it("name the results after their source", () => {
    expect(shellName("Gear", 2)).toBe("Gear (shell 2)");
    expect(cutName("Gear", "below")).toBe("Gear (cut 1)");
    expect(cutName("Gear", "above")).toBe("Gear (cut 2)");
    expect(mergeName(3)).toBe("Merge of 3 parts");
  });

  it("give results palette colours the inputs do not use, cycling through the rest", () => {
    const used = [PART_COLORS[0], PART_COLORS[2].toUpperCase()];
    const out = freshColors(used, PART_COLORS.length);
    const free = PART_COLORS.filter((_, i) => i !== 0 && i !== 2);
    expect(out.slice(0, free.length)).toEqual(free);
    expect(out[free.length]).toBe(free[0]);
    expect(out.some((c) => c === PART_COLORS[0] || c === PART_COLORS[2])).toBe(false);
    // Every colour taken: the palette as it is.
    expect(freshColors(PART_COLORS, 2)).toEqual([PART_COLORS[0], PART_COLORS[1]]);
  });

  it("count results as stale when an input Part was replaced or the project changed", () => {
    const a = part("a");
    const b = part("b");
    expect(isStale([a], 3, { parts: [a, b], generation: 3 })).toBe(false);
    expect(isStale([a], 3, { parts: [{ ...a }, b], generation: 3 })).toBe(true);
    expect(isStale([a], 3, { parts: [b], generation: 3 })).toBe(true);
    expect(isStale([a], 3, { parts: [a, b], generation: 4 })).toBe(true);
  });

  it("make the cut normal unit length, scaling the offset with it", () => {
    expect(unitCutPlane({ normal: [0, 0, 2], offset: 20 })).toEqual({ normal: [0, 0, 1], offset: 10 });
    expect(unitCutPlane({ normal: [0, 0, 0], offset: 1 })).toBeNull();
    expect(unitCutPlane({ normal: [0, 0, 1], offset: NaN })).toBeNull();
  });
});

describe("result plans", () => {
  it("split: replace parts with two or more shells, in shell order, and count the others", () => {
    const [a, b] = [part("a"), part("b")];
    const shells = [made(), made()];
    const plan = planSplit([a, b], [{ shells }, { shells: [] }]);
    expect(plan.groups.map((g) => g.removeIds)).toEqual([["a"]]);
    expect(madeOf(plan).map((m) => m.name)).toEqual(["a (shell 1)", "a (shell 2)"]);
    expect(madeOf(plan)[1].result).toBe(shells[1]);
    expect(plan.single).toBe(1);
  });

  it("cut: replace crossed parts by cut 1 then cut 2, and leave the others alone", () => {
    const [a, b] = [part("a"), part("b")];
    const below = made();
    const above = made();
    expect(crossedSides(below, null)).toBeNull();
    expect(crossedSides(undefined, above)).toBeNull();
    const sides = [null, crossedSides(below, above)];
    const plan = planCut([a, b], sides);
    expect(plan.groups.map((g) => g.removeIds)).toEqual([["b"]]);
    expect(madeOf(plan).map((m) => [m.name, m.result])).toEqual([
      ["b (cut 1)", below],
      ["b (cut 2)", above],
    ]);
    // The warning needs capping and a crossed part that went to Rust.
    expect(cutNeedsWarning(true, [true, false], sides)).toBe(false);
    expect(cutNeedsWarning(true, [false, true], sides)).toBe(true);
    expect(cutNeedsWarning(false, [true, true], sides)).toBe(false);
  });

  it("keep only the worker's invalid indices that name an input, in order", () => {
    expect(badIndices([3, 1, 1, -1, 0, 7], 4)).toEqual([0, 1, 3]);
  });
});

// ---------------------------------------------------------------------------------------------

describe("operations", () => {
  beforeEach(() => {
    vi.stubGlobal("window", { setTimeout: () => 0 });
    vi.stubGlobal("localStorage", undefined);
    // @tauri-apps/api's isTauri() reads this flag.
    vi.stubGlobal("isTauri", true);
    for (const f of [...Object.values(ipc), ...Object.values(manifold)]) f.mockReset();
    s().newProject();
    useUi.setState({ toasts: [], tasks: [], dialog: null });
    useClip.getState().setCutScope("selected");
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("refuse in a plain browser", async () => {
    vi.stubGlobal("isTauri", false);
    addPart("a");
    await splitShellsCommand();
    expect(toasts()).toEqual(["Splitting shells needs the tricky3D desktop app."]);
    expect(ipc.splitShells).not.toHaveBeenCalled();
  });

  it("split shells: replace the part by its shells as one undo step, primed, in new colours, and undo exactly", async () => {
    const a = addPart("a", { color: PART_COLORS[0] });
    const b = addPart("b", { color: PART_COLORS[1] });
    addPart("c");
    s().select([a, b]);
    const shells = [made({ volume: 8000 }), made({ volume: 1000 })];
    ipc.splitShells.mockResolvedValue([
      { count: 2, shells },
      { count: 1, shells: [] },
    ]);
    const before = s().parts;
    const steps = labels().length;

    await splitShellsCommand();

    const sent = ipc.splitShells.mock.calls[0][0];
    expect(sent.map((p: { name: string }) => p.name)).toEqual(["a", "b"]);
    expect(sent[0].mesh).toBe(byIdIn(before, a).mesh);
    expect(sent[0].matrix).toEqual(byIdIn(before, a).matrix);
    expect(names()).toEqual(["a (shell 1)", "a (shell 2)", "b", "c"]);
    expect(labels()).toHaveLength(steps + 1);
    expect(lastLabel()).toBe("Split shells");
    const added = s().parts.slice(0, 2);
    expect(s().selection).toEqual(added.map((p) => p.id));
    expect(added.map((p) => p.mesh)).toEqual(shells.map((r) => r.mesh));
    expect(added.every((p) => p.matrix.every((v, i) => v === IDENTITY[i]))).toBe(true);
    expect(added.map((p) => p.color)).toEqual([PART_COLORS[2], PART_COLORS[3]]);
    expect(cachedStats(shells[1].mesh)).toBe(shells[1].stats);
    expect(toasts()).toEqual([keptSingleMessage(1)]);

    s().undo();
    expect(s().parts).toBe(before);
  });

  it("split shells: say so and change nothing when every part has one shell", async () => {
    addPart("a");
    ipc.splitShells.mockResolvedValue([{ count: 1, shells: [] }]);
    const before = s().parts;
    const steps = labels().length;
    await splitShellsCommand();
    expect(toasts()).toEqual(["Part has only one shell."]);
    expect(s().parts).toBe(before);
    expect(labels()).toHaveLength(steps);
  });

  describe("split shells into more than MANY_PARTS parts", () => {
    const shellsOf = (n: number) => ({ count: n, shells: Array.from({ length: n }, () => made()) });

    /** Starts Split shells and waits for its question; `answer` gives the reply as the dialog would. */
    async function asked(): Promise<{ count: number; partName: string | null; answer(a: "split" | "repair" | null): Promise<void> }> {
      const run = splitShellsCommand();
      await vi.waitFor(() => expect(useUi.getState().dialog?.kind).toBe("split-many"));
      const dialog = useUi.getState().dialog as Extract<ReturnType<typeof useUi.getState>["dialog"], { kind: "split-many" }>;
      return {
        count: dialog.count,
        partName: dialog.partName,
        async answer(a) {
          dialog.resolve(a);
          useUi.getState().closeDialog();
          await run;
        },
      };
    }

    afterEach(async () => {
      expect(await closeRepair()).toBe(true);
    });

    it("split up to MANY_PARTS parts without asking", async () => {
      addPart("a");
      ipc.splitShells.mockResolvedValue([shellsOf(MANY_PARTS)]);
      await splitShellsCommand();
      expect(useUi.getState().dialog).toBeNull();
      expect(s().parts).toHaveLength(MANY_PARTS);
    });

    it("ask above it; Repair first opens repair mode for the part and splits nothing", async () => {
      const a = addPart("a");
      ipc.splitShells.mockResolvedValue([shellsOf(MANY_PARTS + 1)]);
      const before = s().parts;
      const steps = labels().length;
      const q = await asked();
      expect([q.count, q.partName]).toEqual([MANY_PARTS + 1, "a"]);
      // No other operation is held back while the question waits.
      expect(meshOperationRunning()).toBe(false);
      await q.answer("repair");
      expect(useRepair.getState().session?.partId).toBe(a);
      expect(useUi.getState().ribbonTab).toBe("repair");
      expect(s().parts).toBe(before);
      expect(labels()).toHaveLength(steps);
    });

    it("Split anyway splits as one undo step; cancelling changes nothing", async () => {
      addPart("a");
      ipc.splitShells.mockResolvedValue([shellsOf(MANY_PARTS + 1)]);
      const before = s().parts;
      await (await asked()).answer(null);
      expect(s().parts).toBe(before);
      expect(useRepair.getState().session).toBeNull();
      await (await asked()).answer("split");
      expect(s().parts).toHaveLength(MANY_PARTS + 1);
      expect(lastLabel()).toBe("Split shells");
      s().undo();
      expect(s().parts).toBe(before);
    });

    it("count every part made, and let the user pick which to repair when several would split", async () => {
      const a = addPart("a");
      const b = addPart("b");
      addPart("c");
      s().select([a, b]);
      ipc.splitShells.mockResolvedValue([shellsOf(300), shellsOf(300)]);
      const q = await asked();
      expect([q.count, q.partName]).toEqual([600, null]);
      await q.answer("repair");
      expect(useRepair.getState().session).toBeNull();
      expect(useUi.getState().dialog).toEqual({ kind: "repair-choose-part", ids: [a, b] });
    });

    it("drop the split when a part changed while the question was open", async () => {
      const a = addPart("a");
      ipc.splitShells.mockResolvedValue([shellsOf(MANY_PARTS + 1)]);
      const q = await asked();
      s().setPartMatrices(new Map([[a, translation(30, 0, 0)]]), "Move");
      const moved = s().parts;
      await q.answer("split");
      expect(s().parts).toBe(moved);
      expect(lastToast()).toBe(staleMessage("Split shells"));
    });
  });

  it("merge parts: need two parts, send them in tree order, and make one part as one undo step", async () => {
    const a = addPart("a");
    await mergePartsCommand();
    expect(toasts()).toEqual(["Select at least 2 parts to merge."]);
    expect(ipc.mergeMeshes).not.toHaveBeenCalled();

    const b = addPart("b");
    s().select([b, a]);
    const result = made({ shells: 2, volume: 16000 });
    ipc.mergeMeshes.mockResolvedValue(result);
    const before = s().parts;
    const steps = labels().length;
    await mergePartsCommand();
    expect(ipc.mergeMeshes.mock.calls[0][0].map((p: { name: string }) => p.name)).toEqual(["a", "b"]);
    expect(names()).toEqual(["Merge of 2 parts"]);
    expect(s().parts[0].mesh).toBe(result.mesh);
    expect(s().selection).toEqual([s().parts[0].id]);
    expect(labels()).toEqual([...before.map((p) => `Add ${p.name}`), "Merge parts"]);
    expect(labels()).toHaveLength(steps + 1);
    s().undo();
    expect(s().parts).toBe(before);
  });

  it("drop results when an input part changed while the operation ran", async () => {
    const a = addPart("a");
    const pending = deferred<unknown>();
    ipc.splitShells.mockReturnValue(pending.promise);
    const run = splitShellsCommand();
    s().setPartMatrices(new Map([[a, translation(30, 0, 0)]]), "Move");
    const moved = s().parts;
    pending.resolve([{ count: 2, shells: [made(), made()] }]);
    await run;
    expect(s().parts).toBe(moved);
    expect(lastLabel()).toBe("Move");
    expect(toasts()).toEqual([staleMessage("Split shells")]);
  });

  it("drop results when another project was started while the operation ran", async () => {
    addPart("a");
    addPart("b");
    s().select(s().parts.map((p) => p.id));
    const pending = deferred<AnalyzedMesh>();
    ipc.mergeMeshes.mockReturnValue(pending.promise);
    const run = mergePartsCommand();
    s().newProject();
    pending.resolve(made());
    await run;
    expect(s().parts).toEqual([]);
    expect(toasts()).toEqual([staleMessage("Merge parts")]);
  });

  it("run one operation at a time and accept the next once it ends", async () => {
    addPart("a");
    addPart("b");
    s().select(s().parts.map((p) => p.id));
    const pending = deferred<AnalyzedMesh>();
    ipc.mergeMeshes.mockReturnValue(pending.promise);
    const first = mergePartsCommand();
    try {
      const attempts: (() => unknown)[] = [
        splitShellsCommand,
        openCutDialog,
        () => runCut({ plane: Z10, scope: "selected", cap: true, gap: 0 }),
      ];
      for (const attempt of attempts) {
        useUi.setState({ toasts: [] });
        await attempt();
        expect(toasts()).toEqual([WAIT_MESSAGE]);
      }
      expect(ipc.splitShells).not.toHaveBeenCalled();
      expect(manifold.manifoldSplit).not.toHaveBeenCalled();
      expect(useUi.getState().dialog).toBeNull();
    } finally {
      // Ends the running merge even when an expectation failed, so later tests do not find the flag set.
      pending.resolve(made());
      await first;
    }
    ipc.splitShells.mockResolvedValue([{ count: 1, shells: [] }]);
    await splitShellsCommand();
    expect(ipc.splitShells).toHaveBeenCalledTimes(1);
  });

  it("show the status-bar task only after 200 ms, and end it and the busy flag after an error", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    addPart("a");
    const pending = deferred<unknown>();
    ipc.splitShells.mockReturnValue(pending.promise);
    const run = splitShellsCommand();
    try {
      vi.advanceTimersByTime(BUSY_DELAY_MS - 1);
      expect(useUi.getState().tasks).toEqual([]);
      vi.advanceTimersByTime(1);
      expect(useUi.getState().tasks.map((t) => t.label)).toEqual(["Splitting 1 part"]);
    } finally {
      // Rust's errors are full sentences and shown as they are.
      pending.reject("Could not split the part: it has more than 4294967295 vertices.");
      await run;
    }
    expect(useUi.getState().tasks).toEqual([]);
    expect(toasts()).toEqual(["Could not split the part: it has more than 4294967295 vertices."]);
    expect(toastKinds()).toEqual(["error"]);

    // The worker's errors are sentences too; the flag is clear, so this one runs.
    manifold.manifoldSplit.mockRejectedValue(new Error("Could not load the geometry engine: out of memory."));
    addPart("b");
    s().select(s().parts.map((p) => p.id));
    await runCut({ plane: Z10, scope: "selected", cap: true, gap: 0 });
    expect(lastToast()).toBe("Could not load the geometry engine: out of memory.");

    // Any other JS exception gets the operation's context.
    ipc.mergeMeshes.mockRejectedValue(new Error("The measurements of a result mesh are missing"));
    await mergePartsCommand();
    expect(lastToast()).toBe("Could not merge the parts: The measurements of a result mesh are missing");
    expect(useUi.getState().tasks).toEqual([]);

    // A quick operation never shows the task.
    ipc.splitShells.mockResolvedValue([{ count: 1, shells: [] }]);
    await splitShellsCommand();
    vi.advanceTimersByTime(1000);
    expect(useUi.getState().tasks).toEqual([]);
  });

  it("cut with capping: manifold first, Rust for the parts it rejects, one warning, one undo step", async () => {
    const [a, b, c, d] = ["a", "b", "c", "d"].map((n) => addPart(n));
    s().select([a, b, c, d]);
    const before = s().parts;
    const steps = labels().length;
    const aBelow = makeBox([1, 1, 1]);
    const aAbove = makeBox([1, 1, 1]);
    const outcomes = new Map<MeshData, unknown>([
      [byId(a).mesh, { kind: "ok", below: aBelow, above: aAbove }],
      [byId(b).mesh, { kind: "invalid" }],
      [byId(c).mesh, { kind: "ok", below: makeBox([1, 1, 1]), above: null }],
      [byId(d).mesh, { kind: "invalid" }],
    ]);
    manifold.manifoldSplit.mockImplementation(async (p: { mesh: MeshData }) => outcomes.get(p.mesh));
    ipc.analyzeMeshes.mockImplementation(async (meshes: MeshData[]) => meshes.map(() => ({ stats: stats({ volume: 4000 }), edges: new Float32Array(0) })));
    const rustPieces = [
      [piece("below"), piece("above")],
      [piece("below"), piece("above")],
    ];
    ipc.cutMeshes.mockResolvedValue(rustPieces.map((pieces) => ({ pieces, openOutline: true })));

    await runCut({ plane: { normal: [0, 0, 2], offset: 20 }, scope: "selected", cap: true, gap: 5 });

    expect(manifold.manifoldSplit).toHaveBeenCalledTimes(4);
    expect(manifold.manifoldSplit.mock.calls[0].slice(1)).toEqual([Z10, 5]);
    expect(ipc.analyzeMeshes).toHaveBeenCalledTimes(1);
    expect(ipc.analyzeMeshes.mock.calls[0][0]).toEqual([aBelow, aAbove]);
    const [rustParts, plane, cap, gap] = ipc.cutMeshes.mock.calls[0];
    expect(rustParts.map((p: { name: string }) => p.name)).toEqual(["b", "d"]);
    expect([plane, cap, gap]).toEqual([Z10, true, 5]);
    // Each part's pieces take its place; the untouched part stays between them.
    expect(names()).toEqual(["a (cut 1)", "a (cut 2)", "b (cut 1)", "b (cut 2)", "c", "d (cut 1)", "d (cut 2)"]);
    expect(s().parts[0].mesh).toBe(aBelow);
    expect(s().parts[6].mesh).toBe(rustPieces[1][1].mesh);
    expect(byId(c)).toBe(byIdIn(before, c));
    expect(cachedStats(rustPieces[0][0].mesh)).toBe(rustPieces[0][0].stats);
    expect(s().selection).toEqual(s().parts.filter((p) => p.id !== c).map((p) => p.id));
    expect(toasts()).toEqual([OPEN_CUT_WARNING]);
    expect(labels()).toHaveLength(steps + 1);
    expect(lastLabel()).toBe("Cut");
    s().undo();
    expect(s().parts).toBe(before);
  });

  it("cut without capping: Rust for every part, no warning", async () => {
    const [a, b] = ["a", "b"].map((n) => addPart(n));
    s().select([a, b]);
    ipc.cutMeshes.mockResolvedValue([
      { pieces: [piece("below"), piece("above")], openOutline: false },
      { pieces: [piece("above")], openOutline: false },
    ]);
    await runCut({ plane: Z10, scope: "selected", cap: false, gap: 0 });
    expect(manifold.manifoldSplit).not.toHaveBeenCalled();
    expect(ipc.cutMeshes.mock.calls[0][0].map((p: { name: string }) => p.name)).toEqual(["a", "b"]);
    expect(ipc.cutMeshes.mock.calls[0][2]).toBe(false);
    expect(names()).toEqual(["a (cut 1)", "a (cut 2)", "b"]);
    expect(toasts()).toEqual([]);
  });

  it("cut: say so when the plane misses every part, and cut all shown, unlocked parts for Cut all parts", async () => {
    const a = addPart("a");
    addPart("hidden", { visible: false });
    const c = addPart("c");
    s().select([a]);
    manifold.manifoldSplit.mockImplementation(async (p: { mesh: MeshData }) => ({ kind: "ok", below: p.mesh, above: null }));
    const before = s().parts;
    await runCut({ plane: Z10, scope: "selected", cap: true, gap: 0 });
    expect(toasts()).toEqual([noCutMessage("selected")]);
    expect(s().parts).toBe(before);

    useUi.setState({ toasts: [] });
    expect(cutTargets("all").map((p) => p.id)).toEqual([a, c]);
    await runCut({ plane: Z10, scope: "all", cap: true, gap: 0 });
    expect(manifold.manifoldSplit.mock.calls.slice(1).map(([p]) => p.mesh)).toEqual([byId(a).mesh, byId(c).mesh]);
    expect(toasts()).toEqual(["1 hidden part was not cut.", noCutMessage("all")]);
  });

  it("cut: refuse a plane with no direction", async () => {
    addPart("a");
    await runCut({ plane: { normal: [0, 0, 0], offset: 0 }, scope: "selected", cap: true, gap: 0 });
    expect(toastKinds()).toEqual(["error"]);
    expect(manifold.manifoldSplit).not.toHaveBeenCalled();
  });

  it("open the Cut dialog only when the panel's scope has parts", () => {
    useClip.getState().setCutScope("all");
    openCutDialog();
    expect(toasts()).toEqual(["There are no parts to cut."]);
    addPart("a");
    s().clearSelection();
    useClip.getState().setCutScope("selected");
    openCutDialog();
    expect(lastToast()).toBe("Select the parts to cut first.");
    expect(useUi.getState().dialog).toBeNull();
    useClip.getState().setCutScope("all");
    openCutDialog();
    expect(useUi.getState().dialog).toEqual({ kind: "cut" });
  });

  it("are wired into the feature registry", () => {
    expect(registryProblems()).toEqual([]);
    runFeature("modify.merge-parts");
    runFeature("modify.split-shells");
    runFeature("modify.cut");
    runFeature("modify.boolean");
    expect(toasts()).toEqual([
      "Select at least 2 parts to merge.",
      "Select the parts to split first.",
      "Select the parts to cut first.",
      "Select at least 2 parts to combine.",
    ]);
  });
});

function byIdIn(parts: readonly Part[], id: string): Part {
  return parts.find((p) => p.id === id) as Part;
}
