import { beforeEach, describe, expect, it, vi } from "vitest";

// Rust is a stand-in that answers the way the test says; everything else runs as in the app.
const ipc = vi.hoisted(() => ({ runModify: vi.fn() }));
const repairIpc = vi.hoisted(() => ({ cancelRepair: vi.fn(), runRepair: vi.fn() }));
vi.mock("./ipc", () => ipc);
vi.mock("../repair/ipc", async (importOriginal) => ({ ...(await importOriginal<typeof import("../repair/ipc")>()), ...repairIpc }));

import { makeBox, translation } from "../mesh";
import { CANCELLED } from "../repair/ipc";
import { idsToMask, maskToIds } from "../repair/selection";
import { enterRepair } from "../repair/session";
import { useRepair } from "../repair/store";
import { useStore, type MeshData } from "../store";
import { useUi } from "../ui";
import {
  applyManipulate,
  calculate,
  closeManipulate,
  editSettings,
  invertParts,
  LOCKED_PART,
  ONE_PART_ONLY,
  openManipulate,
  PICK_ONE_PART,
  previewIsCurrent,
  previewPart,
  setShowOriginal,
  setTab,
  toolOf,
  useModify,
} from "./session";
import { DEFAULT_SETTINGS } from "./types";

const s = () => useStore.getState();
const m = () => useModify.getState();
const lastToast = () => useUi.getState().toasts[useUi.getState().toasts.length - 1]?.message;
const labels = () => s().history.past.map((e) => e.label);
const flush = () => new Promise((r) => setTimeout(r, 0));

/** A Rust answer: `mesh` as the result. */
const answer = (mesh: MeshData, extra: Partial<{ origin: Uint32Array; message: string; before: number }> = {}) => ({
  changed: true,
  mesh,
  origin: extra.origin ?? null,
  message: extra.message ?? "Reduced 12 triangles to 8.",
  warnings: [],
  before: extra.before ?? 12,
  after: mesh.indices.length / 3,
});

const small = makeBox([10, 10, 10]);

beforeEach(() => {
  vi.stubGlobal("isTauri", true);
  vi.stubGlobal("window", { setTimeout: (f: () => void) => setTimeout(f, 0), clearTimeout: () => {} });
  ipc.runModify.mockReset();
  repairIpc.cancelRepair.mockReset();
  repairIpc.cancelRepair.mockResolvedValue(undefined);
  repairIpc.runRepair.mockReset();
  closeManipulate();
  useRepair.setState({ session: null, busy: null });
  s().newProject();
  useUi.setState({ toasts: [], dialog: null, tasks: [] });
});

function onePart(): string {
  const [id] = s().addParts([{ name: "Cube", mesh: makeBox([20, 20, 20]), matrix: translation(100, 100, 10), color: "#123456" }]);
  s().select([id]);
  return id;
}

describe("Manipulate mesh", () => {
  it("opens on one selected, unlocked part, with half its triangles as the target", () => {
    openManipulate("reduce");
    expect(lastToast()).toBe(PICK_ONE_PART);
    const id = onePart();
    const [other] = s().addParts([{ name: "Other", mesh: makeBox([5, 5, 5]) }]);
    s().select([id, other]);
    openManipulate("reduce");
    expect(lastToast()).toBe(ONE_PART_ONLY);
    s().select([id]);
    s().setPartLocked(id, true);
    openManipulate("reduce");
    expect(lastToast()).toBe(LOCKED_PART);
    s().setPartLocked(id, false);
    openManipulate("reduce");
    const open = m().session!;
    expect(open.target).toEqual({ kind: "part", partId: id });
    expect(open.tab).toBe("reduce");
    // The working copy is the part baked into world space.
    expect(Array.from(open.base.positions.slice(0, 3))).toEqual([90, 90, 0]);
    expect(m().settings.reduce.target).toBe(6);
    // Opening again only switches tabs.
    openManipulate("smooth");
    expect(m().session?.tab).toBe("smooth");
    expect(m().session?.base).toBe(open.base);
  });

  it("shows a calculated result in place of the part, the original while Show original is held, and OK puts it in as one undo step", async () => {
    const id = onePart();
    openManipulate("reduce");
    ipc.runModify.mockResolvedValue(answer(small));
    expect(await calculate()).toBe(true);
    const [mesh, tool, selection] = ipc.runModify.mock.calls[0];
    expect(mesh).toBe(m().session?.base);
    expect(tool).toMatchObject({ tool: "reduce", target: 6, selectedOnly: false });
    expect(selection).toEqual(new Uint32Array(0));
    expect(previewPart()?.mesh).toBe(small);
    expect(previewPart()?.color).toBe("#123456");
    setShowOriginal(true);
    expect(previewPart()).toBeNull();
    setShowOriginal(false);
    expect(previewIsCurrent()).toBe(true);
    // Nothing changed in the project yet.
    expect(labels()).toEqual(["Add parts"]);
    expect(await applyManipulate()).toBe(true);
    expect(ipc.runModify).toHaveBeenCalledTimes(1);
    expect(labels()).toEqual(["Add parts", "Triangle reduction"]);
    const part = s().parts[0];
    expect(part.id).not.toBe(id);
    expect([part.name, part.color, part.mesh]).toEqual(["Cube", "#123456", small]);
    expect(part.matrix).toEqual([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]);
    expect(s().selection).toEqual([part.id]);
    expect(m().session).toBeNull();
    expect(lastToast()).toBe("Reduced 12 triangles to 8.");
    s().undo();
    expect(s().parts[0].id).toBe(id);
  });

  it("calculates again on OK when the settings changed since", async () => {
    onePart();
    openManipulate("reduce");
    ipc.runModify.mockResolvedValue(answer(small));
    await calculate();
    editSettings("reduce", { maxDeviation: 0.2 });
    expect(previewIsCurrent()).toBe(false);
    await applyManipulate();
    expect(ipc.runModify).toHaveBeenCalledTimes(2);
    expect(ipc.runModify.mock.calls[1][1]).toMatchObject({ maxDeviation: 0.2 });
  });

  it("leaves everything as it was when cancelled or refused", async () => {
    onePart();
    openManipulate("smooth");
    ipc.runModify.mockRejectedValue(CANCELLED);
    expect(await calculate()).toBe(false);
    expect(lastToast()).toBe("Smoothing was cancelled. Nothing changed.");
    expect(m().session?.preview).toBeNull();
    ipc.runModify.mockRejectedValue("Smoothing would make triangles cross each other with these settings, so the part was left as it was.");
    expect(await applyManipulate()).toBe(false);
    expect(lastToast()).toContain("would make triangles cross");
    expect(labels()).toEqual(["Add parts"]);
    expect(m().busy).toBeNull();
  });

  it("works on the working copy in repair mode, with its selected triangles, as one inner undo step", async () => {
    const id = onePart();
    repairIpc.runRepair.mockImplementation(async (mesh: MeshData) => ({
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
    }));
    enterRepair(id);
    await flush();
    await flush();
    const working = useRepair.getState().session!;
    useRepair.setState({ session: { ...working, selection: idsToMask([2, 3], 12), selectedCount: 2 } });
    openManipulate("refine");
    expect(m().session?.target).toEqual({ kind: "repair", partId: id });
    expect(m().session?.base).toBe(working.mesh);
    editSettings("refine", { selectedOnly: true });
    // The result: triangles 0..11 kept, two new ones split off triangle 3.
    const finer: MeshData = { positions: working.mesh.positions, indices: new Uint32Array([...working.mesh.indices, 0, 1, 2, 0, 2, 3]) };
    ipc.runModify.mockResolvedValue(answer(finer, { origin: Uint32Array.from([...Array(12).keys(), 3, 3]) }));
    expect(await applyManipulate()).toBe(true);
    expect(Array.from(ipc.runModify.mock.calls[0][2])).toEqual([2, 3]);
    expect(ipc.runModify.mock.calls[0][1]).toMatchObject({ tool: "refine", selectedOnly: true });
    const after = useRepair.getState().session!;
    expect(after.mesh).toBe(finer);
    expect(after.past.map((p) => p.label)).toEqual(["Refine"]);
    // The selection follows its triangles: 2, 3 and the two split off 3.
    expect(Array.from(maskToIds(after.selection))).toEqual([2, 3, 12, 13]);
    // The project itself is untouched until Apply repair.
    expect(labels()).toEqual(["Add parts"]);
  });

  it("inverts the selected parts at once, all in one undo step", async () => {
    const id = onePart();
    const [other] = s().addParts([{ name: "Other", mesh: makeBox([5, 5, 5]) }]);
    s().select([id, other]);
    ipc.runModify.mockImplementation(async (mesh: MeshData) => answer(mesh, { message: "Turned the part inside out (12 triangles)." }));
    await invertParts("invert");
    expect(ipc.runModify).toHaveBeenCalledTimes(2);
    expect(ipc.runModify.mock.calls[0][1]).toEqual({ tool: "invert" });
    expect(labels()).toEqual(["Add parts", "Add parts", "Invert"]);
    expect(s().parts).toHaveLength(2);
    expect(lastToast()).toBe("Invert: 2 parts changed.");
  });
});

describe("Part C tools", () => {
  it("send their settings as Rust reads them, with Selected triangles only only where it applies", () => {
    const remesh = { ...DEFAULT_SETTINGS, remesh: { ...DEFAULT_SETTINGS.remesh, method: "volumetric" as const, selectedOnly: true, slimColored: true } };
    // Show slim faces only colours the view; Volumetric always rebuilds the whole part.
    expect(toolOf("remesh", remesh, true)).toEqual({
      tool: "remesh",
      method: "volumetric",
      edge: 1,
      largeEdge: 4,
      smoothing: 0,
      keepEdges: true,
      edgeAngle: 40,
      fast: false,
      selectedOnly: false,
    });
    const uniform = { ...remesh, remesh: { ...remesh.remesh, method: "uniform" as const } };
    expect(toolOf("remesh", uniform, true)).toMatchObject({ method: "uniform", selectedOnly: true });
    expect(toolOf("remesh", uniform, false)).toMatchObject({ selectedOnly: false });
    expect(toolOf("zcomp", DEFAULT_SETTINGS, false)).toEqual({ tool: "z-compensation", distance: 0.1, angle: 30 });
    expect(toolOf("extrude", DEFAULT_SETTINGS, true)).toEqual({ tool: "extrude", distance: 1, direction: "normal", walls: true });
    expect(toolOf("offset", DEFAULT_SETTINGS, true)).toEqual({ tool: "offset-selection", distance: 0.5, transition: 0 });
    const holes = { ...DEFAULT_SETTINGS, holes: { distance: 2, selectedOnly: true } };
    expect(toolOf("holes", holes, false)).toEqual({ tool: "offset-holes", distance: 2, selectedOnly: false });
  });

  it("opens repair mode on the part for Extrude, whose triangles are picked there", () => {
    const id = onePart();
    repairIpc.runRepair.mockImplementation(async (mesh: MeshData) => ({
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
    }));
    openManipulate("extrude");
    expect(useRepair.getState().session?.partId).toBe(id);
    expect(m().session?.target).toEqual({ kind: "repair", partId: id });
    expect(m().session?.tab).toBe("extrude");
  });

  it("shows the part as it is, coloured, while Show slim faces is on in the Remesh tab", () => {
    onePart();
    openManipulate("remesh");
    expect(previewPart()).toBeNull();
    editSettings("remesh", { slimColored: true });
    expect(previewPart()?.mesh).toBe(m().session?.base);
    setTab("smooth");
    expect(previewPart()).toBeNull();
  });
});

describe("closing", () => {
  it("closes the panel when the project is replaced or repair mode closes under it", () => {
    onePart();
    openManipulate("reduce");
    s().newProject();
    expect(m().session).toBeNull();
    const id = onePart();
    useRepair.setState({ session: { partId: id, mesh: makeBox([5, 5, 5]), selection: new Uint8Array(12), selectedCount: 0 } as never });
    openManipulate("smooth");
    expect(m().session?.target.kind).toBe("repair");
    useRepair.setState({ session: null });
    expect(m().session).toBeNull();
  });
});
