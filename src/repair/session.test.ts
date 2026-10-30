import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { MeshData, PartPacking } from "../store";
import type { ActionSpec, Catalog, Highlights, RepairProgress, RepairScript, RepairStatus, RunResult, ScriptStep } from "./types";

// Rust is a stand-in: runRepair answers what each test says, so these tests check what the session does with the
// answers: undo steps, selection, status, the progress dialog, one run at a time, cancel, Apply and Close.
const ipc = vi.hoisted(() => ({
  runRepair: vi.fn(),
  cancelRepair: vi.fn(),
  fetchCatalog: vi.fn(),
  loadStoredScripts: vi.fn(),
  saveStoredScripts: vi.fn(),
  checkMeshes: vi.fn(),
}));
const dialog = vi.hoisted(() => ({ message: vi.fn() }));
vi.mock("./ipc", async (importOriginal) => ({ ...(await importOriginal<typeof import("./ipc")>()), ...ipc }));
vi.mock("@tauri-apps/plugin-dialog", () => dialog);

import { IDENTITY, makeBox, translation } from "../mesh";
import { BUSY_DELAY_MS, WAIT_MESSAGE } from "../ops/meshOps";
import { PART_COLORS, useStore } from "../store";
import { prompt, useUi, type PlaceAnswer } from "../ui";
import { cachedCheck } from "./badges";
import { triangleBvh } from "../viewport/triPick";
import { CANCELLED } from "./ipc";
import { edgeAdjacency, maskToIds, triangleNormals, triangleShells } from "./selection";
import {
  CLOSE_REPAIR_FIRST,
  SELECT_PART_FIRST,
  SELECT_TRIANGLES_FIRST,
  applyRepair,
  canRedoRepair,
  canUndoRepair,
  cancelRun,
  closeRepair,
  enterRepair,
  expandSelection,
  extractSelection,
  invertSelection,
  isDiscard,
  isRepairing,
  redoRepair,
  refreshStatus,
  repairedPartName,
  requestApply,
  runAction,
  runScript,
  sameMesh,
  selectAll,
  selectAt,
  selectNone,
  selectTriangles,
  selectedBounds,
  selectionInfo,
  showSelectionInfo,
  shrinkSelection,
  startRepair,
  undoRepair,
  workingCopyBounds,
} from "./session";
import { DEFAULT_VIEW, useRepair } from "./store";

const spec = (id: ActionSpec["id"], label: string, over: Partial<ActionSpec> = {}): ActionSpec => ({
  id,
  label,
  implemented: true,
  inScripts: true,
  needsSelection: false,
  hint: "",
  params: [],
  ...over,
});
const catalog: Catalog = {
  actions: [
    spec("close_all_holes", "Close all holes"),
    spec("fix_flipped_triangles", "Fix flipped triangles"),
    spec("remesh", "Remesh", { implemented: false }),
    spec("wrap_part_surface", "Wrap part surface"),
    spec("detect_self_intersections", "Detect self-intersections", { inScripts: false }),
    spec("remove_selected_triangles", "Remove selected triangles", { inScripts: false, needsSelection: true }),
    spec("flip_selected_triangles", "Flip selected triangles", { inScripts: false, needsSelection: true }),
  ],
  scripts: [],
};

const STATUS: RepairStatus = {
  edges: 18,
  triangles: 12,
  vertices: 8,
  shells: 1,
  borderEdges: 4,
  nonManifoldEdges: 0,
  invertedTriangles: 2,
  holes: 1,
  degenerateTriangles: 0,
  closed: false,
  oriented: false,
  volume: 7000,
  area: 2000,
};
const FIXED: RepairStatus = { ...STATUS, borderEdges: 0, invertedTriangles: 0, holes: 0, closed: true, oriented: true, volume: 8000, area: 2400 };
const HIGHLIGHTS: Highlights = { border: new Float32Array(24), nonManifold: new Float32Array(0), inverted: new Uint32Array([3, 4]), degenerate: new Uint32Array(0) };

const unchanged = (mesh: MeshData, over: Partial<RunResult> = {}): RunResult => ({
  changed: false,
  mesh,
  stats: null,
  edges: null,
  selection: new Uint32Array(0),
  selected: false,
  reports: [],
  status: null,
  highlights: null,
  intersecting: null,
  warnings: [],
  ...over,
});
const changed = (mesh: MeshData, over: Partial<RunResult> = {}): RunResult => ({ ...unchanged(mesh), changed: true, ...over });
const report = (label: string, message: string, isChanged = true) => ({ action: "close_all_holes" as const, label, message, warnings: [], resolved: [], changed: isChanged, millis: 5 });

const r = () => useRepair.getState();
const session = () => {
  const s = r().session;
  if (!s) throw new Error("not repairing");
  return s;
};
const toasts = () => useUi.getState().toasts.map((t) => t.message);
const lastToast = () => toasts()[toasts().length - 1];
const mainLabels = () => useStore.getState().history.past.map((e) => e.label);

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** Waits until no run is going. */
const idle = () => vi.waitFor(() => expect(r().busy).toBeNull());

function addPart(name = "Part", fields: { matrix?: number[]; color?: string; mesh?: MeshData; packing?: PartPacking } = {}): string {
  const [id] = useStore.getState().addParts([{ name, mesh: fields.mesh ?? makeBox([20, 20, 20]), ...fields }], `Add ${name}`);
  return id;
}

/** Enters repair mode on a new part; the status run on entering answers STATUS. */
async function enter(fields: Parameters<typeof addPart>[1] = {}): Promise<string> {
  const id = addPart("Part", fields);
  ipc.runRepair.mockImplementationOnce(async (mesh: MeshData) => unchanged(mesh, { status: STATUS, highlights: HIGHLIGHTS }));
  enterRepair(id);
  await idle();
  ipc.runRepair.mockReset();
  useUi.setState({ toasts: [] });
  return id;
}

beforeEach(() => {
  vi.stubGlobal("window", { setTimeout: () => 0, confirm: () => true });
  vi.stubGlobal("localStorage", undefined);
  vi.stubGlobal("isTauri", true);
  for (const f of [...Object.values(ipc), dialog.message]) f.mockReset();
  ipc.cancelRepair.mockResolvedValue(undefined);
  ipc.checkMeshes.mockResolvedValue([]);
  useRepair.setState({ catalog, session: null, busy: null, autoUpdate: false, selectTool: "triangle", view: DEFAULT_VIEW });
  useStore.getState().newProject();
  useUi.setState({ toasts: [], dialog: null, ribbonTab: "home", contextMenu: null });
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

// ---------------------------------------------------------------------------------------------

describe("entering and leaving", () => {
  it("starts on the one selected part, asks which part when several are selected, and needs one", async () => {
    startRepair();
    expect(lastToast()).toBe(SELECT_PART_FIRST);
    const a = addPart("a");
    const b = addPart("b");
    useStore.getState().select([a, b]);
    startRepair();
    expect(useUi.getState().dialog).toEqual({ kind: "repair-choose-part", ids: [a, b] });
    expect(isRepairing()).toBe(false);
    ipc.runRepair.mockImplementation(async (mesh: MeshData) => unchanged(mesh));
    useStore.getState().select([b]);
    startRepair();
    expect(isRepairing()).toBe(true);
    expect(session().partId).toBe(b);
    await idle();
    startRepair();
    expect(lastToast()).toBe(CLOSE_REPAIR_FIRST);
    enterRepair(a);
    expect(session().partId).toBe(b);
  });

  it("bakes the part's matrix into a world-space working copy, opens the Repair tab, and computes the status", async () => {
    const id = addPart("Part", { matrix: translation(100, 50, 10) });
    const status = deferred<RunResult>();
    ipc.runRepair.mockReturnValueOnce(status.promise);
    enterRepair(id);
    const s = session();
    expect(s.partName).toBe("Part");
    expect(s.mesh).toBe(s.initialMesh);
    expect(s.mesh.positions[0]).toBe(90); // -10 + 100
    expect(s.mesh.positions[2]).toBe(0); // -10 + 10
    expect(s.selection.length).toBe(12);
    expect(s.selectedCount).toBe(0);
    expect(s.status).toBeNull();
    expect(useUi.getState().ribbonTab).toBe("repair");
    expect(useStore.getState().selection).toEqual([id]);
    const [mesh, steps, opts] = ipc.runRepair.mock.calls[0];
    expect(mesh).toBe(s.mesh);
    expect(steps).toEqual([]);
    expect(opts.withStatus).toBe(true);
    status.resolve(unchanged(s.mesh, { status: STATUS, highlights: HIGHLIGHTS }));
    await idle();
    expect(session().status).toEqual(STATUS);
    expect(session().highlights).toBe(HIGHLIGHTS);
    expect(session().past).toEqual([]);
    expect(toasts()).toEqual([]);
  });

  it("refuses parts that are gone or have no triangles", () => {
    enterRepair("part-404");
    expect(lastToast()).toBe("That part no longer exists.");
    const id = addPart("Empty", { mesh: { positions: new Float32Array(0), indices: new Uint32Array(0) } });
    enterRepair(id);
    expect(lastToast()).toBe('"Empty" has no triangles to repair.');
    expect(isRepairing()).toBe(false);
  });

  it("closes without asking when nothing changed, and goes back to the tab it came from", async () => {
    await enter();
    expect(await closeRepair()).toBe(true);
    expect(isRepairing()).toBe(false);
    expect(dialog.message).not.toHaveBeenCalled();
    expect(useUi.getState().ribbonTab).toBe("home");
    expect(await closeRepair()).toBe(true);
  });

  it("asks before throwing changes away, and stays open on Cancel", async () => {
    await enter();
    ipc.runRepair.mockImplementation(async () => changed(makeBox([1, 1, 1])));
    await runAction("close_all_holes");
    dialog.message.mockResolvedValueOnce("Cancel");
    expect(await closeRepair()).toBe(false);
    expect(isRepairing()).toBe(true);
    expect(dialog.message.mock.calls[0][0]).toBe("Discard repair changes?");
    expect(dialog.message.mock.calls[0][1].buttons).toEqual({ ok: "Discard", cancel: "Cancel" });
    dialog.message.mockResolvedValueOnce("Discard");
    expect(await closeRepair()).toBe(true);
    expect(isRepairing()).toBe(false);
    expect(mainLabels()).toEqual(["Add Part"]);
  });

  it("does not ask after undoing back to the starting mesh", async () => {
    await enter();
    ipc.runRepair.mockImplementation(async () => changed(makeBox([1, 1, 1])));
    await runAction("close_all_holes");
    undoRepair();
    expect(await closeRepair()).toBe(true);
    expect(dialog.message).not.toHaveBeenCalled();
  });

  it("knows the message box's Discard answer", () => {
    expect([isDiscard("Discard"), isDiscard("Ok"), isDiscard("Cancel"), isDiscard("")]).toEqual([true, true, false, false]);
  });

  it("leaves repair mode when another project replaces the document", async () => {
    await enter();
    useStore.getState().newProject();
    expect(isRepairing()).toBe(false);
    expect(useUi.getState().ribbonTab).toBe("home");
  });

  it("lets go of the picking and selection caches built for its working copies when it ends", async () => {
    for (const end of [() => closeRepair(), () => useStore.getState().newProject()]) {
      await enter();
      const mesh = session().mesh;
      const cached = [edgeAdjacency(mesh), triangleNormals(mesh), triangleShells(mesh), triangleBvh(mesh)];
      await end();
      expect(isRepairing()).toBe(false);
      // Built anew on the next use: the caches no longer held the working copy or anything made from it.
      const now = [edgeAdjacency(mesh), triangleNormals(mesh), triangleShells(mesh), triangleBvh(mesh)];
      now.forEach((value, i) => expect(value).not.toBe(cached[i]));
    }
  });
});

describe("runs", () => {
  it("run an action on the selection, record one undo step labelled with the action, and carry the selection", async () => {
    await enter();
    const before = session();
    selectTriangles([1, 4], "replace");
    const fixed = makeBox([30, 30, 30]);
    ipc.runRepair.mockResolvedValueOnce(
      changed(fixed, { selection: new Uint32Array([2, 5]), reports: [report("Close all holes", "Closed 1 hole, added 2 triangles.")] }),
    );
    await runAction("close_all_holes");
    const [mesh, steps, opts] = ipc.runRepair.mock.calls[0];
    expect(mesh).toBe(before.mesh);
    expect(steps).toEqual([{ type: "close_all_holes", enabled: true, params: {} }]);
    expect(Array.from(opts.selection)).toEqual([1, 4]);
    expect(opts.withStatus).toBe(false);
    expect(opts.label).toBe("Close all holes");
    const s = session();
    expect(s.mesh).toBe(fixed);
    expect(Array.from(maskToIds(s.selection))).toEqual([2, 5]);
    expect(s.selectedCount).toBe(2);
    expect(s.status).toBeNull(); // Auto-update off: "?" until Update
    expect(s.past.map((p) => p.label)).toEqual(["Close all holes"]);
    expect(s.past[0].mesh).toBe(before.mesh);
    expect(Array.from(maskToIds(s.past[0].selection))).toEqual([1, 4]);
    expect(lastToast()).toBe("Closed 1 hole, added 2 triangles.");
    expect(canUndoRepair()).toBe(true);
    expect(canRedoRepair()).toBe(false);
  });

  it("record the selection a run started from as its undo state, even when it changed while the run went on", async () => {
    await enter();
    const start = session();
    selectTriangles([1], "replace");
    const pending = deferred<RunResult>();
    ipc.runRepair.mockReturnValueOnce(pending.promise);
    const run = runAction("flip_selected_triangles");
    // Before the progress dialog covers the window, a key (Esc, = or -) can still change the selection.
    selectTriangles([7, 8], "replace");
    pending.resolve(changed(makeBox([1, 1, 1]), { selection: new Uint32Array([1]) }));
    await run;
    expect(Array.from(maskToIds(session().selection))).toEqual([1]);
    expect(Array.from(maskToIds(session().past[0].selection))).toEqual([1]);
    undoRepair();
    expect(session().mesh).toBe(start.mesh);
    expect(Array.from(maskToIds(session().selection))).toEqual([1]);
    redoRepair();
    expect(Array.from(maskToIds(session().selection))).toEqual([1]);
  });

  it("bring the status along with Auto-update, and add no undo step when nothing changed", async () => {
    await enter();
    useRepair.getState().setAutoUpdate(true);
    ipc.runRepair.mockImplementationOnce(async (mesh: MeshData) =>
      unchanged(mesh, { status: FIXED, highlights: HIGHLIGHTS, reports: [report("Fix flipped triangles", "No flipped triangles.", false)] }),
    );
    const before = session();
    await runAction("fix_flipped_triangles");
    expect(ipc.runRepair.mock.calls[0][2].withStatus).toBe(true);
    expect(session().mesh).toBe(before.mesh);
    expect(session().selection).toBe(before.selection);
    expect(session().past).toEqual([]);
    expect(session().status).toEqual(FIXED);
    expect(lastToast()).toBe("No flipped triangles.");
  });

  it("keep the self-intersections Detect found, without an undo step", async () => {
    await enter();
    ipc.runRepair.mockImplementationOnce(async (mesh: MeshData) =>
      unchanged(mesh, { intersecting: new Uint32Array([3, 8]), reports: [report("Detect self-intersections", "Found 2 self-intersecting triangles.", false)] }),
    );
    await runAction("detect_self_intersections");
    expect(Array.from(session().intersecting ?? [])).toEqual([3, 8]);
    expect(session().past).toEqual([]);
    expect(lastToast()).toBe("Found 2 self-intersecting triangles.");
    // The next change drops them.
    ipc.runRepair.mockResolvedValueOnce(changed(makeBox([1, 1, 1])));
    await runAction("close_all_holes");
    expect(session().intersecting).toBeNull();
  });

  it("refuse actions that need a selection when there is none, and Soon actions", async () => {
    await enter();
    await runAction("remove_selected_triangles");
    expect(lastToast()).toBe(SELECT_TRIANGLES_FIRST);
    await runAction("remesh");
    expect(lastToast()).toBe("Remesh is coming in a future version.");
    expect(ipc.runRepair).not.toHaveBeenCalled();
  });

  it("run a script without a selection, refresh the status, keep its reports, and make it one undo step", async () => {
    await enter();
    selectTriangles([0, 1, 2], "replace");
    const script: RepairScript = {
      id: "own.1",
      name: "My repair",
      builtIn: false,
      actions: [
        { type: "close_all_holes", enabled: true, params: {} },
        { type: "remesh", enabled: false, params: {} },
        { type: "fix_flipped_triangles", enabled: true, params: {} },
      ],
    };
    const fixed = makeBox([2, 2, 2]);
    const reports = [report("Close all holes", "Closed 1 hole, added 2 triangles."), report("Fix flipped triangles", "Flipped 2 triangles.")];
    ipc.runRepair.mockResolvedValueOnce(changed(fixed, { status: FIXED, highlights: HIGHLIGHTS, reports, warnings: ["Something to know."] }));
    await runScript(script);
    const [, steps, opts] = ipc.runRepair.mock.calls[0];
    expect(steps).toEqual(script.actions);
    expect(opts.selection.length).toBe(0);
    expect(opts.withStatus).toBe(true);
    expect(opts.label).toBe("My repair");
    const s = session();
    expect(s.mesh).toBe(fixed);
    expect(s.selectedCount).toBe(0);
    expect(s.selection.length).toBe(12);
    expect(s.status).toEqual(FIXED);
    expect(s.lastRun).toEqual(reports);
    expect(s.past.map((p) => p.label)).toEqual(["My repair"]);
    expect(toasts()).toEqual(["My repair finished in 10 ms: 2 of 2 actions changed the mesh.", "Something to know."]);
    // Undo brings back the selection the script started with.
    undoRepair();
    expect(Array.from(maskToIds(session().selection))).toEqual([0, 1, 2]);
  });

  it("refuse scripts that cannot run, saying why", async () => {
    await enter();
    const bad: RepairScript = { id: "own.1", name: "Bad", builtIn: false, actions: [{ type: "remesh", enabled: true, params: {} }] };
    await runScript(bad);
    expect(lastToast()).toBe('"Bad" cannot run. Step 1, Remesh, is not available yet.');
    await runScript({ ...bad, actions: [{ type: "remesh", enabled: false, params: {} }] });
    expect(lastToast()).toBe('"Bad" has no enabled actions to run.');
    expect(ipc.runRepair).not.toHaveBeenCalled();
  });

  it("run one at a time: everything that would change the working copy waits", async () => {
    await enter();
    const pending = deferred<RunResult>();
    ipc.runRepair.mockReturnValueOnce(pending.promise);
    const first = runAction("close_all_holes");
    try {
      const attempts: (() => unknown)[] = [
        () => runAction("fix_flipped_triangles"),
        () => runScript({ id: "own.1", name: "S", builtIn: false, actions: [{ type: "close_all_holes", enabled: true, params: {} }] }),
        refreshStatus,
        undoRepair,
        redoRepair,
        () => extractSelection(false),
      ];
      for (const attempt of attempts) {
        useUi.setState({ toasts: [] });
        await attempt();
        expect(toasts()).toEqual([WAIT_MESSAGE]);
      }
      expect(await closeRepair()).toBe(false);
      requestApply();
      applyRepair("remove");
      expect(useUi.getState().dialog).toBeNull();
      expect(isRepairing()).toBe(true);
      expect(ipc.runRepair).toHaveBeenCalledTimes(1);
    } finally {
      pending.resolve(changed(makeBox([1, 1, 1])));
      await first;
    }
    expect(r().busy).toBeNull();
  });

  it("show the progress dialog only after 200 ms, follow the progress, and close it when the run ends", async () => {
    await enter();
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    const pending = deferred<RunResult>();
    let progress: ((p: RepairProgress) => void) | undefined;
    ipc.runRepair.mockImplementationOnce((_m: MeshData, _s: ScriptStep[], opts: { onProgress?: (p: RepairProgress) => void }) => {
      progress = opts.onProgress;
      return pending.promise;
    });
    const run = runAction("wrap_part_surface");
    try {
      expect(r().busy).toMatchObject({ label: "Wrap part surface", progress: null, step: null, detail: null, local: null, cancelling: false });
      vi.advanceTimersByTime(BUSY_DELAY_MS - 1);
      expect(useUi.getState().dialog).toBeNull();
      vi.advanceTimersByTime(1);
      expect(useUi.getState().dialog).toEqual({ kind: "repair-progress" });
      progress?.({ job: r().busy?.job ?? 0, step: 0, steps: 1, label: "Wrap part surface", detail: "Finding intersections", local: 0.4, fraction: 0.4 });
      expect(r().busy).toMatchObject({ progress: 0.4, step: "Wrap part surface", detail: "Finding intersections", local: 0.4 });
      // (ipc.ts forwards only this job's events; see ipc.test.ts.)
      progress?.({ job: r().busy?.job ?? 0, step: 2, steps: 11, label: "Fix flipped triangles", fraction: 0.5 });
      expect(r().busy?.step).toBe("Fix flipped triangles (3 of 11)");
      expect(r().busy?.progress).toBe(0.5);
      // An action that names no step leaves no old one showing.
      expect(r().busy?.detail).toBeNull();
    } finally {
      pending.resolve(changed(makeBox([1, 1, 1])));
      await run;
    }
    expect(useUi.getState().dialog).toBeNull();
    expect(r().busy).toBeNull();
    // A quick run never shows it.
    ipc.runRepair.mockResolvedValueOnce(changed(makeBox([2, 2, 2])));
    await runAction("close_all_holes");
    vi.advanceTimersByTime(1000);
    expect(useUi.getState().dialog).toBeNull();
  });

  it("never show the progress dialog in place of another one: it waits until that closes, if the run still goes", async () => {
    await enter();
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    const settle = async () => {
      for (let i = 0; i < 10; i++) await Promise.resolve();
    };
    const dialogKind = () => useUi.getState().dialog?.kind ?? null;
    /** "Place on platform?" for a file dropped on the window, answered when the test says. */
    const askPlace = () => prompt<PlaceAnswer>((resolve) => ({ kind: "place-on-import", fileName: "big.stl", resolve }));
    const answer = (place: PlaceAnswer) => {
      const d = useUi.getState().dialog;
      if (d?.kind !== "place-on-import") throw new Error(`no placement prompt but ${d?.kind}`);
      d.resolve(place);
      useUi.getState().closeDialog();
    };

    // The prompt opens 100 ms into the run; at 200 ms the progress dialog waits behind it instead of cancelling it.
    let pending = deferred<RunResult>();
    ipc.runRepair.mockReturnValueOnce(pending.promise);
    let run = runAction("wrap_part_surface");
    vi.advanceTimersByTime(100);
    let asked = askPlace();
    await settle();
    expect(dialogKind()).toBe("place-on-import");
    vi.advanceTimersByTime(BUSY_DELAY_MS);
    await settle();
    expect(dialogKind()).toBe("place-on-import");
    answer({ place: true, remember: false });
    expect(await asked).toEqual({ place: true, remember: false });
    await settle();
    expect(dialogKind()).toBe("repair-progress");
    pending.resolve(changed(makeBox([1, 1, 1])));
    await run;
    expect(dialogKind()).toBeNull();

    // A run that ends while the prompt is still up never shows its progress dialog.
    pending = deferred<RunResult>();
    ipc.runRepair.mockReturnValueOnce(pending.promise);
    run = runAction("wrap_part_surface");
    asked = askPlace();
    await settle();
    vi.advanceTimersByTime(BUSY_DELAY_MS);
    await settle();
    pending.resolve(changed(makeBox([2, 2, 2])));
    await run;
    expect(dialogKind()).toBe("place-on-import");
    answer({ place: false, remember: false });
    expect(await asked).toEqual({ place: false, remember: false });
    await settle();
    expect(dialogKind()).toBeNull();
  });

  it("cancel: Rust is asked to stop, and the working copy, selection and history stay exactly as they were", async () => {
    await enter();
    ipc.runRepair.mockResolvedValueOnce(changed(makeBox([5, 5, 5])));
    await runAction("close_all_holes");
    selectTriangles([3], "replace");
    useUi.setState({ toasts: [] });
    const before = session();
    const pending = deferred<RunResult>();
    ipc.runRepair.mockReturnValueOnce(pending.promise);
    const run = runScript({ id: "own.1", name: "Long", builtIn: false, actions: [{ type: "wrap_part_surface", enabled: true, params: {} }] });
    const job = r().busy?.job;
    cancelRun();
    cancelRun();
    expect(r().busy?.cancelling).toBe(true);
    expect(ipc.cancelRepair).toHaveBeenCalledTimes(1);
    expect(ipc.cancelRepair).toHaveBeenCalledWith(job);
    pending.reject(CANCELLED);
    await run;
    expect(r().session).toBe(before);
    expect(r().busy).toBeNull();
    expect(toasts()).toEqual([]);
  });

  it("cancel: a result that finished before Rust saw the request is dropped as well", async () => {
    await enter();
    const before = session();
    const pending = deferred<RunResult>();
    ipc.runRepair.mockReturnValueOnce(pending.promise);
    const run = runAction("wrap_part_surface");
    cancelRun();
    pending.resolve(changed(makeBox([9, 9, 9]), { reports: [report("Wrap part surface", "Wrapped the surface.")] }));
    await run;
    expect(r().session).toBe(before);
    expect(r().busy).toBeNull();
    expect(toasts()).toEqual([]);
  });

  it("show Rust's errors as they are and other errors after their context", async () => {
    await enter();
    ipc.runRepair.mockRejectedValueOnce("Wrap part surface ran out of memory.");
    await runAction("wrap_part_surface");
    expect(useUi.getState().toasts.map((t) => [t.kind, t.message])).toEqual([["error", "Wrap part surface ran out of memory."]]);
    ipc.runRepair.mockRejectedValueOnce(new Error("The repair result is missing its mesh."));
    await runAction("close_all_holes");
    expect(lastToast()).toBe("Could not run Close all holes: The repair result is missing its mesh.");
    expect(r().busy).toBeNull();
    expect(session().past).toEqual([]);
  });

  it("drop a result that arrives after repair mode closed", async () => {
    await enter();
    const pending = deferred<RunResult>();
    ipc.runRepair.mockReturnValueOnce(pending.promise);
    const run = runAction("close_all_holes");
    useStore.getState().newProject(); // leaves repair mode and cancels the run
    expect(ipc.cancelRepair).toHaveBeenCalled();
    pending.resolve(changed(makeBox([1, 1, 1])));
    await run;
    expect(r().session).toBeNull();
    expect(r().busy).toBeNull();
  });

  it("refuse in a plain browser", async () => {
    await enter();
    vi.stubGlobal("isTauri", false);
    await runAction("close_all_holes");
    expect(lastToast()).toBe("Repair needs the tricky3D desktop app.");
    expect(ipc.runRepair).not.toHaveBeenCalled();
  });
});

describe("inner undo", () => {
  it("steps back and forth through runs, with their selection and known status", async () => {
    await enter();
    const start = session();
    const m1 = makeBox([1, 1, 1]);
    const m2 = makeBox([2, 2, 2]);
    ipc.runRepair.mockResolvedValueOnce(changed(m1, { status: FIXED, highlights: HIGHLIGHTS }));
    await runAction("close_all_holes");
    selectTriangles([7], "replace");
    ipc.runRepair.mockResolvedValueOnce(changed(m2, { selection: new Uint32Array([7]) }));
    await runAction("fix_flipped_triangles");
    expect(session().past.map((p) => p.label)).toEqual(["Close all holes", "Fix flipped triangles"]);

    undoRepair();
    expect(session().mesh).toBe(m1);
    expect(Array.from(maskToIds(session().selection))).toEqual([7]);
    expect(session().status).toEqual(FIXED);
    expect(canRedoRepair()).toBe(true);
    undoRepair();
    expect(session().mesh).toBe(start.mesh);
    expect(session().status).toEqual(STATUS); // computed on entering
    expect(session().selectedCount).toBe(0);
    undoRepair();
    expect(session().mesh).toBe(start.mesh);
    redoRepair();
    redoRepair();
    expect(session().mesh).toBe(m2);
    expect(session().status).toBeNull();
    expect(canRedoRepair()).toBe(false);

    // A new step after undoing clears the redo list.
    undoRepair();
    ipc.runRepair.mockResolvedValueOnce(changed(makeBox([3, 3, 3])));
    await runAction("close_all_holes");
    expect(canRedoRepair()).toBe(false);
    expect(session().past.map((p) => p.label)).toEqual(["Close all holes", "Close all holes"]);
  });

  it("updates the status after an undo to a mesh without one when Auto-update is on", async () => {
    await enter();
    ipc.runRepair.mockResolvedValueOnce(changed(makeBox([1, 1, 1])));
    await runAction("close_all_holes");
    ipc.runRepair.mockResolvedValueOnce(changed(makeBox([2, 2, 2])));
    await runAction("close_all_holes");
    useRepair.getState().setAutoUpdate(true);
    ipc.runRepair.mockImplementationOnce(async (mesh: MeshData) => unchanged(mesh, { status: FIXED, highlights: HIGHLIGHTS }));
    undoRepair();
    await idle();
    const calls = ipc.runRepair.mock.calls;
    expect(calls[calls.length - 1][1]).toEqual([]);
    expect(session().status).toEqual(FIXED);
  });
});

describe("selection", () => {
  it("picks a triangle, a surface or a shell, and combines picks by mode", async () => {
    await enter();
    selectAt(4, "replace");
    expect(Array.from(maskToIds(session().selection))).toEqual([4]);
    selectAt(6, "toggle");
    expect(Array.from(maskToIds(session().selection))).toEqual([4, 6]);
    selectAt(6, "toggle");
    expect(Array.from(maskToIds(session().selection))).toEqual([4]);
    useRepair.getState().setSelectTool("surface");
    useRepair.getState().setView({ surfaceTolerance: 30 });
    selectAt(8, "add");
    expect(Array.from(maskToIds(session().selection))).toEqual([4, 8, 9]);
    useRepair.getState().setView({ surfaceTolerance: 180 });
    selectAt(0, "replace");
    expect(session().selectedCount).toBe(12);
    useRepair.getState().setSelectTool("shell");
    selectAt(0, "remove");
    expect(session().selectedCount).toBe(0);
    expect(session().past).toEqual([]); // selection is no undo step
  });

  it("selects all, none, the inverse, more and less", async () => {
    await enter();
    selectAll();
    expect(session().selectedCount).toBe(12);
    selectNone();
    expect(session().selectedCount).toBe(0);
    const same = session().selection;
    selectNone();
    expect(session().selection).toBe(same);
    selectTriangles([0], "replace");
    invertSelection();
    expect(session().selectedCount).toBe(11);
    expect(session().selection[0]).toBe(0);
    selectTriangles([0], "replace");
    expandSelection(); // every triangle sharing a corner with the bottom triangle 0 (corners 0, 1, 2)
    const grown = session().selectedCount;
    expect(grown).toBeGreaterThan(1);
    shrinkSelection();
    expect(session().selectedCount).toBeLessThan(grown);
  });

  it("reports the selection's count, area and shells, and its bounds", async () => {
    await enter({ matrix: translation(100, 0, 0) });
    expect(selectedBounds()).toBeNull();
    expect(workingCopyBounds()).toEqual({ min: [90, -10, -10], max: [110, 10, 10] });
    selectTriangles([2, 3], "replace");
    expect(selectionInfo()).toEqual({ count: 2, area: 400, shells: 1 });
    expect(selectedBounds()).toEqual({ min: [90, -10, 10], max: [110, 10, 10] });
    showSelectionInfo();
    expect(lastToast()).toBe("2 triangles selected, area 4.000 cm², in 1 shell");
  });
});

describe("apply repair", () => {
  async function repaired(fields: Parameters<typeof addPart>[1] = {}) {
    const id = await enter(fields);
    const mesh = makeBox([30, 30, 30]);
    ipc.runRepair.mockResolvedValueOnce(changed(mesh, { status: FIXED, highlights: HIGHLIGHTS }));
    await runScript({ id: "own.1", name: "Fix", builtIn: false, actions: [{ type: "close_all_holes", enabled: true, params: {} }] });
    return { id, mesh };
  }

  it("remove: the repaired part takes the old one's place, name, colour and packing settings as one main undo step, and leaves repair mode", async () => {
    addPart("Before");
    const { id, mesh } = await repaired({ matrix: translation(5, 0, 0), color: PART_COLORS[3], packing: { priority: 7, rotation: "z" } });
    addPart("After");
    useStore.getState().select([id]);
    const parts = useStore.getState().parts;
    requestApply();
    expect(useUi.getState().dialog).toEqual({ kind: "repair-apply" });
    applyRepair("remove");
    const now = useStore.getState().parts;
    expect(now.map((p) => p.name)).toEqual(["Before", "Part", "After"]);
    const part = now[1];
    expect(part.id).not.toBe(id);
    expect(part.mesh).toBe(mesh);
    expect(part.matrix).toEqual([...IDENTITY]);
    expect(part.color).toBe(PART_COLORS[3]);
    expect(part.packing).toEqual({ priority: 7, rotation: "z" });
    expect(mainLabels()[mainLabels().length - 1]).toBe("Repair");
    expect(cachedCheck(mesh)).toEqual({ closed: true, oriented: true, holes: 0, borderEdges: 0, nonManifoldEdges: 0, invertedTriangles: 0 });
    expect(isRepairing()).toBe(false);
    expect(useUi.getState().dialog).toBeNull();
    expect(useUi.getState().ribbonTab).toBe("home");
    useStore.getState().undo();
    expect(useStore.getState().parts).toBe(parts);
  });

  it("keep: the repaired part goes right after the old one, named (repaired), in a fresh colour", async () => {
    const { id, mesh } = await repaired({ color: PART_COLORS[0] });
    addPart("Other");
    applyRepair("keep");
    const now = useStore.getState().parts;
    expect(now.map((p) => p.name)).toEqual(["Part", "Part (repaired)", "Other"]);
    expect(now[0].id).toBe(id);
    expect(now[1].mesh).toBe(mesh);
    expect(now[1].color).not.toBe(PART_COLORS[0]);
    expect(now[0].transparent).toBe(false);
    expect(useStore.getState().selection).toEqual([now[1].id]);
  });

  it("compare: like keep, with the old part shown transparent in the same undo step", async () => {
    const { id } = await repaired();
    useStore.getState().setPartsVisible([id], false);
    const steps = mainLabels().length;
    applyRepair("compare");
    const now = useStore.getState().parts;
    expect(now.map((p) => [p.name, p.visible, p.transparent])).toEqual([
      ["Part", true, true],
      ["Part (repaired)", true, false],
    ]);
    expect(mainLabels()).toHaveLength(steps + 1);
    useStore.getState().undo();
    expect(useStore.getState().parts.map((p) => [p.name, p.visible, p.transparent])).toEqual([["Part", false, false]]);
  });

  it("changes nothing when the working copy did not change", async () => {
    await enter();
    const steps = mainLabels().length;
    applyRepair("remove");
    expect(mainLabels()).toHaveLength(steps);
    expect(isRepairing()).toBe(false);
    expect(lastToast()).toBe("Nothing was changed, so the part stays as it was.");
  });

  it("compares the working copy itself with the part, not what the runs reported", async () => {
    await enter();
    const part = session().initialMesh;
    // A run that reports a change but hands back the same triangles: nothing to apply.
    const copy = { positions: part.positions.slice(), indices: part.indices.slice() };
    ipc.runRepair.mockResolvedValueOnce(changed(copy));
    await runAction("close_all_holes");
    expect(session().mesh).toBe(copy);
    expect(await closeRepair()).toBe(true);
    expect(dialog.message).not.toHaveBeenCalled();

    // One vertex moved by an f32 step: applied.
    await enter();
    const moved = { positions: part.positions.slice(), indices: part.indices };
    moved.positions[4] = Math.fround(moved.positions[4] + 2 ** -20);
    ipc.runRepair.mockResolvedValueOnce(changed(moved));
    await runAction("close_all_holes");
    const steps = mainLabels().length;
    applyRepair("remove");
    expect(mainLabels()).toHaveLength(steps + 1);
    const parts = useStore.getState().parts;
    expect(parts[parts.length - 1].mesh).toBe(moved);

    await enter();
    ipc.runRepair.mockResolvedValueOnce(changed({ positions: part.positions, indices: part.indices.slice() }));
    await runAction("close_all_holes");
    applyRepair("keep");
    expect(lastToast()).toBe("Nothing was changed, so the part stays as it was.");
  });

  it("refuses an empty result, and adds the result as a new part when the original is gone", async () => {
    await enter();
    ipc.runRepair.mockResolvedValueOnce(changed({ positions: new Float32Array(0), indices: new Uint32Array(0) }));
    await runAction("close_all_holes");
    applyRepair("remove");
    expect(isRepairing()).toBe(true);
    expect(lastToast()).toMatch(/no triangles left/);

    const mesh = makeBox([3, 3, 3]);
    ipc.runRepair.mockResolvedValueOnce(changed(mesh));
    await runAction("close_all_holes");
    useStore.getState().deleteParts([session().partId]);
    applyRepair("remove");
    expect(useStore.getState().parts.map((p) => [p.name, p.mesh])).toEqual([["Part", mesh]]);
    expect(isRepairing()).toBe(false);
  });
});

describe("sameMesh", () => {
  it("compares the arrays bit for bit, wherever their views start", () => {
    const box = makeBox([2, 2, 2]);
    const buffer = new Float32Array(box.positions.length + 3);
    buffer.set(box.positions, 3);
    const offset = { positions: buffer.subarray(3), indices: box.indices };
    expect(sameMesh(box, box)).toBe(true);
    expect(sameMesh(box, offset)).toBe(true);
    expect(sameMesh(box, { ...box, indices: box.indices.slice(3) })).toBe(false);
    const flipped = box.indices.slice();
    [flipped[1], flipped[2]] = [flipped[2], flipped[1]];
    expect(sameMesh(box, { ...box, indices: flipped })).toBe(false);
    // -0 and 0 are different vertices to an export; NaN equals itself.
    const plus = { ...box, positions: box.positions.slice().fill(0, 0, 1) };
    const minus = { ...box, positions: box.positions.slice().fill(-0, 0, 1) };
    expect(sameMesh(plus, minus)).toBe(false);
    const nan = { ...box, positions: box.positions.slice().fill(NaN, 0, 1) };
    expect(sameMesh(nan, { ...nan, positions: nan.positions.slice() })).toBe(true);
  });
});

describe("extract", () => {
  it("adds the selected triangles as a new part, and with separate also removes them from the working copy", async () => {
    const id = await enter();
    await extractSelection(false);
    expect(lastToast()).toBe(SELECT_TRIANGLES_FIRST);
    selectTriangles([2, 3], "replace");
    await extractSelection(false);
    const parts = useStore.getState().parts;
    expect(parts.map((p) => p.name)).toEqual(["Part", "Part (extracted)"]);
    expect(parts[1].mesh.indices.length).toBe(6);
    expect(parts[1].mesh.positions.length).toBe(12);
    expect(mainLabels()[mainLabels().length - 1]).toBe("Extract part");
    expect(useStore.getState().selection).toEqual([id]);
    expect(session().past).toEqual([]);

    const rest = makeBox([4, 4, 4]);
    ipc.runRepair.mockResolvedValueOnce(changed(rest));
    await extractSelection(true);
    const [, steps, opts] = ipc.runRepair.mock.calls[0];
    expect(steps).toEqual([{ type: "remove_selected_triangles", enabled: true, params: {} }]);
    expect(Array.from(opts.selection)).toEqual([2, 3]);
    expect(session().mesh).toBe(rest);
    expect(session().past.map((p) => p.label)).toEqual(["Separate selection as part"]);
    expect(useStore.getState().parts).toHaveLength(3);

    // When the removal fails, no part is added.
    selectTriangles([0], "replace");
    ipc.runRepair.mockRejectedValueOnce("Could not remove the triangles.");
    await extractSelection(true);
    expect(useStore.getState().parts).toHaveLength(3);
  });

  it("names the new part after the repaired part's name now, which can change in repair mode", async () => {
    const id = await enter();
    // The tree's F2 and double-click, and the part menu's Rename, stay open in repair mode.
    useStore.getState().renamePart(id, "Arm");
    selectTriangles([2, 3], "replace");
    await extractSelection(false);
    expect(useStore.getState().parts.map((p) => p.name)).toEqual(["Arm", "Arm (extracted)"]);
    expect(lastToast()).toBe('Added "Arm (extracted)" with 2 triangles.');
  });
});

describe("repairedPartName", () => {
  it("is the part's name now, or the name it had on entering when the part is gone", async () => {
    const id = await enter();
    expect(repairedPartName(session())).toBe("Part");
    useStore.getState().renamePart(id, "Bracket v2");
    expect(session().partName).toBe("Part");
    expect(repairedPartName(session())).toBe("Bracket v2");
    expect(repairedPartName(session(), [])).toBe("Part");
    useStore.getState().deleteParts([id]);
    expect(repairedPartName(session())).toBe("Part");
  });
});
