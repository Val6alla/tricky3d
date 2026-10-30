import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { makeBox, translation } from "./mesh";
import { GIZMO_TOOLS, isDirty, SELECT_TOOL, shownGizmo, useStore, type GizmoMode } from "./store";

const s = () => useStore.getState();
const x = (id: string) => s().parts.find((p) => p.id === id)?.matrix[12];
const labels = () => s().history.past.map((e) => e.label);

beforeEach(() => {
  s().newProject();
  s().setPlatform([200, 200, 200]);
  s().markSaved("C:\\p\\a.3mf", "a");
});

describe("live previews", () => {
  it("become one step when the drag records them", () => {
    s().addTestCube();
    const id = s().parts[0].id;
    const before = s().snapshot();
    s().setPartMatrices(new Map([[id, translation(101, 100, 10)]]), null);
    s().setPartMatrices(new Map([[id, translation(104, 100, 10)]]), null);
    s().recordChange("Move", before);
    expect(labels()).toEqual(["Add test cube", "Move"]);
    s().undo();
    expect(x(id)).toBe(100);
  });

  it("become their own step when another edit lands mid-drag", () => {
    s().addTestCube();
    const id = s().parts[0].id;
    s().setPartMatrices(new Map([[id, translation(104, 100, 10)]]), null);
    // An import finishing while the user still drags.
    s().addParts([{ name: "other", mesh: makeBox([1, 1, 1]) }], "Import other.stl");
    expect(labels()).toEqual(["Add test cube", "Transform", "Import other.stl"]);
    s().undo();
    expect(s().parts.map((p) => p.name)).toEqual(["Test cube"]);
    expect(x(id)).toBe(104);
    s().undo();
    expect(x(id)).toBe(100);
    s().redo();
    s().redo();
    expect(s().parts).toHaveLength(2);
    expect(x(id)).toBe(104);
  });

  it("leave redo alone when redo is pressed mid-drag", () => {
    s().addTestCube();
    const id = s().parts[0].id;
    s().setPartMatrices(new Map([[id, translation(104, 100, 10)]]), "Move");
    s().undo();
    s().setPartMatrices(new Map([[id, translation(90, 100, 10)]]), null);
    s().redo();
    expect(s().history.future.map((e) => e.label)).toEqual(["Move"]);
    expect(x(id)).toBe(90);
  });

  it("are undone by an undo pressed mid-drag", () => {
    s().addTestCube();
    const id = s().parts[0].id;
    s().setPartMatrices(new Map([[id, translation(104, 100, 10)]]), null);
    s().undo();
    expect(x(id)).toBe(100);
    expect(labels()).toEqual(["Add test cube"]);
  });
});

describe("replaceParts", () => {
  const add = (...names: string[]) => s().addParts(names.map((name) => ({ name, mesh: makeBox([1, 1, 1]) })));

  const box = (name: string) => ({ name, mesh: makeBox([2, 2, 2]) });

  it("puts the new parts where the first removed one was, selects them, and undoes in one step", () => {
    const [a, b, c] = add("a", "b", "c");
    const before = s().parts;
    const ids = s().replaceParts([{ remove: [c, a], add: [box("x"), box("y")] }], "Merge");
    expect(s().parts.map((p) => p.name)).toEqual(["x", "y", "b"]);
    expect(s().selection).toEqual(ids);
    expect(labels()[labels().length - 1]).toBe("Merge");
    s().undo();
    expect(s().parts).toBe(before);
    expect(s().parts.map((p) => p.id)).toEqual([a, b, c]);
    s().redo();
    expect(s().parts.map((p) => p.id)).toEqual([ids[0], ids[1], b]);
  });

  it("keeps each part's pieces in that part's place, around the parts in between", () => {
    const [a, , c, , e] = add("a", "b", "c", "d", "e");
    s().replaceParts(
      [
        { remove: [e], add: [box("e1"), box("e2")] },
        { remove: [a], add: [box("a1"), box("a2")] },
        { remove: [c], add: [] },
      ],
      "Cut",
    );
    expect(s().parts.map((p) => p.name)).toEqual(["a1", "a2", "b", "d", "e1", "e2"]);
    expect(s().selection).toHaveLength(4);
    expect(labels().filter((l) => l === "Cut")).toHaveLength(1);
  });

  it("adds at the end when nothing is removed, and records nothing when nothing changes", () => {
    add("a");
    const ids = s().replaceParts([{ remove: [], add: [box("u")] }], "Union");
    expect(s().parts.map((p) => p.name)).toEqual(["a", "u"]);
    expect(s().selection).toEqual(ids);
    const steps = labels().length;
    expect(s().replaceParts([{ remove: ["no-such-part"], add: [] }], "Cut")).toEqual([]);
    expect(labels()).toHaveLength(steps);
  });

  const byId = (id: string) => s().parts.find((p) => p.id === id);

  it("puts the new parts right after `after`, and patches parts that stay in the same undo step (Apply repair › compare)", () => {
    const [a, b, c] = add("a", "b", "c");
    const before = s().parts;
    const steps = labels().length;
    const ids = s().replaceParts([{ remove: [], add: [box("a (repaired)")], after: a }], "Repair", {
      ids: [a],
      patch: { transparent: true, visible: true },
    });
    expect(s().parts.map((p) => p.name)).toEqual(["a", "a (repaired)", "b", "c"]);
    expect(byId(a)?.transparent).toBe(true);
    expect(byId(b)).toBe(before[1]);
    expect(s().selection).toEqual(ids);
    expect(labels()).toHaveLength(steps + 1);
    expect(labels()[labels().length - 1]).toBe("Repair");
    s().undo();
    expect(s().parts).toBe(before);
    expect(byId(a)?.transparent).toBe(false);
    s().redo();
    expect(s().parts.map((p) => p.id)).toEqual([a, ids[0], b, c]);
    expect(byId(a)?.transparent).toBe(true);

    // After the last part, the new parts end the list.
    s().replaceParts([{ remove: [], add: [box("c (repaired)")], after: c }], "Repair");
    expect(s().parts.map((p) => p.name)).toEqual(["a", "a (repaired)", "b", "c", "c (repaired)"]);
  });

  it("fills the place of an `after` part that is removed too, and goes to the end for an unknown one", () => {
    const [a, b] = add("a", "b");
    s().replaceParts(
      [
        { remove: [b], add: [box("b2")] },
        { remove: [], add: [box("x")], after: a },
      ],
      "Mixed",
    );
    expect(s().parts.map((p) => p.name)).toEqual(["a", "x", "b2"]);
    s().replaceParts([{ remove: [a], add: [] }, { remove: [], add: [box("y")], after: a }], "Swap");
    expect(s().parts.map((p) => p.name)).toEqual(["y", "x", "b2"]);
    s().replaceParts([{ remove: [], add: [box("z")], after: "no-such-part" }], "Add");
    expect(s().parts.map((p) => p.name)).toEqual(["y", "x", "b2", "z"]);
  });

  it("records a patch that changes something as a step, and nothing for a patch that changes nothing or names removed parts", () => {
    const [a, b] = add("a", "b");
    const steps = labels().length;
    expect(s().replaceParts([], "Patch", { ids: [a], patch: { transparent: false } })).toEqual([]);
    expect(labels()).toHaveLength(steps);
    s().replaceParts([{ remove: [b], add: [] }], "Remove b", { ids: [b], patch: { name: "ghost" } });
    expect(s().parts.map((p) => p.name)).toEqual(["a"]);
    expect(labels()).toHaveLength(steps + 1);
    s().replaceParts([], "Patch", { ids: [a], patch: { locked: true } });
    expect(byId(a)?.locked).toBe(true);
    expect(labels()).toHaveLength(steps + 2);
  });
});

describe("platform and dirty state", () => {
  it("keeps the project clean when the same size is set again", () => {
    s().setPlatform([200, 200, 200]);
    expect(isDirty(s())).toBe(false);
  });

  it("is clean again when the size goes back to the saved one", () => {
    s().setPlatform([180, 200, 200]);
    expect(isDirty(s())).toBe(true);
    s().setPlatform([200, 200, 200]);
    expect(isDirty(s())).toBe(false);
  });
});

describe("gizmo tool", () => {
  it("names the picked gizmo in the status bar, and Select again after null", () => {
    s().setGizmoMode("scale");
    expect(s().gizmoMode).toBe("scale");
    expect(s().tool).toBe(GIZMO_TOOLS.scale);
    s().setGizmoMode("combined");
    expect(s().tool).toBe(GIZMO_TOOLS.combined);
    s().setGizmoMode(null);
    expect(s().gizmoMode).toBeNull();
    expect(s().tool).toBe(SELECT_TOOL);
  });

  it("goes back to Select on New project and Open project", () => {
    s().setGizmoMode("scale");
    s().newProject();
    expect(s().gizmoMode).toBeNull();
    expect(s().tool).toBe(SELECT_TOOL);
    s().setGizmoMode("combined");
    s().loadProject({ name: "b", path: "C:\\p\\b.3mf", platform: [200, 200, 200], parts: [] });
    expect(s().gizmoMode).toBeNull();
    expect(s().tool).toBe(SELECT_TOOL);
  });

  it("shows the picked gizmo, else the combined one while Always show gizmo is on", () => {
    const table: [GizmoMode | null, boolean, GizmoMode | null][] = [
      [null, true, "combined"],
      [null, false, null],
      ["combined", true, "combined"],
      ["combined", false, "combined"],
      ["scale", true, "scale"],
      ["scale", false, "scale"],
    ];
    for (const [gizmoMode, alwaysShowGizmo, shown] of table) {
      expect(shownGizmo({ gizmoMode, view: { ...s().view, alwaysShowGizmo } })).toBe(shown);
    }
  });
});

describe("view settings", () => {
  it("keep the fields a change leaves out", () => {
    const before = s().view;
    s().setView({ alwaysShowGizmo: !before.alwaysShowGizmo });
    expect(s().view).toEqual({ ...before, alwaysShowGizmo: !before.alwaysShowGizmo });
    s().setView({ showPlatform: !before.showPlatform, projection: "orthographic" });
    expect(s().view).toEqual({
      ...before,
      alwaysShowGizmo: !before.alwaysShowGizmo,
      showPlatform: !before.showPlatform,
      projection: "orthographic",
    });
    s().setView(before);
    expect(s().view).toEqual(before);
  });
});

/** A Storage stand-in that keeps what is written in `data`. */
function fakeStorage(entries: Record<string, string> = {}) {
  const data = new Map(Object.entries(entries));
  return {
    data,
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => void data.set(key, value),
  };
}

/** A new store module, which reads the remembered settings again as a fresh start of the app does. */
async function freshStore() {
  vi.resetModules();
  return (await import("./store")).useStore;
}

describe("remembered view settings", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("start with the platform hidden and the gizmo shown when there is no storage", async () => {
    // Node has no localStorage either; this makes the test independent of the Node version.
    vi.stubGlobal("localStorage", undefined);
    const store = await freshStore();
    expect(store.getState().view).toMatchObject({ showPlatform: false, alwaysShowGizmo: true });
    // Saving fails quietly; the setting still holds for this session.
    store.getState().setView({ showPlatform: true });
    expect(store.getState().view.showPlatform).toBe(true);
  });

  it("start from what was stored last time", async () => {
    vi.stubGlobal("localStorage", fakeStorage({ "tricky3d.showPlatform": "1", "tricky3d.alwaysShowGizmo": "0" }));
    const store = await freshStore();
    expect(store.getState().view).toMatchObject({ showPlatform: true, alwaysShowGizmo: false });
  });

  it("use the defaults when the stored value is not 1 or 0", async () => {
    vi.stubGlobal("localStorage", fakeStorage({ "tricky3d.showPlatform": "true", "tricky3d.alwaysShowGizmo": "false" }));
    const store = await freshStore();
    expect(store.getState().view).toMatchObject({ showPlatform: false, alwaysShowGizmo: true });
  });

  it("are saved when a change names them, and only then", async () => {
    const storage = fakeStorage();
    vi.stubGlobal("localStorage", storage);
    const store = await freshStore();
    store.getState().setView({ showAxes: false, showRulers: false, projection: "orthographic" });
    expect(storage.data.size).toBe(0);
    store.getState().setView({ showPlatform: true, alwaysShowGizmo: false });
    expect(Object.fromEntries(storage.data)).toEqual({ "tricky3d.showPlatform": "1", "tricky3d.alwaysShowGizmo": "0" });
    store.getState().setView({ alwaysShowGizmo: true });
    expect(Object.fromEntries(storage.data)).toEqual({ "tricky3d.showPlatform": "1", "tricky3d.alwaysShowGizmo": "1" });

    // The next start reads them back; the settings that are not remembered start from their defaults.
    store.getState().setView({ alwaysShowGizmo: false });
    expect((await freshStore()).getState().view).toEqual({
      projection: "perspective",
      shading: "shaded",
      showPlatform: true,
      showAxes: true,
      showRulers: true,
      alwaysShowGizmo: false,
    });
  });
});

describe("remembered export options", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("start as binary STL and one file, also when the stored values are unknown", async () => {
    vi.stubGlobal("localStorage", undefined);
    expect((await freshStore()).getState().exportOptions).toEqual({ stl: "binary", parts: "one-file" });
    vi.stubGlobal("localStorage", fakeStorage({ "tricky3d.exportStl": "text", "tricky3d.exportParts": "each" }));
    expect((await freshStore()).getState().exportOptions).toEqual({ stl: "binary", parts: "one-file" });
  });

  it("are saved by an export and read back at the next start", async () => {
    const storage = fakeStorage();
    vi.stubGlobal("localStorage", storage);
    const store = await freshStore();
    store.getState().setExportOptions({ stl: "ascii", parts: "per-part" });
    expect(store.getState().exportOptions).toEqual({ stl: "ascii", parts: "per-part" });
    expect(Object.fromEntries(storage.data)).toEqual({ "tricky3d.exportStl": "ascii", "tricky3d.exportParts": "per-part" });
    expect((await freshStore()).getState().exportOptions).toEqual({ stl: "ascii", parts: "per-part" });
    store.getState().setExportOptions({ stl: "binary", parts: "per-part" });
    expect((await freshStore()).getState().exportOptions).toEqual({ stl: "binary", parts: "per-part" });
  });
});

describe("copy and paste", () => {
  it("copies in parts-list order and steps each paste by the group's width plus a gap", () => {
    const mesh = makeBox([20, 20, 20]);
    const [a] = s().addParts([{ name: "A", mesh, matrix: translation(0, 0, 0) }], "Add A");
    const [b] = s().addParts([{ name: "B", mesh, matrix: translation(40, 0, 0) }], "Add B");
    s().select([b, a]);
    expect(s().copySelected()).toBe(2);
    expect(s().clipboard.map((p) => p.name)).toEqual(["A", "B"]);
    expect(s().history.past.map((e) => e.label)).toEqual(["Add A", "Add B"]);

    const first = s().pasteClipboard();
    expect(first).toHaveLength(2);
    expect(s().parts.find((p) => p.id === first[0])?.matrix[12]).toBe(65);
    expect(s().parts.find((p) => p.id === first[1])?.matrix[12]).toBe(105);
    expect(s().selection).toEqual(first);

    s().pasteClipboard();
    const third = s().pasteClipboard();
    const thirdX = s().parts.find((p) => p.id === third[0])?.matrix[12];
    s().undo();
    const again = s().pasteClipboard();
    expect(s().parts.find((p) => p.id === again[0])?.matrix[12]).toBe(thirdX);
    expect(s().parts.filter((p) => p.name === "A")).toHaveLength(4);
  });

  it("pastes copies that pack as their originals do, lock included", () => {
    const [id] = s().addParts([{ name: "A", mesh: makeBox([10, 10, 10]), locked: true, packing: { priority: 3, rotation: "z" } }], "Add A");
    s().select([id]);
    s().copySelected();
    const [copy] = s().pasteClipboard();
    const pasted = s().parts.find((p) => p.id === copy);
    expect([pasted?.locked, pasted?.packing]).toEqual([true, { priority: 3, rotation: "z" }]);
  });

  it("leaves the clipboard when nothing is selected, and New project clears it", () => {
    const [id] = s().addParts([{ name: "A", mesh: makeBox([10, 10, 10]) }], "Add A");
    expect(s().pasteClipboard()).toEqual([]);
    s().select([id]);
    s().copySelected();
    s().clearSelection();
    expect(s().copySelected()).toBe(0);
    expect(s().clipboard).toHaveLength(1);
    s().newProject();
    expect(s().clipboard).toEqual([]);
    expect(s().pasteCount).toBe(0);
  });

  it("places a recopy of the paste one step past that paste after redo", () => {
    const mesh = makeBox([20, 20, 20]);
    const [id] = s().addParts([{ name: "A", mesh }], "Add A");
    s().select([id]);
    s().copySelected();
    s().pasteClipboard();
    s().copySelected();
    s().undo();
    s().redo();
    const [again] = s().pasteClipboard();
    expect(s().parts.find((p) => p.id === again)?.matrix[12]).toBe(50);
  });

  it("does not move the next paste when an older copy is redone", () => {
    const mesh = makeBox([20, 20, 20]);
    const [id] = s().addParts([{ name: "A", mesh }], "Add A");
    s().select([id]);
    s().copySelected();
    s().pasteClipboard();
    s().select([id]);
    s().copySelected();
    s().undo();
    s().redo();
    const [pasted] = s().pasteClipboard();
    expect(s().parts.find((p) => p.id === pasted)?.matrix[12]).toBe(25);
  });
});
