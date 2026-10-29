import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Rust is a stand-in: every shape is twice as wide as high, a new label fits at 4 mm on a part 20 mm thick, and Apply
// returns the part's own box (list copies one per entry). The Yes/No question and the picture dialog are stand-ins too.
const ipc = vi.hoisted(() => ({ fetchFonts: vi.fn(), labelShape: vi.fn(), fitLabel: vi.fn(), applyLabels: vi.fn(), pickCsv: vi.fn() }));
const repairIpc = vi.hoisted(() => ({ cancelRepair: vi.fn() }));
const libraryIpc = vi.hoisted(() => ({ pickPicture: vi.fn() }));
vi.mock("./ipc", () => ipc);
vi.mock("../library/ipc", async (importOriginal) => ({ ...(await importOriginal<typeof import("../library/ipc")>()), ...libraryIpc }));
vi.mock("../repair/ipc", async (importOriginal) => ({ ...(await importOriginal<typeof import("../repair/ipc")>()), ...repairIpc }));

import { IDENTITY, makeBox } from "../mesh";
import { CANCELLED } from "../repair/ipc";
import { useStore } from "../store";
import { useUi } from "../ui";
import {
  answer,
  applyLabelMode,
  cancelApply,
  closeLabelMode,
  labelClick,
  labelHistory,
  labelKey,
  labelRedo,
  labelUndo,
  labelWidth,
  openLabelMode,
  OVER_EDGE,
  pickLabelPicture,
  placeKind,
  removeLabel,
  selectLabel,
  updateLabel,
} from "./session";
import { useLabel } from "./store";
import type { PlacedLabel } from "./types";

const s = () => useStore.getState();
const l = () => useLabel.getState();
const lastToast = () => useUi.getState().toasts[useUi.getState().toasts.length - 1]?.message;
const stats = { triangles: 12, vertices: 8, volume: 8000, area: 2400, shells: 1, openEdges: 0, nonManifoldEdges: 0 };

function addBox(name = "Box"): string {
  const [id] = s().addParts([{ name, mesh: makeBox([20, 20, 20]), matrix: [...IDENTITY] }]);
  return id;
}

/** Waits for the promises the session started (shapes, the question) to settle. */
const settle = () => new Promise((r) => setTimeout(r, 0));

/** How the user answers the panel's Yes/No question when it shows. */
let reply = true;
let stopAnswering = () => {};

beforeEach(() => {
  vi.stubGlobal("window", { setTimeout: () => 0, confirm: () => true });
  vi.stubGlobal("isTauri", true);
  ipc.fetchFonts.mockResolvedValue([{ id: "bundled", family: "Noto Sans (built in)", style: "Regular", bundled: true }]);
  ipc.labelShape.mockResolvedValue({ aspect: 2, fill: [], outline: [] });
  ipc.fitLabel.mockResolvedValue({ height: 4, fits: true, thickness: 20 });
  ipc.applyLabels.mockImplementation(async (_part: unknown, labels: PlacedLabel[]) => {
    const lists = labels.filter((x) => x.kind === "list");
    const count = lists.length ? Math.min(...lists.map((x) => (x.list.kind === "numbers" ? x.list.count : 0))) : 1;
    const copies = Array.from({ length: count }, (_, i) => ({ name: lists.length ? `00${i + 1}` : "", mesh: makeBox([20, 20, 21]), stats, edges: new Float32Array(0) }));
    return { copies, note: null };
  });
  repairIpc.cancelRepair.mockResolvedValue(undefined);
  reply = true;
  useLabel.setState({ session: null, busy: null, question: null, shapes: {}, fonts: null });
  stopAnswering = useLabel.subscribe((now, before) => {
    if (now.question && !before.question) queueMicrotask(() => answer(reply));
  });
  s().newProject();
  useUi.setState({ toasts: [], dialog: null, ribbonTab: "home" });
});

afterEach(async () => {
  reply = true;
  await closeLabelMode();
  stopAnswering();
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

const hit = (partId: string, point: [number, number, number] = [10, 10, 20]) => ({ partId, point, normal: [0, 0, 1] as [number, number, number] });

describe("label generation", () => {
  it("needs a selected part, opens its tab with Place on, and places a label where the part is clicked", async () => {
    openLabelMode("text");
    expect(l().session).toBeNull();
    expect(lastToast()).toMatch(/Select the part to label first/);
    const id = addBox("Bracket");
    s().select([id], "replace");
    openLabelMode("text");
    expect(l().session).toMatchObject({ partId: id, tool: "place", kind: "text", labels: [] });
    expect(useUi.getState().ribbonTab).toBe("label");
    labelClick(hit(id));
    await settle();
    const [label] = l().session!.labels;
    // Fitted where it went: Rust's height, the depth the smaller of 5 mm and half of it.
    expect(label).toMatchObject({ kind: "text", caption: "Bracket", point: [10, 10, 20], normal: [0, 0, 1], height: 4, depth: 2, fitting: false, keepAspect: true });
    // The next click selects instead of placing another.
    expect(l().session!.tool).toBe("select");
    expect(l().session!.selected).toBe(label.id);
    // Keep aspect ratio: the width follows the shape Rust drew.
    expect(labelWidth(label)).toBe(8);
    labelClick(hit(id, [1, 1, 20]));
    expect(l().session!.selected).toBeNull();
    labelClick(hit(id, [11, 10.5, 20]));
    expect(l().session!.selected).toBe(label.id);
  });

  it("places labels only on the part it opened for", () => {
    const a = addBox("A");
    const b = addBox("B");
    s().select([a], "replace");
    openLabelMode("image");
    labelClick(hit(b));
    expect(l().session!.labels).toHaveLength(0);
    expect(lastToast()).toMatch(/These labels go on A/);
  });

  it("undoes and redoes its own changes, typing a number being one step", async () => {
    const id = addBox();
    s().select([id], "replace");
    const projectSteps = s().history.past.length;
    openLabelMode("text");
    labelClick(hit(id));
    const labelId = l().session!.labels[0].id;
    updateLabel(labelId, { height: 1 }, "Change height");
    updateLabel(labelId, { height: 12 }, "Change height");
    updateLabel(labelId, { caption: "Hello" }, "Change caption");
    expect(labelHistory("undo")).toEqual(["Change caption", "Change height", "Place text label"]);
    labelUndo();
    expect(l().session!.labels[0]).toMatchObject({ caption: "Box", height: 12 });
    labelUndo();
    expect(l().session!.labels[0].height).toBe(10);
    labelRedo();
    expect(l().session!.labels[0].height).toBe(12);
    labelUndo();
    labelUndo();
    expect(l().session!.labels).toHaveLength(0);
    labelRedo();
    expect(l().session!.labels).toHaveLength(1);
    // The project's own history is untouched by label edits.
    expect(s().history.past.length).toBe(projectSteps);
  });

  it("deletes the selected label with Delete and steps back with Esc", () => {
    const id = addBox();
    s().select([id], "replace");
    openLabelMode("tag");
    expect(labelKey("Escape")).toBe(false);
    labelClick(hit(id));
    expect(labelKey("Escape")).toBe(true);
    expect(l().session!.selected).toBeNull();
    selectLabel(l().session!.labels[0].id);
    expect(labelKey("Delete")).toBe(true);
    expect(l().session!.labels).toHaveLength(0);
    placeKind("list");
    expect(l().session).toMatchObject({ tool: "place", kind: "list" });
  });

  it("applies several labels in one run; Yes replaces the original, as one undo step", async () => {
    const id = addBox("Bracket");
    s().select([id], "replace");
    const before = s().history.past.length;
    openLabelMode("text");
    labelClick(hit(id));
    placeKind("image");
    labelClick(hit(id, [5, 5, 20]));
    await settle();
    expect(l().session!.labels).toHaveLength(2);
    reply = true;
    await applyLabelMode();
    expect(ipc.applyLabels).toHaveBeenCalledTimes(1);
    expect(ipc.applyLabels.mock.calls[0][1]).toHaveLength(2);
    expect(ipc.applyLabels.mock.calls[0][2]).toEqual([8, 8]);
    expect(l().session).toBeNull();
    expect(s().parts.map((p) => p.name)).toEqual(["Bracket labelled"]);
    expect(s().history.past.length).toBe(before + 1);
    expect(s().history.past[s().history.past.length - 1].label).toBe("Generate labels");
    s().undo();
    expect(s().parts.map((p) => p.name)).toEqual(["Bracket"]);
    s().redo();
    expect(s().parts.map((p) => p.name)).toEqual(["Bracket labelled"]);
  });

  it("No keeps both parts", async () => {
    const id = addBox("Bracket");
    s().select([id], "replace");
    openLabelMode("text");
    labelClick(hit(id));
    await settle();
    reply = false;
    await applyLabelMode();
    expect(s().parts.map((p) => p.name)).toEqual(["Bracket", "Bracket labelled"]);
  });

  it("lays list copies side by side, each named after its entry", async () => {
    const id = addBox("Bar");
    s().select([id], "replace");
    openLabelMode("list");
    labelClick(hit(id));
    await settle();
    const labelId = l().session!.labels[0].id;
    updateLabel(labelId, { list: { kind: "numbers", start: 1, step: 1, count: 3, prefix: "", suffix: "", digits: 3 } });
    reply = false;
    await applyLabelMode();
    expect(s().parts.map((p) => p.name)).toEqual(["Bar", "Bar 001", "Bar 002", "Bar 003"]);
    // Kept, the original stays where it was and the copies start one step along X (20 mm wide + 5 mm).
    expect(s().parts.slice(1).map((p) => p.matrix[12])).toEqual([25, 50, 75]);
  });

  it("says why Apply failed and keeps the labels; Cancel stops it and changes nothing", async () => {
    const id = addBox();
    s().select([id], "replace");
    openLabelMode("text");
    labelClick(hit(id));
    await settle();
    ipc.applyLabels.mockRejectedValueOnce("The label reaches past the edge of the part's surface. Make it smaller or move it.");
    await applyLabelMode();
    expect(lastToast()).toMatch(/reaches past the edge/);
    expect(l().session!.labels).toHaveLength(1);
    let release: (v: unknown) => void = () => {};
    ipc.applyLabels.mockImplementationOnce(() => new Promise((_, reject) => (release = reject)));
    const running = applyLabelMode();
    await settle();
    expect(l().busy).not.toBeNull();
    cancelApply();
    expect(repairIpc.cancelRepair).toHaveBeenCalled();
    release(CANCELLED);
    await running;
    expect(s().parts).toHaveLength(1);
    expect(l().busy).toBeNull();
  });

  it("fits every new label fresh where it is placed, whatever the one before was sized to", async () => {
    const id = addBox();
    s().select([id], "replace");
    openLabelMode("text");
    labelClick(hit(id));
    await settle();
    const first = l().session!.labels[0].id;
    updateLabel(first, { height: 2.5 }, "Change height");
    expect(ipc.fitLabel.mock.calls[0][2]).toEqual({ point: [10, 10, 20], normal: [0, 0, 1], angle: 0 });
    ipc.fitLabel.mockResolvedValueOnce({ height: 7.256, fits: true, thickness: 20 });
    placeKind("text");
    labelClick(hit(id, [5, 5, 20]));
    // Not drawn, and not applied, until it is fitted.
    expect(l().session!.labels[1].fitting).toBe(true);
    await settle();
    expect(l().session!.labels.map((x) => x.height)).toEqual([2.5, 7.25]);
    expect(l().session!.labels[1].depth).toBe(3.62);
  });

  it("starts at 1 mm and says it runs over an edge where nothing fits", async () => {
    ipc.fitLabel.mockResolvedValueOnce({ height: 1, fits: false, thickness: 0.8 });
    const id = addBox();
    s().select([id], "replace");
    openLabelMode("tag");
    labelClick(hit(id));
    await settle();
    // A tag's caption starts at 30 % of its 2 mm plate, not by the label's height.
    expect(l().session!.labels[0]).toMatchObject({ height: 1, depth: 0.6 });
    expect(lastToast()).toBe(OVER_EDGE);
  });

  it("starts the depth at the smaller of 5 mm and half the height, sunk at most 40 % of the part, until the user sets it", async () => {
    ipc.fitLabel.mockResolvedValueOnce({ height: 10, fits: true, thickness: 6 });
    const id = addBox();
    s().select([id], "replace");
    openLabelMode("text");
    labelClick(hit(id));
    await settle();
    const labelId = l().session!.labels[0].id;
    const label = () => l().session!.labels[0];
    expect(label().depth).toBe(5);
    updateLabel(labelId, { build: "subtract" }, "Change build type");
    expect(label().depth).toBe(2.4);
    updateLabel(labelId, { height: 3 }, "Change height");
    expect(label().depth).toBe(1.5);
    updateLabel(labelId, { build: "add" }, "Change build type");
    expect(label().depth).toBe(1.5);
    // Typed, it stays.
    updateLabel(labelId, { depth: 4 }, "Change depth");
    updateLabel(labelId, { build: "subtract-mirrored", height: 8 }, "Change build type");
    expect(label().depth).toBe(4);
    // A tag's caption: 30 % of its plate's thickness, raised or sunk; it follows the plate until typed.
    placeKind("tag");
    ipc.fitLabel.mockResolvedValueOnce({ height: 6, fits: true, thickness: 20 });
    labelClick(hit(id, [4, 4, 20]));
    await settle();
    const tag = () => l().session!.labels[1];
    expect(tag().depth).toBe(0.6);
    updateLabel(tag().id, { build: "subtract" }, "Change variant");
    expect(tag().depth).toBe(0.6);
    updateLabel(tag().id, { thickness: 4 }, "Change thickness");
    expect(tag().depth).toBe(1.2);
    updateLabel(tag().id, { depth: 0.3 }, "Change depth");
    updateLabel(tag().id, { thickness: 3, build: "add" }, "Change thickness");
    expect(tag().depth).toBe(0.3);
  });

  it("keeps the height when the caption changes, the width following the letters", async () => {
    ipc.labelShape.mockImplementation(async (request: { text: string }) => ({ aspect: request.text.length, fill: [], outline: [] }));
    const id = addBox("Box");
    s().select([id], "replace");
    openLabelMode("text");
    labelClick(hit(id));
    await settle();
    const labelId = l().session!.labels[0].id;
    expect(labelWidth(l().session!.labels[0])).toBe(12);
    updateLabel(labelId, { caption: "Bracket" }, "Change caption");
    await settle();
    expect(l().session!.labels[0].height).toBe(4);
    expect(labelWidth(l().session!.labels[0])).toBe(28);
  });

  it("fits an image label again when its picture comes, unless the user sized it first", async () => {
    const picture = { name: "logo.png", width: 4, height: 2, gray: new Float32Array(8) };
    libraryIpc.pickPicture.mockResolvedValue(picture);
    const id = addBox();
    s().select([id], "replace");
    openLabelMode("image");
    labelClick(hit(id));
    await settle();
    const labelId = l().session!.labels[0].id;
    ipc.fitLabel.mockResolvedValueOnce({ height: 5, fits: true, thickness: 20 });
    await pickLabelPicture(labelId);
    expect(ipc.fitLabel.mock.calls[1][3]).toBe(picture);
    expect(l().session!.labels[0].height).toBe(5);
    updateLabel(labelId, { height: 3 }, "Change height");
    ipc.fitLabel.mockResolvedValueOnce({ height: 9, fits: true, thickness: 20 });
    await pickLabelPicture(labelId);
    expect(ipc.fitLabel).toHaveBeenCalledTimes(2);
    expect(l().session!.labels[0].height).toBe(3);
  });

  it("sizes a list label by all its entries", async () => {
    const id = addBox();
    s().select([id], "replace");
    openLabelMode("list");
    labelClick(hit(id));
    await settle();
    const request = ipc.fitLabel.mock.calls[0][1];
    expect(request).toMatchObject({ kind: "list", entries: ["001", "002", "003", "004", "005"] });
    expect(ipc.labelShape.mock.calls[0][0].entries).toHaveLength(5);
  });

  it("asks before leaving with labels placed, and ends when the part goes away", async () => {
    const id = addBox();
    s().select([id], "replace");
    openLabelMode("text");
    labelClick(hit(id));
    reply = false;
    await closeLabelMode();
    expect(l().session).not.toBeNull();
    removeLabel(l().session!.labels[0].id);
    await closeLabelMode();
    expect(l().session).toBeNull();
    s().select([id], "replace");
    openLabelMode("text");
    s().deleteParts([id]);
    expect(l().session).toBeNull();
  });
});
