import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { MeshData } from "./store";
import type { ActionSpec, Catalog, RunResult } from "./repair/types";

// Rust and the viewport are stand-ins: runRepair answers what each test says, and zooms are recorded. These tests
// check the registry (ids, groups, ready and Soon entries) and what the repair commands do with the session.
const ipc = vi.hoisted(() => ({
  runRepair: vi.fn(),
  cancelRepair: vi.fn(),
  fetchCatalog: vi.fn(),
  loadStoredScripts: vi.fn(),
  saveStoredScripts: vi.fn(),
  checkMeshes: vi.fn(),
  fetchCutPath: vi.fn(),
}));
const io = vi.hoisted(() => ({ pickOpenFile: vi.fn(), splitShells: vi.fn(), openProjectFile: vi.fn(), saveProjectFile: vi.fn() }));
const dialog = vi.hoisted(() => ({ message: vi.fn() }));
const view = vi.hoisted(() => ({ zoomTo: vi.fn(), zoomToBounds: vi.fn(), setViewPreset: vi.fn(), centreOn: vi.fn(), viewDirection: vi.fn() }));
vi.mock("./repair/ipc", async (importOriginal) => ({ ...(await importOriginal<typeof import("./repair/ipc")>()), ...ipc }));
vi.mock("./io/ipc", async (importOriginal) => ({ ...(await importOriginal<typeof import("./io/ipc")>()), ...io }));
vi.mock("@tauri-apps/plugin-dialog", () => dialog);
vi.mock("./viewport/api", () => ({ viewport: view, registerViewport: vi.fn() }));

import {
  CONTEXT_MENU,
  FEATURES,
  PICK_TRIANGLE_FIRST,
  REPAIR_TOOLS,
  activate,
  blockedReason,
  childrenOf,
  effectiveStatus,
  featureById,
  historyMenu,
  hoverHintFor,
  isMenu,
  registryProblems,
  runFeature,
  soonMessage,
  tooltipFor,
  type Feature,
} from "./features";
import { makeBox, translation } from "./mesh";
import { repairContextMenuEntries, tidy, type MenuEntry } from "./menus/entries";
import { CLOSE_BOOLEAN_FIRST, openBooleanMode } from "./boolean/session";
import { useBoolean } from "./boolean/store";
import { mergePartsCommand, openCutDialog, runCut, splitShellsCommand, WAIT_MESSAGE } from "./ops/meshOps";
import { bakeMesh } from "./ops/bake";
import { CLOSE_REPAIR_FIRST, SELECT_TRIANGLES_FIRST, closeRepair, enterRepair, isRepairing, runAction, selectAll, selectTriangles } from "./repair/session";
import { DEFAULT_VIEW, useRepair } from "./repair/store";
import { addCutPoint, executeCut, setCutSettings, stepCurrentPoint, undoLastPoint } from "./repair/cut/session";
import { DEFAULT_CUT, useCut } from "./repair/cut/store";
import { moveNode } from "./repair/nodes";
import { maskToIds } from "./repair/selection";
import { groupsFor } from "./ribbon/layout";
import { useStore } from "./store";
import { useUi } from "./ui";

const feature = (id: string): Feature => {
  const f = featureById(id);
  if (!f) throw new Error(`no feature ${id}`);
  return f;
};

const spec = (id: ActionSpec["id"], label: string, needsSelection = false): ActionSpec => ({
  id,
  label,
  implemented: true,
  inScripts: !needsSelection,
  needsSelection,
  hint: "",
  params: [],
});
const catalog: Catalog = {
  actions: [
    spec("close_all_holes", "Close all holes"),
    spec("close_trivial_holes", "Close trivial holes"),
    spec("close_defined_holes", "Close defined holes", true),
    spec("stitch_triangles", "Stitch triangles"),
    spec("fix_flipped_triangles", "Fix flipped triangles"),
    spec("split_non_oriented_edges", "Split non-oriented edges"),
    spec("split_non_manifold_edges", "Split non-manifold edges"),
    spec("remove_selected_triangles", "Remove selected triangles", true),
    spec("flip_selected_triangles", "Flip selected triangles", true),
    spec("smooth_triangles", "Smooth triangles"),
    spec("close_holes_curved", "Close holes along the curvature"),
    spec("bridge_gap", "Bridge a gap"),
    spec("grow_hole_band", "Grow a band from a hole edge"),
    spec("subdivide_triangles", "Subdivide triangles"),
    spec("add_triangle", "Add triangle"),
    spec("add_point", "Add point", true),
    { ...spec("move_node", "Move nodes"), inScripts: false },
    { ...spec("cut_surface", "Cut surfaces"), inScripts: false },
    spec("select_problem_triangles", "Select by criteria"),
    spec("find_overlaps", "Find overlapping triangles"),
  ],
  scripts: [],
};

const unchanged = (mesh: MeshData): RunResult => ({
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
});
const changed = (mesh: MeshData, selection = new Uint32Array(0)): RunResult => ({ ...unchanged(mesh), changed: true, selection });
/** One action's report with this message, as Rust sends it. */
const report = (message: string) => ({ action: "subdivide_triangles" as const, label: "", message, warnings: [], resolved: [], changed: false, millis: 0 });

const r = () => useRepair.getState();
const s = () => useStore.getState();
const session = () => {
  const current = r().session;
  if (!current) throw new Error("not repairing");
  return current;
};
const toasts = () => useUi.getState().toasts.map((t) => t.message);
const lastToast = () => toasts()[toasts().length - 1];
const idle = () => vi.waitFor(() => expect(r().busy).toBeNull());
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => (resolve = res));
  return { promise, resolve };
}

/** Two separate 20 mm boxes in one mesh: triangles 0–11 and 12–23. */
function twoBoxes(): MeshData {
  const a = makeBox([20, 20, 20]);
  const b = bakeMesh(makeBox([20, 20, 20]), translation(50, 0, 0));
  const indices = new Uint32Array(a.indices.length * 2);
  indices.set(a.indices);
  indices.set(b.indices.map((v) => v + a.positions.length / 3), a.indices.length);
  const positions = new Float32Array(a.positions.length * 2);
  positions.set(a.positions);
  positions.set(b.positions, a.positions.length);
  return { positions, indices };
}

function addPart(name = "Part", mesh: MeshData = makeBox([20, 20, 20])): string {
  const [id] = s().addParts([{ name, mesh }], `Add ${name}`);
  return id;
}

/** Enters repair mode on a new part (selected first, as the commands expect). */
async function enter(mesh?: MeshData): Promise<string> {
  const id = addPart("Part", mesh);
  ipc.runRepair.mockImplementationOnce(async (m: MeshData) => unchanged(m));
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
  for (const f of [...Object.values(ipc), ...Object.values(io), ...Object.values(view), dialog.message]) f.mockReset();
  ipc.cancelRepair.mockResolvedValue(undefined);
  useRepair.setState({ catalog, session: null, busy: null, autoUpdate: false, selectTool: "triangle", editTool: null, view: DEFAULT_VIEW });
  useCut.setState({ points: [], current: -1, added: [], settings: DEFAULT_CUT, path: null, routedFor: null, routing: false, routeError: null });
  useBoolean.setState({ session: null });
  s().newProject();
  useUi.setState({ toasts: [], dialog: null, ribbonTab: "modify", contextMenu: null });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

// ---------------------------------------------------------------------------------------------

/** The Repair tab as designed: group → ids, in order. */
const REPAIR_GROUPS: [string, string[]][] = [
  ["Repair", ["repair.script", "repair.edit-scripts"]],
  ["Info", ["repair.selection-info", "repair.status"]],
  [
    "Select",
    [
      "repair.select-primitive",
      "repair.select-triangle",
      "repair.select-surface",
      "repair.select-brush",
      "repair.select-shell",
      "repair.select-all",
      "repair.select-none",
      "repair.select-overlap",
      "repair.select-toggle",
      "repair.select-advanced",
    ],
  ],
  ["Refine mesh", ["repair.z-comp", "repair.remesh", "repair.offset", "repair.join-contours", "repair.close-freeform", "repair.split-non-manifold"]],
  ["Add", ["repair.add-triangle", "repair.add-nodes", "repair.add-free-triangle"]],
  ["Remove", ["repair.remove-triangles", "repair.remove-edge"]],
  ["Edit mesh", ["repair.flip-selected", "repair.move-nodes", "repair.cut-surfaces", "repair.extrude", "repair.extract"]],
  ["Zoom to", ["repair.zoom-part", "repair.zoom-selected", "repair.zoom-area"]],
  ["Close", ["repair.apply", "repair.close"]],
];

/** The Home tab as designed (the reference tool's layout): group → ids, in order. */
const HOME_GROUPS: [string, string[]][] = [
  ["Edit", ["home.save", "home.undo", "home.redo"]],
  ["Add parts", ["home.my-machines", "home.add-part", "home.part-library", "home.platform-statistics"]],
  ["Prepare", ["home.repair", "home.run-script"]],
  ["Analyze", ["home.new-analysis", "home.measure"]],
  ["Modify", ["home.boolean", "home.generate-shell", "home.generate-label"]],
  ["Arrange", ["home.scale", "home.rotate", "home.move", "home.orient", "home.duplicate", "home.pack"]],
  ["Support", ["home.generate-supports", "home.manage-supports"]],
  ["Slice parts", ["home.slice-parts"]],
  ["Zoom to", ["home.zoom-everything", "home.zoom-selected", "home.zoom-platform", "home.zoom-area", "home.zoom-parts", "home.zoom-point"]],
  ["Utilities", ["home.open-utility"]],
];

/** Where each repair tool of the old Home tab went (or the entry that already did the same). */
const FORMER_HOME_REPAIR = [
  "repair.damage-warnings",
  "repair.problem-counts",
  "repair.shell-list",
  "repair.apply",
  "repair.select-primitive",
  "repair.select-triangle",
  "repair.select-surface",
  "repair.select-shell",
  "repair.select-brush",
  "repair.select-criteria",
  "repair.selection-info",
  "repair.select-overlap",
  "repair.hide-triangles",
  "repair-ctx.close-all",
  "repair-ctx.close-defined",
  "repair-ctx.close-curved",
  "repair-ctx.bridge-gap",
  "repair-ctx.grow-band",
  "repair-ctx.stitch",
  "repair-ctx.remove-duplicates",
  "repair-ctx.remove-degenerate",
  "repair-ctx.merge-points",
  "repair-ctx.remove-tiny-shells",
  "repair-ctx.self-intersections",
  "repair-ctx.split-non-oriented",
  "repair.split-non-manifold",
  "repair-ctx.remove-problem-areas",
  "repair-ctx.fix-flipped",
  "repair-ctx.invert-shells",
  "repair-ctx.wrap-surface",
  "repair-ctx.refine",
  "repair-ctx.fill-three-edge",
  "repair-ctx.insert-point",
  "repair.move-nodes",
  "repair-ctx.smooth",
  "repair.remove-triangles",
  "repair.flip-selected",
  "repair.cut-surfaces",
  "repair.extract-copy",
  "repair.extract-shells",
  "repair.edit-scripts",
];

/** The entries of the Repair tab that work today; every other entry of the tab is Soon. */
const REPAIR_READY = new Set([
  "repair.script",
  "repair.edit-scripts",
  "repair.selection-info",
  "repair.status",
  "repair.damage-warnings",
  "repair.problem-counts",
  "repair.shell-list",
  "repair.select-primitive",
  "repair.select-triangle",
  "repair.select-surface",
  "repair.select-brush",
  "repair.select-shell",
  "repair.select-all",
  "repair.select-none",
  "repair.select-overlap",
  "repair.select-toggle",
  "repair.select-advanced",
  "repair.select-criteria",
  "repair.hide-triangles",
  "repair.split-non-manifold",
  "repair.remove-triangles",
  "repair.flip-selected",
  "repair.move-nodes",
  "repair.cut-surfaces",
  "repair.extract",
  "repair.extract-copy",
  "repair.extract-separate",
  "repair.extract-shells",
  "repair.zoom-part",
  "repair.zoom-selected",
  "repair.zoom-area",
  "repair.apply",
  "repair.close",
]);

/** The repair right-click menu as designed, top to bottom, and the entries that need a selection. */
const REPAIR_MENU: [string, string][] = [
  ["repair-ctx.add-triangle", "Add this triangle to selection"],
  ["repair-ctx.add-surface", "Add this surface to selection"],
  ["repair-ctx.add-shell", "Add this shell to selection"],
  ["repair-ctx.expand", "Expand selection"],
  ["repair-ctx.shrink", "Shrink selection"],
  ["repair-ctx.select-criteria", "Select damaged triangles"],
  ["repair-ctx.hide-triangles", "Hide selected triangles"],
  ["repair-ctx.reduce", "Reduce triangles"],
  ["repair-ctx.refine", "Refine triangle mesh"],
  ["repair-ctx.insert-point", "Add point"],
  ["repair-ctx.smooth", "Smooth triangles"],
  ["repair-ctx.z-compensation", "Z-compensation"],
  ["repair-ctx.remesh", "Remesh"],
  ["repair-ctx.offset", "Offset selection"],
  ["repair-ctx.close-trivial", "Close trivial holes"],
  ["repair-ctx.close-defined", "Close defined holes"],
  ["repair-ctx.close-all", "Close all holes"],
  ["repair-ctx.close-curved", "Close holes along the curvature"],
  ["repair-ctx.fill-three-edge", "Fill three-edge holes"],
  ["repair-ctx.grow-band", "Grow a band from a hole edge"],
  ["repair-ctx.bridge-gap", "Bridge a gap"],
  ["repair-ctx.stitch", "Stitch triangles"],
  ["repair-ctx.remove-duplicates", "Remove duplicate triangles"],
  ["repair-ctx.remove-degenerate", "Collapse degenerate triangles"],
  ["repair-ctx.merge-points", "Merge duplicate points"],
  ["repair-ctx.remove-tiny-shells", "Delete small shells"],
  ["repair-ctx.self-intersections", "Fix self-intersections"],
  ["repair-ctx.remove-problem-areas", "Cut out damaged areas"],
  ["repair-ctx.wrap-surface", "Keep only the outer surface"],
  ["repair-ctx.remove-selected", "Remove selected triangles"],
  ["repair-ctx.flip-selected", "Flip selected triangles"],
  ["repair-ctx.fix-flipped", "Fix flipped triangles"],
  ["repair-ctx.invert-shells", "Turn inside-out shells"],
  ["repair-ctx.split-non-oriented", "Split non-oriented edges"],
  ["repair-ctx.extract-copy", "Extract selected triangles as part"],
  ["repair-ctx.extract-separate", "Separate selection as part"],
  ["repair-ctx.extract-shells", "Selected shells to new part"],
];
const MENU_SOON = new Set(["repair-ctx.reduce", "repair-ctx.z-compensation", "repair-ctx.remesh", "repair-ctx.offset", "repair-ctx.bridge-gap"]);
const MENU_NEEDS_SELECTION = new Set([
  "repair-ctx.expand",
  "repair-ctx.shrink",
  "repair-ctx.insert-point",
  "repair-ctx.close-defined",
  "repair-ctx.remove-selected",
  "repair-ctx.flip-selected",
  "repair-ctx.extract-copy",
  "repair-ctx.extract-separate",
]);

/** Main-document commands that would change the part being repaired. */
const MAIN_COMMANDS = [
  "arrange.move",
  "arrange.rotate",
  "modify.scale",
  "arrange.place-on-platform",
  "arrange.centre-on-platform",
  "context.delete",
  "modify.cut",
  "modify.boolean",
  "modify.split-shells",
  "modify.merge-parts",
  "home.repair",
  "context.repair",
  "quick.repair",
  "context.copy",
  "context.paste",
  "context.batch-repair",
];

const ids = (entries: readonly MenuEntry[]) => entries.map((e) => (e === "-" ? "-" : e.id));

describe("the registry", () => {
  it("has no mistakes", () => {
    expect(registryProblems()).toEqual([]);
  });

  it("lists the Repair tab's designed groups and buttons, in order, with Extract parts as a menu", () => {
    const groups = groupsFor("repair", false);
    expect(groups.map((g) => [g.name, g.entries.map((f) => f.id)])).toEqual(REPAIR_GROUPS);
    expect(childrenOf("repair.extract").map((f) => [f.id, f.label])).toEqual([
      ["repair.extract-copy", "Extract selected triangles as part"],
      ["repair.extract-separate", "Separate selection as part"],
      ["repair.extract-shells", "Selected shells to new part"],
    ]);
    expect(["repair.script", "repair.apply", "repair.close"].map((id) => feature(id).size)).toEqual(["large", "large", "large"]);
  });

  it("makes ready exactly the Repair tab entries that work, each with a run, and leaves the rest Soon without one", () => {
    const tab = FEATURES.filter((f) => f.tab === "repair");
    expect(tab.length).toBe(REPAIR_GROUPS.flatMap(([, list]) => list).length + 8);
    for (const f of tab) {
      expect([f.id, effectiveStatus(f)]).toEqual([f.id, REPAIR_READY.has(f.id) ? "ready" : "soon"]);
      if (isMenu(f)) expect(f.run).toBeUndefined();
      else expect([f.id, typeof f.run]).toEqual([f.id, REPAIR_READY.has(f.id) ? "function" : "undefined"]);
    }
  });

  it("lists the repair context menu's entries in the designed order, in sections, greyed as designed", () => {
    const entries = repairContextMenuEntries();
    const rows = entries.filter((e): e is Feature => e !== "-");
    expect(rows.map((f) => [f.id, f.label])).toEqual(REPAIR_MENU);
    // Sections: this triangle, the selection, mesh actions, holes, clean up, triangles, extract.
    expect(ids(entries).join(" ").split(" - ").length).toBe(7);
    for (const f of rows) {
      expect([f.id, f.status]).toEqual([f.id, MENU_SOON.has(f.id) ? "soon" : "ready"]);
      expect([f.id, f.needsTriangles === true]).toEqual([f.id, MENU_NEEDS_SELECTION.has(f.id)]);
      expect([f.id, f.underCursor === true]).toEqual([f.id, f.id.startsWith("repair-ctx.add-")]);
    }
    expect([feature("repair-ctx.expand").shortcut, feature("repair-ctx.shrink").shortcut]).toEqual(["=", "-"]);
  });

  it("drops the Soon section of the repair context menu when Soon features are hidden, without a double separator", () => {
    const shown = ids(tidy(repairContextMenuEntries(), true));
    expect(shown.some((id) => MENU_SOON.has(id))).toBe(false);
    expect(shown.join(",")).not.toContain("-,-");
    expect(shown[0]).not.toBe("-");
    expect(shown[shown.length - 1]).not.toBe("-");
  });

  it("lays out the Home tab like the reference: its groups and buttons in order, with Repair part large", () => {
    const groups = groupsFor("home", false);
    expect(groups.map((g) => [g.name, g.entries.map((f) => f.id)])).toEqual(HOME_GROUPS);
    const home = feature("home.repair");
    expect([home.label, home.status, home.size, typeof home.run]).toEqual(["Repair part", "ready", "large", "function"]);
    expect(home.hint).toBe("Repair the selected part: find and fix holes, flipped triangles and other problems.");
    expect(childrenOf("home.run-script").map((f) => [f.id, effectiveStatus(f)])).toEqual([
      ["home.run-repair-script", "ready"],
      ["home.run-support-script", "soon"],
      ["home.run-lua-script", "soon"],
      ["home.quick-access", "soon"],
    ]);
    expect(isMenu(feature("home.quick-access"))).toBe(true);
    expect(childrenOf("home.new-analysis").map((f) => f.label)).toEqual([
      "Up-/Down-skin",
      "Center of gravity",
      "Standard analysis",
      "Shadow area",
      "Support volume",
      "Wall thickness",
    ]);
    expect(childrenOf("home.new-analysis").every((f) => f.status === "soon")).toBe(true);
    // Home's buttons run the same commands as their twins on the other tabs.
    for (const [a, b] of [
      ["home.boolean", "modify.boolean"],
      ["home.scale", "modify.scale"],
      ["home.rotate", "arrange.rotate"],
      ["home.move", "arrange.move"],
      ["home.save", "file.save"],
    ]) {
      expect([a, feature(a).run]).toEqual([a, feature(b).run]);
    }
    const quick = FEATURES.filter((f) => f.tab === "quick");
    expect(quick.map((f) => [f.id, f.label, f.status, f.group])).toEqual([
      ["quick.repair", "Repair", "ready", "Quick"],
      ["quick.model-package", "Create model package", "soon", "Quick"],
      ["quick.slice", "Slice selected parts", "soon", "Quick"],
      ["quick.file-preview", "File preview browser", "soon", "Quick"],
      ["quick.collisions", "Collision detection", "soon", "Quick"],
    ]);
    expect(quick[0].icon).toBe(home.icon);
    const at = CONTEXT_MENU.indexOf("context.rename");
    expect(CONTEXT_MENU[at + 1]).toBe("context.repair");
    expect(feature("context.repair").label).toBe("Repair part");
    // Copy and Paste moved to the part menu, Platform size to Arrange, Export to File › Export part.
    expect(CONTEXT_MENU.slice(0, 5)).toEqual(["context.export", "-", "context.copy", "context.paste", "context.duplicate"]);
    expect(CONTEXT_MENU).toContain("context.batch-repair");
    expect(feature("arrange.platform-size").run).toBeTypeOf("function");
    expect(feature("file.export-any").parent).toBe("file.export");
  });

  it("lays out the Measurement tab: six dropdowns as the reference lists them, Wall thickness, 3-point angle and Cancel", () => {
    const groups = groupsFor("measure", false);
    expect(groups.map((g) => [g.name, g.entries.map((f) => f.id)])).toEqual([
      ["New measurement", ["measure.point", "measure.edge", "measure.circle", "measure.circle3", "measure.sphere4", "measure.surface", "measure.wall", "measure.angle3"]],
      ["Exit measurement", ["measure.cancel"]],
    ]);
    expect(childrenOf("measure.point").map((f) => f.label)).toEqual([
      "Distance point to point",
      "Distance point to edge",
      "Distance point to detected circle",
      "Distance point to 3-point circle",
      "Distance point to sphere",
      "Distance point to surface",
      "Position point",
    ]);
    expect(childrenOf("measure.edge").map((f) => f.label)).toEqual([
      "Distance edge to point",
      "Distance edge to edge",
      "Distance edge to sphere",
      "Angle edge to edge",
      "Angle edge to surface",
      "Angle edge to YZ plane",
      "Angle edge to XZ plane",
      "Angle edge to XY plane",
    ]);
    expect(childrenOf("measure.surface").map((f) => f.label)).toEqual([
      "Distance surface to point",
      "Angle surface to edge",
      "Angle surface to surface",
      "Position surface",
      "Angle surface to YZ plane",
      "Angle surface to XZ plane",
      "Angle surface to XY plane",
    ]);
    const tab = FEATURES.filter((f) => f.tab === "measure");
    expect(tab.every((f) => effectiveStatus(f) === "ready")).toBe(true);
    expect(["home.measure", "analyze.measure", "analyze.wall-thickness"].map((id) => effectiveStatus(feature(id)))).toEqual(["ready", "ready", "ready"]);
  });

  it("keeps every repair tool the Home tab had, in the Repair tab or the repair right-click menu", () => {
    for (const id of FORMER_HOME_REPAIR) {
      const f = feature(id);
      expect([id, ["repair", "repair-context"].includes(f.tab)]).toEqual([id, true]);
      expect([id, effectiveStatus(f)]).toEqual([id, id === "repair-ctx.bridge-gap" ? "soon" : "ready"]);
    }
    expect(FEATURES.some((f) => f.tab === "home" && f.group === "Repair")).toBe(false);
  });

  it("toasts for a Soon repair entry instead of running anything", () => {
    activate(feature("repair.z-comp"));
    expect(lastToast()).toBe(soonMessage(feature("repair.z-comp")));
    expect(blockedReason(feature("repair.z-comp"))).toBeNull();
  });

  it("names the repair select tools in the status bar", () => {
    expect(REPAIR_TOOLS.triangle.name).toBe("Repair: Triangle");
    expect(REPAIR_TOOLS.triangle.hint).toMatch(
      /^Click a triangle to select it, Shift\+click to add or remove it\. A box dragged from empty space selects the triangles facing you \(Shift adds, Ctrl toggles\)\. Right-drag orbits/,
    );
    expect(REPAIR_TOOLS.surface.name).toBe("Repair: Surface");
    expect(REPAIR_TOOLS.shell.name).toBe("Repair: Shell");
  });
});

describe("entries that cannot run right now", () => {
  it("are greyed out while no triangle is selected, with the reason in the tooltip, and a click only shows it", async () => {
    await enter();
    const remove = feature("repair.remove-triangles");
    expect(blockedReason(remove)).toBe(SELECT_TRIANGLES_FIRST);
    expect(tooltipFor(remove, blockedReason(remove))).toBe("Remove triangle (Del) — Select triangles first.");
    expect(hoverHintFor(remove, blockedReason(remove))).toBe(SELECT_TRIANGLES_FIRST);
    activate(remove);
    expect(lastToast()).toBe(SELECT_TRIANGLES_FIRST);
    expect(ipc.runRepair).not.toHaveBeenCalled();

    selectTriangles([3, 5], "replace");
    expect(blockedReason(remove)).toBeNull();
    expect(tooltipFor(remove)).toBe("Remove triangle (Del) — Delete the selected triangles.");
    ipc.runRepair.mockImplementationOnce(async () => changed(makeBox([1, 1, 1])));
    activate(remove);
    await idle();
    const [, steps, opts] = ipc.runRepair.mock.calls[0];
    expect(steps).toEqual([{ type: "remove_selected_triangles", enabled: true, params: {} }]);
    expect(Array.from(opts.selection)).toEqual([3, 5]);
  });

  it("include the Add this triangle entries when the menu did not open on a triangle", async () => {
    await enter();
    const add = feature("repair-ctx.add-triangle");
    expect(blockedReason(add, r(), undefined)).toBe(PICK_TRIANGLE_FIRST);
    expect(blockedReason(add, r(), null)).toBe(PICK_TRIANGLE_FIRST);
    expect(blockedReason(add, r(), 4)).toBeNull();
    useUi.getState().openContextMenu({ x: 1, y: 1, point: null, triangle: null });
    activate(add);
    expect(lastToast()).toBe(PICK_TRIANGLE_FIRST);
    expect(session().selectedCount).toBe(0);
  });

  it("include the main-document commands while repair mode is open: they refuse with Close repair mode first", async () => {
    const id = await enter();
    const before = s().parts;
    for (const command of MAIN_COMMANDS) {
      useUi.setState({ toasts: [] });
      expect([command, blockedReason(feature(command))]).toEqual([command, CLOSE_REPAIR_FIRST]);
      runFeature(command);
      expect([command, toasts()]).toEqual([command, [CLOSE_REPAIR_FIRST]]);
    }
    expect(s().parts).toBe(before);
    expect(s().gizmoMode).toBeNull();
    expect(useUi.getState().dialog).toBeNull();
    expect(session().partId).toBe(id);

    // The Cuts panel and the dialogs call these directly; they refuse too.
    const direct: [string, () => unknown][] = [
      ["openCutDialog", openCutDialog],
      ["openBooleanMode", openBooleanMode],
      ["splitShellsCommand", splitShellsCommand],
      ["mergePartsCommand", mergePartsCommand],
      ["runCut", () => runCut({ scope: "selected", plane: { normal: [0, 0, 1], offset: 0 }, cap: true, gap: 0 })],
    ];
    for (const [name, call] of direct) {
      useUi.setState({ toasts: [] });
      await call();
      expect([name, toasts()]).toEqual([name, [CLOSE_REPAIR_FIRST]]);
    }
    expect(io.splitShells).not.toHaveBeenCalled();
    expect(useUi.getState().dialog).toBeNull();
    expect(s().parts).toBe(before);

    // Outside repair mode nothing is blocked.
    expect(await closeRepair()).toBe(true);
    for (const command of MAIN_COMMANDS) expect([command, blockedReason(feature(command))]).toEqual([command, null]);
  });

  it("include Analyze › Information while repair mode is open, since the repair panel has no Information section", async () => {
    await enter();
    const info = feature("analyze.information");
    expect(blockedReason(info)).toBe(CLOSE_REPAIR_FIRST);
    expect(tooltipFor(info, blockedReason(info))).toBe("Information — Close repair mode first.");
    const focus = useUi.getState().focus;
    runFeature("analyze.information");
    expect(lastToast()).toBe(CLOSE_REPAIR_FIRST);
    // No request for a section that is not on show, which would only play its highlight once repair mode ends.
    expect(useUi.getState().focus).toBe(focus);

    expect(await closeRepair()).toBe(true);
    expect(blockedReason(info)).toBeNull();
    runFeature("analyze.information");
    expect(useUi.getState().focus?.section).toBe("information");
  });
});

describe("repair commands", () => {
  it("start repair from Home, the quick strip and the part menu on the selected part, but not during a mesh operation", async () => {
    for (const command of ["home.repair", "quick.repair", "context.repair"]) {
      s().newProject();
      const id = addPart();
      ipc.runRepair.mockImplementation(async (m: MeshData) => unchanged(m));
      runFeature(command);
      expect([command, isRepairing()]).toEqual([command, true]);
      expect(session().partId).toBe(id);
      expect(useUi.getState().ribbonTab).toBe("repair");
      await idle();
      expect(await closeRepair()).toBe(true);
    }

    // A Split shells still running would replace the part under the repair.
    s().newProject();
    addPart();
    let finish: (value: { count: number; shells: [] }[]) => void = () => {};
    io.splitShells.mockReturnValueOnce(new Promise((resolve) => (finish = resolve)));
    const split = splitShellsCommand();
    useUi.setState({ toasts: [] });
    runFeature("home.repair");
    expect(isRepairing()).toBe(false);
    expect(lastToast()).toBe(WAIT_MESSAGE);
    finish([{ count: 1, shells: [] }]);
    await split;
  });

  it("show the select tools pressed from the repair store, and pick them", async () => {
    await enter();
    const pressed = () => ["triangle", "surface", "shell"].map((t) => feature(`repair.select-${t}`).repairActive?.(r()));
    expect(pressed()).toEqual([true, false, false]);
    runFeature("repair.select-surface");
    expect(r().selectTool).toBe("surface");
    expect(pressed()).toEqual([false, true, false]);
    runFeature("repair.select-shell");
    expect(pressed()).toEqual([false, false, true]);
  });

  it("select all, none and the other way round", async () => {
    await enter();
    runFeature("repair.select-all");
    expect(session().selectedCount).toBe(12);
    runFeature("repair.select-none");
    expect(session().selectedCount).toBe(0);
    selectTriangles([0, 1], "replace");
    runFeature("repair.select-toggle");
    expect(Array.from(maskToIds(session().selection))).toEqual([2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
  });

  it("add the triangle, surface or shell under the cursor to the selection", async () => {
    await enter(twoBoxes());
    useRepair.getState().setView({ surfaceTolerance: 10 });
    selectTriangles([20], "replace");
    useUi.getState().openContextMenu({ x: 1, y: 1, point: null, triangle: 2 });
    runFeature("repair-ctx.add-triangle");
    expect(Array.from(maskToIds(session().selection))).toEqual([2, 20]);
    // Triangle 2 is half of the top face; the surface at 10° is that face.
    runFeature("repair-ctx.add-surface");
    expect(Array.from(maskToIds(session().selection))).toEqual([2, 3, 20]);
    runFeature("repair-ctx.add-shell");
    expect(Array.from(maskToIds(session().selection))).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 20]);
  });

  it("grow and shrink the selection from the context menu", async () => {
    await enter();
    selectTriangles([0], "replace");
    runFeature("repair-ctx.expand");
    const grown = session().selectedCount;
    expect(grown).toBeGreaterThan(1);
    runFeature("repair-ctx.shrink");
    expect(session().selectedCount).toBeLessThan(grown);
  });

  it("run each action entry's action, on the selection when there is one", async () => {
    const actions: [string, string][] = [
      ["repair-ctx.smooth", "smooth_triangles"],
      ["repair-ctx.close-trivial", "close_trivial_holes"],
      ["repair-ctx.close-defined", "close_defined_holes"],
      ["repair-ctx.close-all", "close_all_holes"],
      ["repair-ctx.stitch", "stitch_triangles"],
      ["repair-ctx.remove-selected", "remove_selected_triangles"],
      ["repair-ctx.flip-selected", "flip_selected_triangles"],
      ["repair-ctx.fix-flipped", "fix_flipped_triangles"],
      ["repair-ctx.split-non-oriented", "split_non_oriented_edges"],
      ["repair.split-non-manifold", "split_non_manifold_edges"],
      ["repair.flip-selected", "flip_selected_triangles"],
    ];
    await enter();
    ipc.runRepair.mockImplementation(async (m: MeshData) => unchanged(m));
    selectTriangles([7], "replace");
    for (const [id] of actions) {
      runFeature(id);
      await idle();
    }
    expect(ipc.runRepair.mock.calls.map((call) => call[1][0].type)).toEqual(actions.map(([, action]) => action));
    for (const call of ipc.runRepair.mock.calls) expect(Array.from(call[2].selection)).toEqual([7]);
  });

  it("make Undo and Redo step through the repair history, not the main one", async () => {
    await enter();
    const start = session().mesh;
    const fixed = makeBox([30, 30, 30]);
    ipc.runRepair.mockResolvedValueOnce(changed(fixed));
    await runAction("close_all_holes");
    const mainSteps = s().history.past.length;

    runFeature("modify.undo");
    expect(session().mesh).toBe(start);
    runFeature("home.undo");
    expect(lastToast()).toBe("Nothing to undo.");
    runFeature("home.redo");
    expect(session().mesh).toBe(fixed);
    runFeature("modify.redo");
    expect(lastToast()).toBe("Nothing to redo.");
    expect(s().history.past.length).toBe(mainSteps);
    expect(s().parts).toHaveLength(1);
  });

  it("zoom to the part, and to the selected triangles or the part when none is selected", async () => {
    await enter(twoBoxes());
    runFeature("repair.zoom-part");
    expect(view.zoomToBounds).toHaveBeenLastCalledWith({ min: [-10, -10, -10], max: [60, 10, 10] });
    runFeature("repair.zoom-selected");
    expect(view.zoomToBounds).toHaveBeenLastCalledWith({ min: [-10, -10, -10], max: [60, 10, 10] });
    selectTriangles([12], "replace");
    runFeature("repair.zoom-selected");
    const box = view.zoomToBounds.mock.calls[view.zoomToBounds.mock.calls.length - 1][0];
    expect(box.min[0]).toBeGreaterThanOrEqual(40);
    expect(view.zoomTo).not.toHaveBeenCalled();
  });

  it("open the script picker and the Apply dialog, show the selection info, and close repair mode", async () => {
    await enter();
    runFeature("repair.script");
    expect(useUi.getState().dialog).toEqual({ kind: "repair-script-picker" });
    useUi.getState().closeDialog();
    runFeature("repair.apply");
    expect(useUi.getState().dialog).toEqual({ kind: "repair-apply" });
    useUi.getState().closeDialog();
    runFeature("repair.selection-info");
    expect(lastToast()).toBe("No triangles selected.");
    runFeature("repair.close");
    await flush();
    expect(isRepairing()).toBe(false);
    expect(useUi.getState().ribbonTab).toBe("modify");
  });

  it("extract the selected triangles as a part, as a copy or taken out", async () => {
    await enter();
    selectTriangles([0, 1], "replace");
    runFeature("repair.extract-copy");
    await flush();
    expect(s().parts.map((p) => p.name)).toEqual(["Part", "Part (extracted)"]);
    expect(ipc.runRepair).not.toHaveBeenCalled();
    ipc.runRepair.mockResolvedValueOnce(changed(makeBox([1, 1, 1])));
    runFeature("repair-ctx.extract-separate");
    await idle();
    await flush();
    expect(ipc.runRepair.mock.calls[0][1][0].type).toBe("remove_selected_triangles");
    expect(s().parts).toHaveLength(3);
  });

  it("make New project and Open leave repair mode first, and stop when it stays open", async () => {
    const id = await enter();
    ipc.runRepair.mockResolvedValueOnce(changed(makeBox([1, 1, 1])));
    await runAction("close_all_holes");
    const generation = s().generation;

    // "Discard repair changes?" answered Cancel: nothing else happens.
    dialog.message.mockResolvedValueOnce("Cancel");
    runFeature("file.new");
    await vi.waitFor(() => expect(dialog.message).toHaveBeenCalledTimes(1));
    await flush();
    expect(isRepairing()).toBe(true);
    expect(s().generation).toBe(generation);
    dialog.message.mockResolvedValueOnce("Cancel");
    runFeature("file.open");
    await vi.waitFor(() => expect(dialog.message).toHaveBeenCalledTimes(2));
    await flush();
    expect(isRepairing()).toBe(true);
    expect(io.pickOpenFile).not.toHaveBeenCalled();
    expect(dialog.message.mock.calls.map((call) => call[0])).toEqual(["Discard repair changes?", "Discard repair changes?"]);

    // Discard, then the unsaved project question: Don't save. Repair mode closes, then the project starts over.
    dialog.message.mockResolvedValueOnce("Discard").mockResolvedValueOnce("Don't save");
    runFeature("file.new");
    await vi.waitFor(() => expect(s().generation).toBe(generation + 1));
    expect(isRepairing()).toBe(false);
    expect(s().parts.some((p) => p.id === id)).toBe(false);

    // Open with nothing to discard goes straight to the file dialog.
    await enter();
    io.pickOpenFile.mockResolvedValueOnce(null);
    dialog.message.mockResolvedValueOnce("Don't save");
    runFeature("file.open");
    await vi.waitFor(() => expect(io.pickOpenFile).toHaveBeenCalled());
    expect(isRepairing()).toBe(false);
  });

  /** Puts the part into repair mode while the window is live (an Open reading, a save writing) and changes the working copy. */
  async function repairMeanwhile(id: string) {
    s().select([id]);
    ipc.runRepair.mockImplementationOnce(async (m: MeshData) => unchanged(m));
    runFeature("home.repair");
    expect(isRepairing()).toBe(true);
    await idle();
    ipc.runRepair.mockResolvedValueOnce(changed(makeBox([1, 1, 1])));
    await runAction("close_all_holes");
    expect(session().mesh).not.toBe(session().initialMesh);
  }

  it("let a File › Open replace the project without any further question when no part is in repair mode", async () => {
    addPart();
    s().markSaved("C:/parts/project.3mf", "project");
    io.pickOpenFile.mockResolvedValueOnce("C:/parts/other.3mf");
    io.openProjectFile.mockResolvedValueOnce({ platform: null, warnings: [], meshes: [{ name: "Other", matrix: null, mesh: makeBox([5, 5, 5]), stats: null, edges: null }] });
    runFeature("file.open");
    await vi.waitFor(() => expect(s().parts.map((p) => p.name)).toEqual(["Other"]));
    expect(dialog.message).not.toHaveBeenCalled();
    expect(isRepairing()).toBe(false);
  });

  it("ask about repair work started while File › Open was reading before the file replaces the project", async () => {
    const id = addPart();
    s().markSaved("C:/parts/project.3mf", "project");
    const other = { platform: null, warnings: [], meshes: [{ name: "Other", matrix: null, mesh: makeBox([5, 5, 5]), stats: null, edges: null }] };
    io.pickOpenFile.mockResolvedValue("C:/parts/big.3mf");

    // Answered Cancel: the repair goes on and the open project stays.
    let read = deferred<typeof other>();
    io.openProjectFile.mockReturnValueOnce(read.promise);
    runFeature("file.open");
    await vi.waitFor(() => expect(io.openProjectFile).toHaveBeenCalledTimes(1));
    await repairMeanwhile(id);
    dialog.message.mockResolvedValueOnce("Cancel");
    read.resolve(other);
    await vi.waitFor(() => expect(dialog.message).toHaveBeenCalledTimes(1));
    await flush();
    expect(dialog.message.mock.calls[0][0]).toBe("Discard repair changes?");
    expect(isRepairing()).toBe(true);
    expect(s().parts.map((p) => p.name)).toEqual(["Part"]);

    // Answered Discard: repair mode closes, then the file replaces the project. The Open first asks about the repair
    // still open, as always; another one starts while the file is read.
    dialog.message.mockReset();
    read = deferred<typeof other>();
    io.openProjectFile.mockReturnValueOnce(read.promise);
    dialog.message.mockResolvedValueOnce("Discard");
    runFeature("file.open");
    await vi.waitFor(() => expect(io.openProjectFile).toHaveBeenCalledTimes(2));
    expect(isRepairing()).toBe(false);
    await repairMeanwhile(id);
    dialog.message.mockResolvedValueOnce("Discard");
    read.resolve(other);
    await vi.waitFor(() => expect(s().parts.map((p) => p.name)).toEqual(["Other"]));
    expect(dialog.message.mock.calls.map((call) => call[0])).toEqual(["Discard repair changes?", "Discard repair changes?"]);
    expect(isRepairing()).toBe(false);
  });

  it("ask about repair work started while New project waited for a save to finish before starting over", async () => {
    const id = addPart();
    s().markSaved("C:/parts/project.3mf", "project");
    s().addTestCube();
    const generation = s().generation;
    const write = deferred<void>();
    io.saveProjectFile.mockReturnValueOnce(write.promise);
    runFeature("file.save");
    runFeature("file.new");
    await flush();
    expect(lastToast()).toBe("Waiting for the save to finish…");
    await repairMeanwhile(id);
    dialog.message.mockResolvedValueOnce("Cancel");
    write.resolve();
    await vi.waitFor(() => expect(dialog.message).toHaveBeenCalledTimes(1));
    await flush();
    // The save wrote everything, so the unsaved-changes question is not asked; the repair one is.
    expect(dialog.message.mock.calls[0][0]).toBe("Discard repair changes?");
    expect(isRepairing()).toBe(true);
    expect(s().generation).toBe(generation);
  });
});

const CLOSED_STATUS = {
  edges: 0,
  triangles: 12,
  vertices: 8,
  shells: 1,
  borderEdges: 0,
  nonManifoldEdges: 0,
  invertedTriangles: 0,
  holes: 0,
  degenerateTriangles: 0,
  closed: true,
  oriented: true,
  volume: 8000,
  area: 2400,
};

const settle = async () => {
  await flush();
  await idle();
};

describe("Undo and Redo arrows", () => {
  const labels = (kind: "undo" | "redo") => historyMenu(kind).map((f) => f.label);

  it("list the project's steps newest first, and a row takes back that step and the newer ones", () => {
    expect(labels("undo")).toEqual(["Nothing to undo."]);
    addPart("A");
    addPart("B");
    addPart("C");
    expect(labels("undo")).toEqual(["Undo Add C", "Undo Add B", "Undo Add A"]);
    historyMenu("undo")[1].run?.();
    expect(s().parts.map((p) => p.name)).toEqual(["A"]);
    expect(labels("redo")).toEqual(["Redo Add B", "Redo Add C"]);
    historyMenu("redo")[1].run?.();
    expect(s().parts.map((p) => p.name)).toEqual(["A", "B", "C"]);
    expect(labels("redo")).toEqual(["Nothing to redo."]);
  });

  it("list repair mode's own steps while it is open, and nothing in Boolean operations", async () => {
    await enter();
    selectTriangles([0], "replace");
    ipc.runRepair.mockImplementation(async () => ({ ...changed(makeBox([5, 5, 5])), reports: [report("Added 1 point.")] }));
    runFeature("repair-ctx.insert-point");
    await settle();
    expect(labels("undo")).toEqual(["Undo Add point"]);
    historyMenu("undo")[0].run?.();
    expect(session().past).toHaveLength(0);
    expect(labels("redo")).toEqual(["Redo Add point"]);
    expect(await closeRepair()).toBe(true);

    addPart("Two");
    s().select(s().parts.map((p) => p.id));
    openBooleanMode();
    expect(labels("undo")).toEqual([CLOSE_BOOLEAN_FIRST]);
    useBoolean.setState({ session: null });
  });
});

describe("Home copy and paste", () => {
  it("copies the selected parts and pastes each copy one gap further along +X", async () => {
    ipc.checkMeshes.mockResolvedValue([{ closed: true, oriented: true, holes: 0, borderEdges: 0, nonManifoldEdges: 0, invertedTriangles: 0 }]);
    const mesh = makeBox([20, 20, 20]);
    addPart("Widget", mesh);
    runFeature("context.copy");
    expect(lastToast()).toBe("Copied 1 part.");
    expect(s().clipboard).toHaveLength(1);

    runFeature("context.paste");
    await settle();
    expect(s().parts).toHaveLength(2);
    expect(s().parts[1].matrix[12]).toBe(25);
    expect(s().parts[1].mesh).toBe(mesh);
    expect(s().selection).toEqual([s().parts[1].id]);
    expect(ipc.checkMeshes).toHaveBeenCalledTimes(1);

    runFeature("context.paste");
    expect(s().parts[2].matrix[12]).toBe(50);
    expect(s().history.past[s().history.past.length - 1]?.label).toBe("Paste Widget");
  });

  it("toasts when nothing is selected or the clipboard is empty, and keeps the clipboard", () => {
    runFeature("context.copy");
    expect(lastToast()).toBe("Select a part to repair.");
    addPart();
    runFeature("context.copy");
    s().clearSelection();
    runFeature("context.copy");
    expect(s().clipboard).toHaveLength(1);
    s().newProject();
    runFeature("context.paste");
    expect(lastToast()).toBe("Nothing to paste. Copy a part first.");
    expect(s().parts).toHaveLength(0);
  });
});

describe("Home repair commands", () => {
  const actions = () => ipc.runRepair.mock.calls.flatMap((call) => (call[1] as { type: string }[]).map((step) => step.type));

  beforeEach(() => {
    ipc.runRepair.mockImplementation(async (m: MeshData) => unchanged(m));
  });

  it("does not undo a part while Boolean operations are open", () => {
    addPart("One");
    addPart("Two");
    s().select(s().parts.map((p) => p.id));
    openBooleanMode();
    runFeature("home.undo");
    expect(s().parts).toHaveLength(2);
    expect(lastToast()).toBe(CLOSE_BOOLEAN_FIRST);
    useBoolean.setState({ session: null });
  });

  it("stays out of repair while Boolean operations are open", async () => {
    addPart("One");
    addPart("Two");
    s().select(s().parts.map((p) => p.id));
    openBooleanMode();
    runFeature("repair.shell-list");
    await flush();
    expect(isRepairing()).toBe(false);
    expect(lastToast()).toBe(CLOSE_BOOLEAN_FIRST);
    useBoolean.setState({ session: null });
  });

  it("opens repair on the selected part and reports that a closed mesh has no damage", async () => {
    const id = addPart();
    ipc.runRepair.mockImplementation(async (m: MeshData) => ({ ...unchanged(m), status: CLOSED_STATUS, highlights: null }));
    runFeature("repair.damage-warnings");
    await settle();
    expect(isRepairing()).toBe(true);
    expect(session().partId).toBe(id);
    expect(r().panelTab).toBe("status");
    expect(r().highlight).toMatchObject({ holes: true, triangles: true, degenerate: true, errors: true });
    expect(lastToast()).toBe("No damage warnings: mesh is closed and oriented.");
  });

  it("lists problem counts, opens the shell list, and shows selection info", async () => {
    await enter();
    ipc.runRepair.mockImplementation(async (m: MeshData) => ({
      ...unchanged(m),
      status: { ...CLOSED_STATUS, borderEdges: 4, holes: 1, invertedTriangles: 2 },
    }));
    runFeature("repair.problem-counts");
    await settle();
    expect(lastToast()).toBe("Border edges 4, Holes 1, Inv. orientation 2, Degenerate faces 0, Non-manifold edges 0");
    expect(r().panelTab).toBe("status");

    runFeature("repair.shell-list");
    await flush();
    expect(r().panelTab).toBe("shells");

    runFeature("repair.selection-info");
    await flush();
    expect(lastToast()).toBe("No triangles selected.");
  });

  it("asks to apply only while a repair is open, and undoes the repair or the part edit", async () => {
    addPart();
    runFeature("repair.apply");
    expect(useUi.getState().dialog).toBeNull();
    runFeature("home.undo");
    expect(s().parts).toHaveLength(0);
    runFeature("home.undo");
    expect(lastToast()).toBe("Nothing to undo.");

    await enter();
    runFeature("repair.apply");
    expect(useUi.getState().dialog).toEqual({ kind: "repair-apply" });
    useUi.getState().closeDialog();
    selectTriangles([0], "replace");
    const before = session().mesh;
    const finer = makeBox([10, 10, 10]);
    // Rust splits the triangles and leaves nothing selected.
    ipc.runRepair.mockImplementation(async () => ({ ...changed(finer), selected: true, reports: [report("Added 3 triangles.")] }));
    runFeature("repair-ctx.refine");
    await settle();
    expect(actions()).toEqual(["subdivide_triangles"]);
    expect(Array.from(ipc.runRepair.mock.calls[0][2].selection)).toEqual([0]);
    expect(session().mesh).toBe(finer);
    expect(session().selectedCount).toBe(0);
    expect(session().past).toHaveLength(1);
    expect(session().past[0].label).toBe("Subdivide triangles");
    expect(lastToast()).toBe("Added 3 triangles.");
    runFeature("home.undo");
    expect(session().mesh).toBe(before);
    expect(Array.from(maskToIds(session().selection))).toEqual([0]);
  });

  it("picks each select tool, selects damaged triangles in Rust, and hides them until asked again", async () => {
    await enter();
    for (const [id, tool] of [
      ["repair.select-primitive", "area"],
      ["repair.select-triangle", "triangle"],
      ["repair.select-surface", "surface"],
      ["repair.select-shell", "shell"],
      ["repair.select-brush", "brush"],
    ] as const) {
      runFeature(id);
      await flush();
      expect(r().selectTool).toBe(tool);
      expect(feature(id).repairActive?.(r())).toBe(true);
    }
    expect(feature("repair.select-overlap").repairActive).toBeUndefined();

    // Nothing found: the selection stays as it was.
    selectTriangles([5], "replace");
    ipc.runRepair.mockImplementation(async (m: MeshData) => ({ ...unchanged(m), reports: [report("No problem triangles found.")] }));
    runFeature("repair.select-criteria");
    await settle();
    expect(actions()).toEqual(["select_problem_triangles"]);
    expect(Array.from(ipc.runRepair.mock.calls[0][2].selection)).toEqual([]);
    expect(Array.from(maskToIds(session().selection))).toEqual([5]);
    expect(lastToast()).toBe("No problem triangles found.");

    // Found: the selection Rust sends replaces it, and no undo step is added.
    ipc.runRepair.mockImplementation(async (m: MeshData) => ({
      ...unchanged(m),
      selected: true,
      selection: new Uint32Array([1, 2]),
      reports: [report("Selected 2 problem triangles.")],
    }));
    runFeature("repair.select-criteria");
    await settle();
    expect(Array.from(maskToIds(session().selection))).toEqual([1, 2]);
    expect(session().past).toHaveLength(0);
    expect(lastToast()).toBe("Selected 2 problem triangles.");

    runFeature("repair.hide-triangles");
    await flush();
    expect(Array.from(maskToIds(session().hiddenTriangles ?? new Uint8Array(0)))).toEqual([1, 2]);
    expect(session().selectedCount).toBe(0);
    expect(lastToast()).toBe("Hidden 2 triangles. Click Hide selected triangles again to show all.");
    // A selection Rust sends skips the hidden triangles.
    ipc.runRepair.mockImplementation(async (m: MeshData) => ({ ...unchanged(m), selected: true, selection: new Uint32Array([1, 2, 3]) }));
    runFeature("repair.select-criteria");
    await settle();
    expect(Array.from(maskToIds(session().selection))).toEqual([3]);
    runFeature("repair.hide-triangles");
    await flush();
    expect(lastToast()).toBe("Hidden 1 triangles. Click Hide selected triangles again to show all.");
    runFeature("repair.hide-triangles");
    await flush();
    expect(session().hiddenTriangles).toBeNull();
    expect(lastToast()).toBe("All triangles shown.");
    runFeature("repair.hide-triangles");
    await flush();
    expect(lastToast()).toBe(SELECT_TRIANGLES_FIRST);

    useRepair.setState({
      session: { ...session(), hiddenTriangles: new Uint8Array(12).fill(1), selection: new Uint8Array(12), selectedCount: 0 },
    });
    selectAll();
    expect(session().selectedCount).toBe(0);
  });

  it("finds overlapping triangles over the whole part and highlights the crossing ones", async () => {
    await enter();
    selectTriangles([7], "replace");
    ipc.runRepair.mockImplementation(async (m: MeshData) => ({
      ...unchanged(m),
      selected: true,
      selection: new Uint32Array([0, 1]),
      intersecting: new Uint32Array([0]),
      reports: [report("Found 2 overlapping triangles.")],
    }));
    runFeature("repair.select-overlap");
    await settle();
    expect(actions()).toEqual(["find_overlaps"]);
    expect(Array.from(ipc.runRepair.mock.calls[0][2].selection)).toEqual([]);
    expect(Array.from(maskToIds(session().selection))).toEqual([0, 1]);
    expect(Array.from(session().intersecting ?? [])).toEqual([0]);
    expect(lastToast()).toBe("Found 2 overlapping triangles.");
  });

  it("runs the hole, clean-up and orientation actions, and the local edits, each on the selection", async () => {
    await enter();
    ipc.runRepair.mockImplementation(async (m: MeshData) => unchanged(m));
    const ran = async (id: string) => {
      ipc.runRepair.mockClear();
      runFeature(id);
      await settle();
      return actions();
    };
    const sent = () => Array.from(ipc.runRepair.mock.calls[0][2].selection as Uint32Array);
    expect(await ran("repair-ctx.close-all")).toEqual(["close_all_holes"]);
    expect(ipc.runRepair).toHaveBeenCalledTimes(1);
    selectTriangles([0, 1], "replace");
    expect(await ran("repair-ctx.close-defined")).toEqual(["close_defined_holes"]);
    expect(await ran("repair-ctx.close-curved")).toEqual(["close_holes_curved"]);
    expect(sent()).toEqual([0, 1]);
    selectTriangles([], "replace");
    expect(await ran("repair-ctx.close-curved")).toEqual(["close_holes_curved"]);
    expect(sent()).toEqual([]);
    expect(await ran("repair-ctx.stitch")).toEqual(["stitch_triangles"]);
    expect(await ran("repair-ctx.remove-duplicates")).toEqual(["remove_double_triangles"]);
    expect(await ran("repair-ctx.remove-degenerate")).toEqual(["remove_degenerate_faces"]);
    expect(await ran("repair-ctx.merge-points")).toEqual(["sanitize_duplicate_nodes"]);
    expect(await ran("repair-ctx.remove-tiny-shells")).toEqual(["remove_tiny_shells"]);
    expect(await ran("repair-ctx.self-intersections")).toEqual(["remove_self_intersections"]);
    expect(await ran("repair-ctx.split-non-oriented")).toEqual(["split_non_oriented_edges"]);
    expect(await ran("repair.split-non-manifold")).toEqual(["split_non_manifold_edges"]);
    expect(await ran("repair-ctx.remove-problem-areas")).toEqual(["remove_problem_areas"]);
    expect(await ran("repair-ctx.fix-flipped")).toEqual(["fix_flipped_triangles"]);
    expect(await ran("repair-ctx.invert-shells")).toEqual(["invert_negative_shells"]);
    expect(await ran("repair-ctx.wrap-surface")).toEqual(["wrap_part_surface"]);

    // The local edits: one Rust action each, on the whole part or on the selection. Bridge a gap is Soon.
    expect(await ran("repair-ctx.grow-band")).toEqual(["grow_hole_band"]);
    expect(await ran("repair-ctx.bridge-gap")).toEqual([]);
    expect(lastToast()).toBe(soonMessage(feature("repair-ctx.bridge-gap")));
    expect(await ran("repair-ctx.refine")).toEqual(["subdivide_triangles"]);
    expect(await ran("repair-ctx.smooth")).toEqual(["smooth_triangles"]);
    expect(await ran("repair-ctx.fill-three-edge")).toEqual(["add_triangle"]);
    expect(sent()).toEqual([]);
    selectTriangles([3], "replace");
    for (const [id, action] of [
      ["repair-ctx.grow-band", "grow_hole_band"],
      ["repair-ctx.refine", "subdivide_triangles"],
      ["repair-ctx.fill-three-edge", "add_triangle"],
      ["repair-ctx.insert-point", "add_point"],
      ["repair-ctx.smooth", "smooth_triangles"],
    ] as const) {
      expect(await ran(id)).toEqual([action]);
      expect(sent()).toEqual([3]);
    }

    // Edits that work on the selected triangles refuse without one, and Rust is not asked.
    selectTriangles([], "replace");
    ipc.runRepair.mockClear();
    for (const id of ["repair-ctx.insert-point", "repair.remove-triangles"]) {
      useUi.setState({ toasts: [] });
      runFeature(id);
      await flush();
      expect(lastToast()).toBe(SELECT_TRIANGLES_FIRST);
    }
    expect(ipc.runRepair).not.toHaveBeenCalled();
  });

  it("applies a Rust edit as one undo step, with nothing selected afterwards", async () => {
    await enter();
    selectTriangles([0], "replace");
    const before = session().mesh;
    const pointed = makeBox([30, 30, 30]);
    ipc.runRepair.mockImplementation(async () => ({ ...changed(pointed), selected: true, reports: [report("Added 1 point.")] }));
    runFeature("repair-ctx.insert-point");
    await settle();
    expect(session().mesh).toBe(pointed);
    expect(session().selectedCount).toBe(0);
    expect(session().past.map((p) => p.label)).toEqual(["Add point"]);
    expect(lastToast()).toBe("Added 1 point.");
    // Nothing changed: no undo step, and the selection stays.
    selectTriangles([2], "replace");
    ipc.runRepair.mockImplementation(async (m: MeshData) => ({ ...unchanged(m), reports: [report("No interior point to smooth.")] }));
    runFeature("repair-ctx.smooth");
    await settle();
    expect(session().past).toHaveLength(1);
    expect(Array.from(maskToIds(session().selection))).toEqual([2]);
    expect(lastToast()).toBe("No interior point to smooth.");
    runFeature("home.undo");
    expect(session().mesh).toBe(before);
    expect(Array.from(maskToIds(session().selection))).toEqual([0]);
  });

  it("turns Move nodes and Cut surfaces on and off from Home and the Repair tab, and a select tool ends them", async () => {
    await enter();
    runFeature("repair.move-nodes");
    await flush();
    expect(r().editTool).toBe("move-nodes");
    expect(feature("repair.move-nodes").repairActive?.(r())).toBe(true);
    expect(feature("repair.move-nodes").repairActive?.(r())).toBe(true);
    // The select tool stays picked, but is not the one taking the clicks.
    expect(feature("repair.select-triangle").repairActive?.(r())).toBe(false);
    expect(feature("repair.select-triangle").repairActive?.(r())).toBe(false);
    runFeature("repair.cut-surfaces");
    await flush();
    expect(r().editTool).toBe("cut");
    expect(feature("repair.cut-surfaces").repairActive?.(r())).toBe(true);
    runFeature("repair.cut-surfaces");
    await flush();
    expect(r().editTool).toBeNull();
    runFeature("repair.move-nodes");
    await flush();
    expect(r().editTool).toBe("move-nodes");
    runFeature("repair.select-shell");
    await flush();
    expect([r().editTool, r().selectTool]).toEqual([null, "shell"]);
    // Leaving repair mode ends the tools.
    runFeature("repair.cut-surfaces");
    await flush();
    expect(await closeRepair()).toBe(true);
    expect(r().editTool).toBeNull();
    expect(ipc.runRepair).not.toHaveBeenCalled();
  });

  it("moves a node in Rust as one undo step, and the selection stays", async () => {
    await enter();
    selectTriangles([2], "replace");
    const moved = makeBox([30, 30, 30]);
    ipc.runRepair.mockImplementation(async () => ({ ...changed(moved, new Uint32Array([2])), reports: [report("Moved the point to 1.0000, 2.0000, 3.0000.")] }));
    expect(await moveNode(4, [1, 2, 3])).toBe(true);
    expect(ipc.runRepair.mock.calls[0][1]).toEqual([{ type: "move_node", enabled: true, params: { vertex: 4, x: 1, y: 2, z: 3 } }]);
    expect(session().mesh).toBe(moved);
    expect(session().past.map((p) => p.label)).toEqual(["Move nodes"]);
    expect(Array.from(maskToIds(session().selection))).toEqual([2]);
    // Every drag would toast otherwise: the tooltip already shows where the point went.
    expect(toasts()).toEqual([]);
    ipc.runRepair.mockImplementation(async (m: MeshData) => unchanged(m));
    expect(await moveNode(4, [1, 2, 3])).toBe(false);
    expect(session().past).toHaveLength(1);
  });

  it("collects cut points in the insert order, has Rust route the line, and cuts along it as one undo step", async () => {
    await enter();
    runFeature("repair.cut-surfaces");
    await flush();
    ipc.fetchCutPath.mockImplementation(async (_mesh: MeshData, points: { at: number[] }[]) => ({
      pieces: points.slice(1).map(() => new Float32Array(6)),
      spans: points.slice(1).map((_, i) => [i, i + 1]),
      anchors: points.map((p) => p.at),
    }));
    const at = () => useCut.getState().points.map((p) => p.at);
    addCutPoint([0, 0, 20], 4);
    addCutPoint([10, 0, 20], 5);
    addCutPoint([10, 10, 20], 6);
    expect(at()).toEqual([[0, 0, 20], [10, 0, 20], [10, 10, 20]]);
    expect(useCut.getState().current).toBe(2);
    await vi.waitFor(() => expect(useCut.getState().routedFor?.points).toBe(useCut.getState().points));
    const routed = ipc.fetchCutPath.mock.calls[ipc.fetchCutPath.mock.calls.length - 1];
    expect(routed[0]).toBe(session().mesh);
    expect(routed[1].map((p: { triangle: number }) => p.triangle)).toEqual([4, 5, 6]);
    expect(routed[2]).toEqual({ smooth: false, step: 2, snap: 0.5, closed: false });
    // At last position: right after the current point.
    stepCurrentPoint(-1);
    addCutPoint([5, 5, 20], 7);
    expect(at()[2]).toEqual([5, 5, 20]);
    expect(useCut.getState().current).toBe(2);
    undoLastPoint();
    expect(at()).toEqual([[0, 0, 20], [10, 0, 20], [10, 10, 20]]);
    expect(useCut.getState().current).toBe(1);
    // At nearest position: where it lengthens the line least.
    setCutSettings({ order: "nearest" });
    addCutPoint([5, -0.5, 20], 8);
    expect(at()[1]).toEqual([5, -0.5, 20]);

    const cut = makeBox([40, 40, 40]);
    ipc.runRepair.mockImplementation(async () => ({ ...changed(cut), selected: true, reports: [report("Cut the surface open along 9 edges, adding 6 triangles.")] }));
    setCutSettings({ snap: 0.25, closed: true });
    await executeCut();
    const [, steps, opts] = ipc.runRepair.mock.calls[0];
    expect(steps).toEqual([
      {
        type: "cut_surface",
        enabled: true,
        params: {
          points: [
            [0, 0, 20, 4],
            [5, -0.5, 20, 8],
            [10, 0, 20, 5],
            [10, 10, 20, 6],
          ],
          smooth: false,
          step: 2,
          snap: 0.25,
          closed: true,
        },
      },
    ]);
    // The status comes along even with Auto-update off, so the cut shows as holes.
    expect(opts.withStatus).toBe(true);
    expect(session().mesh).toBe(cut);
    expect(session().past.map((p) => p.label)).toEqual(["Cut surfaces"]);
    expect(lastToast()).toBe("Cut the surface open along 9 edges, adding 6 triangles.");
    // The points named the old working copy's triangles; the mode stays on for the next line.
    expect(useCut.getState().points).toEqual([]);
    expect(r().editTool).toBe("cut");
    await executeCut();
    expect(lastToast()).toBe("Click at least two points on the surface to cut along.");
    expect(ipc.runRepair).toHaveBeenCalledTimes(1);
  });

  it("refuses triangle edits with no selection, and extracts the selected shell", async () => {
    await enter();
    runFeature("repair.flip-selected");
    await flush();
    expect(lastToast()).toBe(SELECT_TRIANGLES_FIRST);
    selectTriangles([0], "replace");
    ipc.runRepair.mockClear();
    runFeature("repair.flip-selected");
    await settle();
    expect(actions()).toEqual(["flip_selected_triangles"]);

    ipc.runRepair.mockClear();
    selectTriangles([0], "replace");
    runFeature("repair.remove-triangles");
    await settle();
    expect(actions()).toEqual(["remove_selected_triangles"]);

    const before = s().parts.length;
    selectTriangles([0], "replace");
    runFeature("repair.extract-copy");
    await flush();
    expect(s().parts).toHaveLength(before + 1);
    expect(lastToast()).toMatch(/Added "Part \(extracted\)"/);

    selectTriangles([0], "replace");
    runFeature("repair.extract-shells");
    await flush();
    expect(s().parts).toHaveLength(before + 2);

    s().newProject();
    useRepair.setState({ session: null, busy: null });
    runFeature("repair.extract-shells");
    await flush();
    expect(lastToast()).toBe("Select a part to repair.");
  });

  it("opens the script picker and the scripts tab, and repairs every unlocked part in one step", async () => {
    await enter();
    runFeature("home.run-repair-script");
    await flush();
    expect(useUi.getState().dialog).toEqual({ kind: "repair-script-picker" });
    useUi.getState().closeDialog();
    runFeature("repair.edit-scripts");
    await flush();
    expect(r().panelTab).toBe("scripts");

    expect(await closeRepair()).toBe(true);
    s().newProject();
    useRepair.setState({
      session: null,
      catalog: {
        ...catalog,
        scripts: [{ id: "builtin.default", name: "Default repair", builtIn: true, actions: [{ type: "close_trivial_holes", enabled: true, params: {} }] }],
      },
    });
    const first = addPart("One");
    const second = addPart("Two");
    s().setPartLocked(second, true);
    s().select([]);
    const repaired = makeBox([4, 4, 4]);
    ipc.runRepair.mockImplementation(async () => changed(repaired));
    runFeature("context.batch-repair");
    await settle();
    expect(s().parts.find((p) => p.name === "One")?.mesh).toBe(repaired);
    expect(s().parts.find((p) => p.name === "Two")?.mesh).not.toBe(repaired);
    expect(lastToast()).toBe("Repaired 1 part.");
    expect(s().history.past[s().history.past.length - 1]?.label).toBe("Repair One");

    s().undo();
    s().setPartLocked(first, true);
    ipc.runRepair.mockClear();
    runFeature("context.batch-repair");
    await settle();
    expect(lastToast()).toBe("Locked parts were left as they are.");
    expect(ipc.runRepair).not.toHaveBeenCalled();
  });
});
