import { beforeEach, describe, expect, it, vi } from "vitest";

// Rust is a stand-in that answers the way the test says; everything else runs as in the app.
const ipc = vi.hoisted(() => ({ previewHollow: vi.fn(), placeHoles: vi.fn(), applyHollow: vi.fn(), forgetHollow: vi.fn() }));
const repairIpc = vi.hoisted(() => ({ cancelRepair: vi.fn() }));
vi.mock("./ipc", () => ipc);
vi.mock("../repair/ipc", async (importOriginal) => ({ ...(await importOriginal<typeof import("../repair/ipc")>()), ...repairIpc }));

import { useClip } from "../clip";
import { makeBox, translation } from "../mesh";
import { CANCELLED } from "../repair/ipc";
import { useStore, type MeshData } from "../store";
import { useUi } from "../ui";
import {
  addHoleAt,
  applyHollowNow,
  closeHollow,
  deleteSelectedHole,
  editOptions,
  editSettings,
  ONE_PART_ONLY,
  openHollow,
  PICK_ONE_PART,
  PREVIEW_DELAY,
  useHollow,
  volumesText,
} from "./session";
import type { Hole } from "./types";

const s = () => useStore.getState();
const h = () => useHollow.getState();
const lastToast = () => useUi.getState().toasts[useUi.getState().toasts.length - 1]?.message;
const flush = () => new Promise((r) => setTimeout(r, 0));

const cavity = makeBox([36, 36, 36]);
const made = (key: number) => ({ kind: "made", key, cavity, partVolume: 64000, cavityVolume: 46656, thin: null, warnings: [] });
/** Every hole checked as fine, each with a small solid to draw. */
const fine = (_key: number, holes: Hole[]) => ({
  holes,
  checks: holes.map(() => ({ ok: true, reason: null, depth: 2, room: 36 })),
  solids: holes.map(() => makeBox([1, 1, 1])),
});

beforeEach(() => {
  vi.useRealTimers();
  vi.stubGlobal("isTauri", true);
  vi.stubGlobal("window", { setTimeout: (f: () => void) => setTimeout(f, 0), clearTimeout: () => {} });
  for (const f of Object.values(ipc)) f.mockReset();
  ipc.forgetHollow.mockResolvedValue(undefined);
  ipc.placeHoles.mockImplementation(async (key: number, holes: Hole[]) => fine(key, holes));
  repairIpc.cancelRepair.mockReset();
  repairIpc.cancelRepair.mockResolvedValue(undefined);
  closeHollow();
  useClip.getState().reset();
  s().newProject();
  useUi.setState({ toasts: [], dialog: null, tasks: [] });
  editOptions({ removeOriginal: false, plugs: false, section: true });
});

function onePart(): string {
  const [id] = s().addParts([{ name: "Cube", mesh: makeBox([40, 40, 40]), matrix: translation(100, 100, 20), color: "#123456" }]);
  s().select([id]);
  return id;
}

describe("Hollow", () => {
  it("opens on one selected part, works out the cavity and cuts the view through the middle", async () => {
    openHollow();
    expect(lastToast()).toBe(PICK_ONE_PART);
    const id = onePart();
    const [other] = s().addParts([{ name: "Other", mesh: makeBox([5, 5, 5]) }]);
    s().select([id, other]);
    openHollow();
    expect(lastToast()).toBe(ONE_PART_ONLY);
    s().select([id]);
    ipc.previewHollow.mockImplementation(async (_mesh: MeshData, _s: unknown, _job: number, key: number) => made(key));
    openHollow();
    expect(h().session?.partId).toBe(id);
    // The part in world space, as Rust works on it.
    const [base, settings] = ipc.previewHollow.mock.calls[0];
    expect(Math.min(...Array.from(base.positions as Float32Array).filter((_, i) => i % 3 === 0))).toBe(80);
    expect(settings).toEqual(h().options.settings);
    await flush();
    expect(h().session?.preview?.cavity).toBe(cavity);
    expect(volumesText(h().session!.preview!)).toBe("64.0 cm³ → 17.3 cm³; resin saved 46.7 cm³ (73 %)");
    // The section: the Y plane through the part's middle, given back on Close.
    expect(useClip.getState().rows.y).toMatchObject({ enabled: true, offset: 100, flipped: true });
    closeHollow();
    expect(useClip.getState().rows.y).toMatchObject({ enabled: false, flipped: false });
    expect(ipc.forgetHollow).toHaveBeenCalled();
  });

  it("cancels the cavity being worked out when the thickness changes, and works it out again once it settles", async () => {
    onePart();
    let firstJob = 0;
    ipc.previewHollow.mockImplementationOnce(
      (_mesh: MeshData, _s: unknown, job: number) =>
        new Promise((_, reject) => {
          firstJob = job;
          repairIpc.cancelRepair.mockImplementation(async (j: number) => {
            if (j === job) reject(CANCELLED);
          });
        }),
    );
    ipc.previewHollow.mockImplementation(async (_mesh: MeshData, _s: unknown, _job: number, key: number) => made(key));
    openHollow();
    expect(h().session?.computing?.job).toBe(firstJob);
    editSettings({ thickness: 3 });
    expect(repairIpc.cancelRepair).toHaveBeenCalledWith(firstJob);
    expect(h().session?.computing).toBeNull();
    await new Promise((r) => setTimeout(r, PREVIEW_DELAY + 50));
    await flush();
    expect(ipc.previewHollow).toHaveBeenCalledTimes(2);
    expect(ipc.previewHollow.mock.calls[1][1]).toMatchObject({ thickness: 3 });
    expect(h().session?.preview?.cavity).toBe(cavity);
    expect(h().session?.failed).toBeNull();
  });

  it("adds a hole where the part's outside is clicked, checks it, and Delete removes the selected one", async () => {
    const id = onePart();
    ipc.previewHollow.mockImplementation(async (_mesh: MeshData, _s: unknown, _job: number, key: number) => made(key));
    openHollow();
    await flush();
    addHoleAt({ partId: "someone-else", point: [0, 0, 0], normal: [0, 0, -1] });
    expect(h().session?.holes).toHaveLength(0);
    addHoleAt({ partId: id, point: [100, 100, 0], normal: [0, 0, -1] });
    await flush();
    const [hole] = h().session!.holes;
    expect(hole.hole).toEqual({ at: [100, 100, 0], normal: [0, 0, -1], ...h().options.shape });
    expect(hole.check?.ok).toBe(true);
    expect(h().session?.selected).toBe(hole.id);
    expect(deleteSelectedHole()).toBe(true);
    expect(h().session?.holes).toHaveLength(0);
    expect(deleteSelectedHole()).toBe(false);
  });

  it("applies as one undo step named after the part, keeps the original unless asked, and undo brings it back", async () => {
    const id = onePart();
    ipc.previewHollow.mockImplementation(async (_mesh: MeshData, _s: unknown, _job: number, key: number) => made(key));
    const result = makeBox([39, 39, 39]);
    const plug = makeBox([2, 2, 2]);
    ipc.applyHollow.mockResolvedValue({
      mesh: { mesh: result, stats: null, edges: new Float32Array() },
      plugs: [{ mesh: plug, stats: null, edges: new Float32Array() }],
      skipped: [],
      warnings: [],
    });
    editOptions({ plugs: true, clearance: 0.2 });
    openHollow();
    await flush();
    addHoleAt({ partId: id, point: [100, 100, 0], normal: [0, 0, -1] });
    await flush();
    expect(await applyHollowNow()).toBe(true);
    const [job, key, holes, plugs] = ipc.applyHollow.mock.calls[0];
    expect(typeof job).toBe("number");
    expect(key).toBe(h().session?.preview?.key ?? key);
    expect(holes).toHaveLength(1);
    expect(plugs).toBe(0.2);
    expect(s().parts.map((p) => p.name)).toEqual(["Cube", "Cube (hollow)", "Cube (plug)"]);
    const hollow = s().parts[1];
    expect(hollow.mesh).toBe(result);
    expect(hollow.color).toBe("#123456");
    expect(s().history.past.map((e) => e.label)).toEqual(["Add parts", "Hollow"]);
    expect(h().session).toBeNull();
    s().undo();
    expect(s().parts.map((p) => p.id)).toEqual([id]);
    // Remove original: the hollow part takes its place.
    editOptions({ removeOriginal: true, plugs: false });
    s().select([id]);
    openHollow();
    await flush();
    ipc.applyHollow.mockResolvedValue({ mesh: { mesh: result, stats: null, edges: new Float32Array() }, plugs: [], skipped: [{ index: 0, reason: "No cavity." }], warnings: [] });
    expect(await applyHollowNow()).toBe(true);
    expect(s().parts.map((p) => p.name)).toEqual(["Cube (hollow)"]);
    s().undo();
    expect(s().parts.map((p) => p.id)).toEqual([id]);
  });

  it("changes nothing on Close or a cancelled Apply", async () => {
    const id = onePart();
    ipc.previewHollow.mockImplementation(async (_mesh: MeshData, _s: unknown, _job: number, key: number) => made(key));
    openHollow();
    await flush();
    ipc.applyHollow.mockRejectedValue(CANCELLED);
    expect(await applyHollowNow()).toBe(false);
    expect(lastToast()).toBe("Hollow was cancelled. Nothing changed.");
    closeHollow();
    expect(s().parts.map((p) => p.id)).toEqual([id]);
    expect(s().history.past.map((e) => e.label)).toEqual(["Add parts"]);
  });
});
