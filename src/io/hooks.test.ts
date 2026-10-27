import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { MeshData } from "../store";
import type { Catalog, RunResult } from "../repair/types";

// Rust and the message boxes are stand-ins: runRepair and message answer what each test says.
const ipc = vi.hoisted(() => ({
  runRepair: vi.fn(),
  cancelRepair: vi.fn(),
  fetchCatalog: vi.fn(),
  loadStoredScripts: vi.fn(),
  saveStoredScripts: vi.fn(),
  checkMeshes: vi.fn(),
}));
const dialog = vi.hoisted(() => ({ message: vi.fn() }));
vi.mock("../repair/ipc", async (importOriginal) => ({ ...(await importOriginal<typeof import("../repair/ipc")>()), ...ipc }));
vi.mock("@tauri-apps/plugin-dialog", () => dialog);

import { makeBox } from "../mesh";
import { enterRepair, isRepairing, runAction } from "../repair/session";
import { useRepair } from "../repair/store";
import { useStore } from "../store";
import { useUi } from "../ui";
import { listenWhileMounted, mayCloseWindow } from "./hooks";

/** A listen() whose promise the test settles by hand. */
function deferredListen() {
  const unlisten = vi.fn();
  let settle!: (fail?: boolean) => void;
  const listen = () =>
    new Promise<() => void>((resolve, reject) => {
      settle = (fail) => (fail ? reject(new Error("no permission")) : resolve(unlisten));
    });
  return { listen, unlisten, settle: (fail?: boolean) => settle(fail) };
}

const flush = () => new Promise((r) => setTimeout(r, 0));

describe("listenWhileMounted", () => {
  it("removes the listener on cleanup", async () => {
    const d = deferredListen();
    const cleanup = listenWhileMounted(d.listen);
    d.settle();
    await flush();
    expect(d.unlisten).not.toHaveBeenCalled();
    cleanup();
    expect(d.unlisten).toHaveBeenCalledTimes(1);
    cleanup(); // a second cleanup does nothing
    expect(d.unlisten).toHaveBeenCalledTimes(1);
  });

  it("removes a listener that arrives after cleanup (StrictMode mount, unmount, mount)", async () => {
    const d = deferredListen();
    const cleanup = listenWhileMounted(d.listen);
    cleanup();
    expect(d.unlisten).not.toHaveBeenCalled();
    d.settle();
    await flush();
    expect(d.unlisten).toHaveBeenCalledTimes(1);
  });

  it("logs a failed listen instead of rejecting", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const d = deferredListen();
    const cleanup = listenWhileMounted(d.listen);
    d.settle(true);
    await flush();
    expect(error).toHaveBeenCalledTimes(1);
    cleanup();
    expect(d.unlisten).not.toHaveBeenCalled();
    error.mockRestore();
  });
});

describe("mayCloseWindow", () => {
  const catalog: Catalog = {
    actions: [{ id: "close_all_holes", label: "Close all holes", implemented: true, inScripts: true, needsSelection: false, hint: "", params: [] }],
    scripts: [],
  };
  const result = (mesh: MeshData, changed: boolean): RunResult => ({
    changed,
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
  });
  const questions = () => dialog.message.mock.calls.map((call) => call[0] as string);

  /** Repair mode on a part of a saved project, with a run that changed the working copy. */
  async function repairWithChanges() {
    const s = useStore.getState();
    const [id] = s.addParts([{ name: "Part", mesh: makeBox([20, 20, 20]) }], "Add Part");
    s.markSaved("C:/parts/project.3mf", "project");
    ipc.runRepair.mockImplementationOnce(async (mesh: MeshData) => result(mesh, false));
    enterRepair(id);
    await vi.waitFor(() => expect(useRepair.getState().busy).toBeNull());
    ipc.runRepair.mockResolvedValueOnce(result(makeBox([1, 1, 1]), true));
    await runAction("close_all_holes");
    const session = useRepair.getState().session;
    expect(session && session.mesh !== session.initialMesh).toBe(true);
  }

  beforeEach(() => {
    vi.stubGlobal("window", { setTimeout: () => 0, confirm: () => true });
    vi.stubGlobal("localStorage", undefined);
    vi.stubGlobal("isTauri", true);
    for (const f of [...Object.values(ipc), dialog.message]) f.mockReset();
    ipc.cancelRepair.mockResolvedValue(undefined);
    useRepair.setState({ catalog, session: null, busy: null, autoUpdate: false });
    useStore.getState().newProject();
    useUi.setState({ toasts: [], dialog: null, ribbonTab: "modify", contextMenu: null });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("asks before throwing unapplied repair work away, even in a saved project, and keeps the window open on Cancel", async () => {
    await repairWithChanges();
    dialog.message.mockResolvedValueOnce("Cancel");
    expect(await mayCloseWindow()).toBe(false);
    expect(isRepairing()).toBe(true);
    expect(questions()).toEqual(["Discard repair changes?"]);

    dialog.message.mockResolvedValueOnce("Discard");
    expect(await mayCloseWindow()).toBe(true);
    expect(isRepairing()).toBe(false);
    // The project itself was saved, so nothing else is asked.
    expect(questions()).toEqual(["Discard repair changes?", "Discard repair changes?"]);
  });

  it("asks about the repair first, then about the unsaved project, and stays open when that is cancelled", async () => {
    await repairWithChanges();
    useStore.getState().addTestCube();
    dialog.message.mockResolvedValueOnce("Discard").mockResolvedValueOnce("Cancel");
    expect(await mayCloseWindow()).toBe(false);
    expect(questions()[0]).toBe("Discard repair changes?");
    expect(questions()[1]).toMatch(/^Do you want to save the changes to "project"\?/);
  });

  it("lets the window close at once with nothing to ask about", async () => {
    expect(await mayCloseWindow()).toBe(true);
    expect(dialog.message).not.toHaveBeenCalled();
  });
});
