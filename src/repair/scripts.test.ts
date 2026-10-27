import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ActionSpec, Catalog, ParamSpec, RepairScript, ScriptStep, StoredScripts } from "./types";

const ipc = vi.hoisted(() => ({
  fetchCatalog: vi.fn(),
  loadStoredScripts: vi.fn(),
  saveStoredScripts: vi.fn(),
  importScriptFile: vi.fn(),
  exportScriptFile: vi.fn(),
}));
vi.mock("./ipc", async (importOriginal) => ({ ...(await importOriginal<typeof import("./ipc")>()), ...ipc }));

import {
  addStep,
  canAddAction,
  cleanStored,
  copyScript,
  createScript,
  defaultParams,
  enabledSteps,
  moveInOrder,
  moveStep,
  newScriptId,
  normalizeParam,
  normalizeStep,
  orderedScripts,
  removeStep,
  renameScript,
  sameScript,
  scriptActions,
  scriptProblems,
  setStepParam,
  toggleStep,
  uniqueName,
} from "./scripts";

// ---------------------------------------------------------------------------------------------
// The catalog as Rust's defaults::catalog() serialises it (src-tauri/src/repair/defaults.rs).

const mm = (id: string, label: string, decimals: number, value: number): ParamSpec => ({ id, label, kind: "float", unit: "mm", decimals, min: 0, default: value });
const yesNo = (id: string, label: string, value: boolean): ParamSpec => ({ id, label, kind: "bool", default: value });
const spec = (id: ActionSpec["id"], label: string, over: Partial<ActionSpec> = {}): ActionSpec => ({
  id,
  label,
  implemented: true,
  inScripts: true,
  needsSelection: false,
  hint: `${label}.`,
  params: [],
  ...over,
});
const soon = { implemented: false };
const actionsOnly = { inScripts: false };

const ACTIONS: ActionSpec[] = [
  spec("close_all_holes", "Close all holes", { repairType: 4 }),
  spec("close_trivial_holes", "Close trivial holes", { repairType: 3 }),
  spec("fix_flipped_triangles", "Fix flipped triangles", { repairType: 2 }),
  spec("invert_negative_shells", "Invert negative shells"),
  spec("prefix_suffix", "Prefix-Suffix", soon),
  spec("reduce_triangles", "Reduce triangles", soon),
  spec("refine_triangles", "Refine triangles", soon),
  spec("remesh", "Remesh", soon),
  spec("remove_degenerate_faces", "Remove degenerate faces", {
    repairType: 6,
    params: [yesNo("calc_tolerance", "Calculate tolerance by bounding box", true), mm("tolerance", "Tolerance", 4, 0.002)],
  }),
  spec("remove_double_triangles", "Remove double triangles", { repairType: 5 }),
  spec("remove_problem_areas", "Remove problem areas"),
  spec("remove_self_intersections", "Remove self-intersections"),
  spec("remove_tiny_shells", "Remove tiny shells", {
    repairType: 7,
    params: [
      {
        id: "mode",
        label: "Tiny-shell detection",
        kind: "choice",
        default: "closed_volume_open_area",
        choices: [
          { id: "closed_volume_open_area", label: "Delete closed shells below minimum volume and open shells below minimum area" },
          { id: "volume", label: "Delete shells below minimum volume" },
          { id: "area", label: "Delete shells below minimum area" },
        ],
      },
      { ...mm("min_volume", "Minimum volume", 4, 0.0001), unit: "mm³" },
      { ...mm("min_area", "Minimum area", 4, 0.0001), unit: "mm²" },
    ],
  }),
  spec("sanitize_duplicate_nodes", "Sanitize duplicate nodes", { params: [mm("tolerance", "Tolerance", 4, 0.0001)] }),
  spec("scale", "Scale", soon),
  spec("smooth_triangles", "Smooth triangles"),
  spec("split_non_manifold_edges", "Split non-manifold edges"),
  spec("split_off_self_intersections", "Split-off self-intersections"),
  spec("stitch_triangles", "Stitch triangles", { repairType: 1, params: [mm("tolerance", "Tolerance", 3, 0.01), yesNo("ignore_orientation", "Ignore orientation", false)] }),
  spec("wrap_part_surface", "Wrap part surface", { repairType: 13 }),
  spec("z_compensation", "Z-compensation", soon),
  spec("close_defined_holes", "Close defined holes", { ...actionsOnly, needsSelection: true }),
  spec("split_non_oriented_edges", "Split non-oriented edges", actionsOnly),
  spec("detect_self_intersections", "Detect self-intersections", actionsOnly),
  spec("remove_selected_triangles", "Remove selected triangles", { ...actionsOnly, needsSelection: true }),
  spec("flip_selected_triangles", "Flip selected triangles", { ...actionsOnly, needsSelection: true }),
];

const step = (type: ScriptStep["type"], params: ScriptStep["params"] = {}, enabled = true): ScriptStep => ({ type, enabled, params });
const stitch = (ignore: boolean) => step("stitch_triangles", { tolerance: 0.01, ignore_orientation: ignore });
const defaultSteps = [
  stitch(false),
  step("close_trivial_holes"),
  step("remove_double_triangles"),
  step("fix_flipped_triangles"),
  step("close_trivial_holes"),
  stitch(true),
  step("fix_flipped_triangles"),
  step("close_all_holes"),
  step("remove_degenerate_faces", { calc_tolerance: true, tolerance: 0.002 }),
  step("remove_tiny_shells", { mode: "closed_volume_open_area", min_volume: 0.0001, min_area: 0.0001 }),
];
const BUILTINS: RepairScript[] = [
  { id: "builtin.simple", name: "Simple repair", builtIn: true, actions: [step("fix_flipped_triangles"), step("close_trivial_holes"), step("close_all_holes")] },
  { id: "builtin.default", name: "Default repair", builtIn: true, actions: defaultSteps },
  { id: "builtin.extended", name: "Extended repair", builtIn: true, actions: [...defaultSteps.slice(0, 8), step("wrap_part_surface"), ...defaultSteps.slice(8)] },
];
const catalog: Catalog = { actions: ACTIONS, scripts: BUILTINS };

const own = (id: string, name: string, actions: ScriptStep[] = []): RepairScript => ({ id, name, builtIn: false, actions });
const ids = (scripts: RepairScript[]) => scripts.map((s) => s.id);

// ---------------------------------------------------------------------------------------------

describe("catalog lookups", () => {
  it("list the add-action dropdown in catalog order, with Soon actions that cannot be added", () => {
    const listed = scriptActions(catalog);
    expect(listed.map((a) => a.label)).toEqual([
      "Close all holes",
      "Close trivial holes",
      "Fix flipped triangles",
      "Invert negative shells",
      "Prefix-Suffix",
      "Reduce triangles",
      "Refine triangles",
      "Remesh",
      "Remove degenerate faces",
      "Remove double triangles",
      "Remove problem areas",
      "Remove self-intersections",
      "Remove tiny shells",
      "Sanitize duplicate nodes",
      "Scale",
      "Smooth triangles",
      "Split non-manifold edges",
      "Split-off self-intersections",
      "Stitch triangles",
      "Wrap part surface",
      "Z-compensation",
    ]);
    expect(listed.filter(canAddAction).length).toBe(15);
    expect(canAddAction(undefined)).toBe(false);
    expect(defaultParams(ACTIONS[18])).toEqual({ tolerance: 0.01, ignore_orientation: false });
  });
});

describe("scripts", () => {
  it("are listed in picker order: by the stored order, then unlisted ones in their own sequence, built-ins first", () => {
    const mine = [own("own.1", "A"), own("own.2", "B")];
    expect(ids(orderedScripts(catalog, mine, []))).toEqual(["builtin.simple", "builtin.default", "builtin.extended", "own.1", "own.2"]);
    expect(ids(orderedScripts(catalog, mine, ["own.2", "builtin.extended", "gone", "own.1"]))).toEqual([
      "own.2",
      "builtin.extended",
      "own.1",
      "builtin.simple",
      "builtin.default",
    ]);
    expect(ids(orderedScripts(null, mine, ["own.2"]))).toEqual(["own.2", "own.1"]);
  });

  it("move up and down in the picker order, and stay put at either end", () => {
    const all = orderedScripts(catalog, [own("own.1", "A")], []);
    expect(moveInOrder(all, "own.1", -1)).toEqual(["builtin.simple", "builtin.default", "own.1", "builtin.extended"]);
    expect(moveInOrder(all, "builtin.simple", 1)).toEqual(["builtin.default", "builtin.simple", "builtin.extended", "own.1"]);
    expect(moveInOrder(all, "builtin.simple", -1)).toEqual(ids(all));
    expect(moveInOrder(all, "own.1", 1)).toEqual(ids(all));
    expect(moveInOrder(all, "nope", 1)).toEqual(ids(all));
  });

  it("get fresh ids and unique names", () => {
    expect(newScriptId([], 1700)).toBe("own.1700");
    expect(newScriptId(["own.1700", "own.1700-2"], 1700)).toBe("own.1700-3");
    expect(uniqueName("Mine", ["Mine", "Mine 2"])).toBe("Mine 4".replace("4", "3"));
    expect(uniqueName("Other", ["Mine"])).toBe("Other");
  });

  it("are created empty and own, renamed with a trimmed name, and copied with their own steps", () => {
    const fresh = createScript("  ", BUILTINS, 5);
    expect(fresh).toEqual({ id: "own.5", name: "New script", builtIn: false, actions: [] });
    expect(createScript("New script", [...BUILTINS, fresh], 5).name).toBe("New script 2");
    expect(renameScript(fresh, "  Fast fix ")).toEqual({ ...fresh, name: "Fast fix" });
    expect(renameScript(fresh, "   ")).toBe(fresh);
    expect(renameScript(fresh, "New script")).toBe(fresh);

    const copy = copyScript(BUILTINS[2], BUILTINS, undefined, 9);
    expect(copy.id).toBe("own.9");
    expect(copy.name).toBe("Extended repair (copy)");
    expect(copy.builtIn).toBe(false);
    expect(copy.actions).toEqual(BUILTINS[2].actions);
    copy.actions[0].params.tolerance = 5;
    expect(BUILTINS[2].actions[0].params.tolerance).toBe(0.01);
    expect(copyScript(BUILTINS[0], [...BUILTINS, own("own.1", "Mine")], " Mine ").name).toBe("Mine 2");
  });

  it("compare by name and steps", () => {
    const a = own("own.1", "A", [stitch(false)]);
    expect(sameScript(a, { ...a, id: "own.2", actions: [stitch(false)] })).toBe(true);
    expect(sameScript(a, { ...a, name: "B" })).toBe(false);
    expect(sameScript(a, { ...a, actions: [stitch(true)] })).toBe(false);
    expect(sameScript(a, { ...a, actions: [{ ...stitch(false), enabled: false }] })).toBe(false);
    expect(sameScript(a, { ...a, actions: [step("stitch_triangles", { tolerance: 0.01 })] })).toBe(false);
    expect(sameScript(a, { ...a, actions: [step("stitch_triangles", { tolerance: 0.01, other: false })] })).toBe(false);
  });
});

describe("steps", () => {
  const base = own("own.1", "Mine", [step("close_trivial_holes"), stitch(false), step("close_all_holes")]);

  it("are added at the catalog defaults, but never Soon, unknown or selection-only actions", () => {
    const added = addStep(base, "remove_tiny_shells", catalog);
    expect(added.actions[3]).toEqual(step("remove_tiny_shells", { mode: "closed_volume_open_area", min_volume: 0.0001, min_area: 0.0001 }));
    expect(addStep(base, "remesh", catalog)).toBe(base);
    expect(addStep(base, "close_defined_holes", catalog)).toBe(base);
    expect(addStep(base, "bogus" as ScriptStep["type"], catalog)).toBe(base);
  });

  it("are removed, moved and toggled, and an index out of range changes nothing", () => {
    expect(removeStep(base, 1).actions.map((s) => s.type)).toEqual(["close_trivial_holes", "close_all_holes"]);
    expect(removeStep(base, 3)).toBe(base);
    expect(moveStep(base, 0, 2).actions.map((s) => s.type)).toEqual(["stitch_triangles", "close_all_holes", "close_trivial_holes"]);
    expect(moveStep(base, 2, 0).actions.map((s) => s.type)).toEqual(["close_all_holes", "close_trivial_holes", "stitch_triangles"]);
    expect(moveStep(base, 2, 99).actions.map((s) => s.type)).toEqual(base.actions.map((s) => s.type));
    expect(moveStep(base, 0, -5)).toBe(base);
    expect(moveStep(base, 7, 0)).toBe(base);
    const off = toggleStep(base, 1);
    expect(off.actions[1].enabled).toBe(false);
    expect(toggleStep(off, 1).actions[1].enabled).toBe(true);
    expect(toggleStep(base, -1)).toBe(base);
    expect(enabledSteps(off).map((s) => s.type)).toEqual(["close_trivial_holes", "close_all_holes"]);
    // The input never changes.
    expect(base.actions.map((s) => s.enabled)).toEqual([true, true, true]);
  });

  it("normalise parameter values: rounded to the shown decimals, clamped to the minimum, typed, else the default", () => {
    const tolerance = ACTIONS[18].params[0];
    expect(normalizeParam(tolerance, 0.01234)).toBe(0.012);
    expect(normalizeParam(tolerance, 0.0104)).toBe(0.01);
    expect(normalizeParam(tolerance, 0.0106)).toBe(0.011);
    expect(normalizeParam(tolerance, -3)).toBe(0);
    expect(Object.is(normalizeParam(tolerance, -0.0001), 0)).toBe(true);
    expect(normalizeParam(tolerance, "0,5 mm")).toBe(0.5);
    expect(normalizeParam(tolerance, "abc")).toBe(0.01);
    expect(normalizeParam(tolerance, Number.NaN)).toBe(0.01);
    expect(normalizeParam(tolerance, Infinity)).toBe(0.01);
    expect(normalizeParam(tolerance, true)).toBe(0.01);
    const ignore = ACTIONS[18].params[1];
    expect(normalizeParam(ignore, true)).toBe(true);
    expect(normalizeParam(ignore, "true")).toBe(false);
    const mode = ACTIONS[12].params[0];
    expect(normalizeParam(mode, "area")).toBe("area");
    expect(normalizeParam(mode, "everything")).toBe("closed_volume_open_area");
  });

  it("set one parameter of one step, normalised, and change nothing for unknown parameters or the same value", () => {
    const set = setStepParam(base, 1, "tolerance", 0.0256, catalog);
    expect(set.actions[1].params).toEqual({ tolerance: 0.026, ignore_orientation: false });
    expect(base.actions[1].params.tolerance).toBe(0.01);
    expect(setStepParam(base, 1, "tolerance", 0.01, catalog)).toBe(base);
    expect(setStepParam(base, 1, "speed", 3, catalog)).toBe(base);
    expect(setStepParam(base, 0, "tolerance", 3, catalog)).toBe(base);
    expect(setStepParam(base, 9, "tolerance", 3, catalog)).toBe(base);
    expect(setStepParam(base, 1, "ignore_orientation", true, catalog).actions[1].params.ignore_orientation).toBe(true);
  });

  it("are normalised to exactly their action's parameters", () => {
    expect(normalizeStep(step("stitch_triangles", { tolerance: "0.02", junk: 1 }), catalog)).toEqual(
      step("stitch_triangles", { tolerance: 0.02, ignore_orientation: false }),
    );
    expect(normalizeStep(step("close_all_holes", { junk: 1 }, false), catalog)).toEqual(step("close_all_holes", {}, false));
    const unknown = step("bogus" as ScriptStep["type"], { a: 1 });
    expect(normalizeStep(unknown, catalog)).toBe(unknown);
  });
});

describe("validation", () => {
  it("passes every built-in script", () => {
    for (const s of BUILTINS) expect(scriptProblems(s, catalog)).toEqual([]);
  });

  it("names unknown actions wherever they are, and Soon, selection-only or badly set actions when enabled", () => {
    const script = own("own.1", "Bad", [
      step("bogus" as ScriptStep["type"], {}, false),
      step("remesh"),
      step("remesh", {}, false),
      step("flip_selected_triangles"),
      step("stitch_triangles", { tolerance: -1, ignore_orientation: "yes" }),
      step("remove_tiny_shells", { mode: "all" }),
      step("stitch_triangles", { tolerance: -1 }, false),
      step("sanitize_duplicate_nodes", {}),
    ]);
    expect(scriptProblems(script, catalog)).toEqual([
      "Step 1 uses an action tricky3D does not know (bogus).",
      "Step 2, Remesh, is not available yet.",
      "Step 4, Flip selected triangles, needs selected triangles, which a script does not have.",
      "Step 5, Stitch triangles: Tolerance must be a number of 0 mm or more.",
      "Step 5, Stitch triangles: Ignore orientation must be Yes or No.",
      "Step 6, Remove tiny shells: Tiny-shell detection is not one of its choices.",
    ]);
  });

  it("cleans what the config file holds", () => {
    const stored = {
      scripts: [
        own("own.1", "Mine", [step("stitch_triangles", { tolerance: 0.5 })]),
        own("own.1", "Duplicate"),
        { ...own("builtin.extended", "Fake"), builtIn: true },
        { id: "own.2", name: "  ", builtIn: true, actions: [{ type: "close_all_holes" }] } as unknown as RepairScript,
      ],
      order: ["own.2", "own.2", "builtin.simple", 5],
    } as unknown as StoredScripts;
    expect(cleanStored(stored, catalog)).toEqual({
      scripts: [own("own.1", "Mine", [step("stitch_triangles", { tolerance: 0.5, ignore_orientation: false })]), own("own.2", "New script", [step("close_all_holes")])],
      order: ["own.2", "builtin.simple"],
    });
    expect(cleanStored(null, catalog)).toEqual({ scripts: [], order: [] });
  });
});

// ---------------------------------------------------------------------------------------------
// Loading and saving. Each test gets fresh modules: the load and save state is per app start.

async function fresh() {
  vi.resetModules();
  const scripts = await import("./scripts");
  const { useRepair } = await import("./store");
  const { useUi } = await import("../ui");
  return { scripts, useRepair, toasts: () => useUi.getState().toasts.map((t) => [t.kind, t.message]) };
}

describe("loading and saving", () => {
  beforeEach(() => {
    vi.stubGlobal("window", { setTimeout: () => 0 });
    vi.stubGlobal("localStorage", undefined);
    vi.stubGlobal("isTauri", true);
    for (const f of Object.values(ipc)) f.mockReset();
    ipc.fetchCatalog.mockResolvedValue(catalog);
    ipc.saveStoredScripts.mockResolvedValue(undefined);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  /** What the last write (or the one `back` writes before it) sent. */
  const saved = (back = 0): StoredScripts => {
    const calls = ipc.saveStoredScripts.mock.calls;
    return calls[calls.length - 1 - back]?.[0];
  };

  it("load the catalog and the user's scripts once, sharing one load between callers", async () => {
    const { scripts, useRepair } = await fresh();
    ipc.loadStoredScripts.mockResolvedValue({ scripts: [own("own.1", "Mine", [step("close_all_holes")])], order: ["own.1"] });
    const [a, b] = await Promise.all([scripts.loadScripts(), scripts.loadScripts()]);
    expect([a, b]).toEqual([true, true]);
    expect(await scripts.loadScripts()).toBe(true);
    expect(ipc.fetchCatalog).toHaveBeenCalledTimes(1);
    expect(useRepair.getState().catalog).toBe(catalog);
    expect(ids(useRepair.getState().ownScripts)).toEqual(["own.1"]);
    expect(ids(scripts.allScripts())).toEqual(["own.1", "builtin.simple", "builtin.default", "builtin.extended"]);
    expect(scripts.scriptById("builtin.default")?.name).toBe("Default repair");
    expect(scripts.scriptById(null)).toBeUndefined();
  });

  it("try again after a failed load, and say why it failed", async () => {
    const { scripts, useRepair, toasts } = await fresh();
    ipc.fetchCatalog.mockRejectedValueOnce("The repair catalog is unavailable.");
    expect(await scripts.loadScripts()).toBe(false);
    expect(toasts()).toEqual([["error", "The repair catalog is unavailable."]]);
    ipc.loadStoredScripts.mockResolvedValue(null);
    expect(await scripts.loadScripts()).toBe(true);
    expect(useRepair.getState().catalog).toBe(catalog);
  });

  it("refuse outside the desktop app", async () => {
    vi.stubGlobal("isTauri", false);
    const { scripts } = await fresh();
    expect(await scripts.loadScripts()).toBe(false);
    expect(ipc.fetchCatalog).not.toHaveBeenCalled();
  });

  it("never overwrite a scripts file that could not be read", async () => {
    const { scripts, useRepair, toasts } = await fresh();
    ipc.loadStoredScripts.mockRejectedValue("Could not read repair-scripts.json: it is not valid JSON.");
    expect(await scripts.loadScripts()).toBe(true);
    expect(useRepair.getState().catalog).toBe(catalog);
    const created = await scripts.createOwnScript("Mine");
    expect(created?.name).toBe("Mine");
    expect(ipc.saveStoredScripts).not.toHaveBeenCalled();
    expect(toasts()).toEqual([
      ["error", "Could not read repair-scripts.json: it is not valid JSON."],
      ["warn", "Your repair scripts file could not be read, so changes to scripts are not saved."],
    ]);
  });

  it("create, save, rename, copy, move and delete own scripts, writing every script id in picker order", async () => {
    const { scripts, useRepair } = await fresh();
    ipc.loadStoredScripts.mockResolvedValue(null);
    const created = await scripts.createOwnScript("");
    if (!created) throw new Error("not created");
    expect(created.name).toBe("New script");
    expect(useRepair.getState().editingScriptId).toBe(created.id);
    expect(saved()).toEqual({ scripts: [created], order: ["builtin.simple", "builtin.default", "builtin.extended", created.id] });

    // Save: edited steps are normalised against the catalog.
    const edited = { ...created, actions: [step("stitch_triangles", { tolerance: 0.0234 }), step("close_all_holes")] };
    expect(await scripts.saveScript(edited)).toBe(true);
    expect(saved().scripts[0].actions[0]).toEqual(step("stitch_triangles", { tolerance: 0.023, ignore_orientation: false }));
    expect(await scripts.saveScript(BUILTINS[0])).toBe(false);

    expect(await scripts.renameOwnScript(created.id, " Quick ")).toBe(true);
    expect(saved().scripts[0].name).toBe("Quick");
    expect(await scripts.renameOwnScript("builtin.simple", "Hacked")).toBe(false);

    const copy = await scripts.saveScriptAsCopy(BUILTINS[0]);
    if (!copy) throw new Error("not copied");
    expect(copy.name).toBe("Simple repair (copy)");
    expect(useRepair.getState().editingScriptId).toBe(copy.id);
    expect(saved().order).toEqual(["builtin.simple", copy.id, "builtin.default", "builtin.extended", created.id]);

    await scripts.moveScript(created.id, -1);
    expect(saved().order).toEqual(["builtin.simple", copy.id, "builtin.default", created.id, "builtin.extended"]);
    const writes = ipc.saveStoredScripts.mock.calls.length;
    await scripts.moveScript("builtin.simple", -1);
    expect(ipc.saveStoredScripts.mock.calls.length).toBe(writes);

    // Deleting the script shown in the tab shows the one before it.
    useRepair.getState().setEditingScriptId(created.id);
    expect(await scripts.deleteOwnScript(created.id)).toBe(true);
    expect(useRepair.getState().editingScriptId).toBe("builtin.default");
    expect(saved()).toEqual({ scripts: [copy], order: ["builtin.simple", copy.id, "builtin.default", "builtin.extended"] });
    expect(await scripts.deleteOwnScript("builtin.extended")).toBe(false);
    expect(ids(scripts.allScripts())).toEqual(["builtin.simple", copy.id, "builtin.default", "builtin.extended"]);
  });

  it("come back after a restart exactly as saved: create, add actions, reorder, toggle, save, reload", async () => {
    const first = await fresh();
    ipc.loadStoredScripts.mockResolvedValue(null);
    const created = await first.scripts.createOwnScript("Shop repair");
    if (!created) throw new Error("not created");
    let draft = addStep(created, "stitch_triangles", catalog);
    draft = addStep(draft, "close_all_holes", catalog);
    draft = addStep(draft, "wrap_part_surface", catalog);
    draft = moveStep(draft, 2, 0);
    draft = toggleStep(draft, 2);
    draft = setStepParam(draft, 1, "tolerance", 0.05, catalog);
    expect(await first.scripts.saveScript(draft)).toBe(true);
    await first.scripts.moveScript(created.id, -1);
    const file = saved();

    const second = await fresh();
    ipc.loadStoredScripts.mockResolvedValue(JSON.parse(JSON.stringify(file)));
    expect(await second.scripts.loadScripts()).toBe(true);
    const back = second.scripts.scriptById(created.id);
    expect(back).toEqual(draft);
    expect(back?.actions.map((s) => [s.type, s.enabled])).toEqual([
      ["wrap_part_surface", true],
      ["stitch_triangles", true],
      ["close_all_holes", false],
    ]);
    expect(back?.actions[1].params).toEqual({ tolerance: 0.05, ignore_orientation: false });
    expect(ids(second.scripts.allScripts())).toEqual(["builtin.simple", "builtin.default", created.id, "builtin.extended"]);
  });

  it("write one save after another, each with the latest scripts", async () => {
    const { scripts } = await fresh();
    ipc.loadStoredScripts.mockResolvedValue(null);
    await scripts.loadScripts();
    let release!: () => void;
    ipc.saveStoredScripts.mockImplementationOnce(() => new Promise<void>((resolve) => (release = resolve)));
    const first = scripts.createOwnScript("A");
    await vi.waitFor(() => expect(ipc.saveStoredScripts).toHaveBeenCalledTimes(1));
    const second = scripts.createOwnScript("B");
    await Promise.resolve();
    expect(ipc.saveStoredScripts).toHaveBeenCalledTimes(1);
    release();
    await Promise.all([first, second]);
    expect(ipc.saveStoredScripts).toHaveBeenCalledTimes(2);
    expect(saved().scripts.map((s) => s.name)).toEqual(["A", "B"]);
  });

  it("import a script as an own script with a fresh id when its id is taken, and show its warnings", async () => {
    const { scripts, useRepair, toasts } = await fresh();
    ipc.loadStoredScripts.mockResolvedValue({ scripts: [own("own.1", "Mine")], order: [] });
    ipc.importScriptFile.mockResolvedValueOnce(null);
    expect(await scripts.importScript()).toBeNull();
    ipc.importScriptFile.mockResolvedValueOnce({
      script: { id: "own.1", name: "Mine", builtIn: false, actions: [stitch(false), step("wrap_part_surface")] },
      warnings: ["Skipped action type 9, which tricky3D does not know."],
    });
    const imported = await scripts.importScript();
    expect(imported?.id).not.toBe("own.1");
    expect(imported?.id.startsWith("own.")).toBe(true);
    expect(useRepair.getState().editingScriptId).toBe(imported?.id);
    expect(saved().scripts.map((s) => s.name)).toEqual(["Mine", "Mine 2"]);
    expect(toasts()).toEqual([
      ["warn", "Skipped action type 9, which tricky3D does not know."],
      ["info", 'Imported "Mine 2".'],
    ]);
    ipc.importScriptFile.mockRejectedValueOnce("Could not read \"x.repair\": it is not a repair list.");
    expect(await scripts.importScript()).toBeNull();
  });

  it("import a script under a name no other script has, built-ins included", async () => {
    const { scripts, toasts } = await fresh();
    ipc.loadStoredScripts.mockResolvedValue(null);
    // The intended flow: Extended repair exported from the reference tool, imported next to the built-in one.
    const file = { script: { id: "own.9", name: " Extended repair ", builtIn: false, actions: [step("wrap_part_surface")] }, warnings: [] };
    ipc.importScriptFile.mockResolvedValueOnce(file).mockResolvedValueOnce(file);
    expect((await scripts.importScript())?.name).toBe("Extended repair 2");
    // The same file again gets the next free name.
    expect((await scripts.importScript())?.name).toBe("Extended repair 3");
    expect(saved().scripts.map((s) => s.name)).toEqual(["Extended repair 2", "Extended repair 3"]);
    expect(toasts()).toEqual([
      ["info", 'Imported "Extended repair 2".'],
      ["info", 'Imported "Extended repair 3".'],
    ]);
  });

  it("export a script and say so, or say why not", async () => {
    const { scripts, toasts } = await fresh();
    ipc.exportScriptFile.mockResolvedValueOnce(true).mockResolvedValueOnce(false).mockRejectedValueOnce("Could not write \"Mine.json\".");
    await scripts.exportScript(own("own.1", "Mine"));
    await scripts.exportScript(own("own.1", "Mine"));
    await scripts.exportScript(own("own.1", "Mine"));
    expect(toasts()).toEqual([
      ["info", 'Exported "Mine".'],
      ["error", 'Could not write "Mine.json".'],
    ]);
  });
});
