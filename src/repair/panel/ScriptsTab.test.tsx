import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { moveStep, removeStep, setStepParam, toggleStep, addStep } from "../scripts";
import type { ActionReport, ActionSpec, Catalog, ParamSpec, RepairScript, ScriptStep } from "../types";
import {
  LastRun,
  StepList,
  carryKeys,
  editedScript,
  executeBlocked,
  insertionIndex,
  moveTarget,
  nextEdit,
  onEmptyStepsKeyDown,
  optionLabel,
  paramText,
  runTime,
  saveLabel,
  scriptInfo,
  stepKeys,
  type ScriptEdit,
} from "./ScriptsTab";

// The parts of Rust's catalog (src-tauri/src/repair/defaults.rs) these tests touch.
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
const catalog: Catalog = {
  actions: [
    spec("close_all_holes", "Close all holes"),
    spec("close_trivial_holes", "Close trivial holes"),
    spec("fix_flipped_triangles", "Fix flipped triangles"),
    spec("remesh", "Remesh", { implemented: false }),
    spec("remove_degenerate_faces", "Remove degenerate faces", {
      params: [yesNo("calc_tolerance", "Calculate tolerance by bounding box", true), mm("tolerance", "Tolerance", 4, 0.002)],
    }),
    spec("remove_double_triangles", "Remove double triangles"),
    spec("remove_tiny_shells", "Remove tiny shells", {
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
    spec("stitch_triangles", "Stitch triangles", { params: [mm("tolerance", "Tolerance", 3, 0.01), yesNo("ignore_orientation", "Ignore orientation", false)] }),
    spec("wrap_part_surface", "Wrap part surface"),
    spec("close_defined_holes", "Close defined holes", { inScripts: false, needsSelection: true }),
  ],
  scripts: [],
};

const step = (type: ScriptStep["type"], params: ScriptStep["params"] = {}, enabled = true): ScriptStep => ({ type, enabled, params });
const stitch = (ignore: boolean) => step("stitch_triangles", { tolerance: 0.01, ignore_orientation: ignore });
const defaultRepair: RepairScript = {
  id: "builtin.default",
  name: "Default repair",
  builtIn: true,
  actions: [
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
  ],
};
const own: RepairScript = { id: "own.1", name: "Mine", builtIn: false, actions: [step("close_trivial_holes"), stitch(false), step("close_all_holes")] };

let seq = 100;
const fresh = () => ++seq;
const indices = (n: number) => Array.from({ length: n }, (_, i) => i);

describe("editedScript and stepKeys", () => {
  it("show the saved script while there are no unsaved steps", () => {
    expect(editedScript(own, undefined)).toBe(own);
    expect(editedScript(own, { actions: null, keys: [5, 6, 7] })).toBe(own);
    expect(stepKeys(own, undefined)).toEqual([0, 1, 2]);
    // Keys kept after a save still fit the saved steps.
    expect(stepKeys(own, { actions: null, keys: [5, 6, 7] })).toEqual([5, 6, 7]);
  });

  it("show the unsaved steps under the saved name and id", () => {
    const actions = [step("wrap_part_surface")];
    const shown = editedScript({ ...own, name: "Renamed meanwhile" }, { actions, keys: [9] });
    expect(shown).toEqual({ id: "own.1", name: "Renamed meanwhile", builtIn: false, actions });
    expect(stepKeys(shown, { actions, keys: [9] })).toEqual([9]);
  });

  it("fall back to index keys when the keys no longer fit the steps", () => {
    expect(stepKeys(own, { actions: null, keys: [1, 2] })).toEqual([0, 1, 2]);
  });
});

describe("carryKeys", () => {
  const keys = [10, 11, 12];

  it("moves each key with its step", () => {
    const moved = moveStep(own, 0, 2);
    expect(carryKeys(own.actions, keys, moved.actions, fresh)).toEqual([11, 12, 10]);
    expect(carryKeys(own.actions, keys, moveStep(own, 2, 0).actions, fresh)).toEqual([12, 10, 11]);
  });

  it("keeps the other keys when a step is removed", () => {
    expect(carryKeys(own.actions, keys, removeStep(own, 1).actions, fresh)).toEqual([10, 12]);
    expect(carryKeys(own.actions, keys, removeStep(own, 0).actions, fresh)).toEqual([11, 12]);
  });

  it("keeps a changed step's key (switched, parameter set)", () => {
    expect(carryKeys(own.actions, keys, toggleStep(own, 1).actions, fresh)).toEqual(keys);
    const param = setStepParam(own, 1, "ignore_orientation", true, catalog);
    expect(param.actions[1]).not.toBe(own.actions[1]);
    expect(carryKeys(own.actions, keys, param.actions, fresh)).toEqual(keys);
  });

  it("gives an added step a fresh key", () => {
    const added = addStep(own, "wrap_part_surface", catalog);
    const out = carryKeys(own.actions, keys, added.actions, () => 99);
    expect(out).toEqual([10, 11, 12, 99]);
  });

  it("never repeats a key", () => {
    // Two lists that share no step objects and differ in length: every key is fresh.
    let k = 0;
    const out = carryKeys(own.actions, keys, [step("close_all_holes"), step("close_all_holes")], () => ++k);
    expect(out).toEqual([1, 2]);
    // Repeated keys coming in, and a key source that hands out one already taken: still all different.
    const unique = carryKeys(own.actions, [4, 4, 4], own.actions.slice(), () => ++k);
    expect(unique[0]).toBe(4);
    expect(new Set(unique).size).toBe(3);
  });
});

describe("nextEdit", () => {
  it("keeps the changed steps and their keys", () => {
    const next = moveStep(own, 0, 1);
    const edit = nextEdit(own, own, [0, 1, 2], next, fresh);
    expect(edit).toEqual({ actions: next.actions, keys: [1, 0, 2] });
  });

  it("has no unsaved steps once the edits are undone by hand, and keeps the keys", () => {
    const first = nextEdit(own, own, [0, 1, 2], toggleStep(own, 2), fresh);
    const shown = editedScript(own, first);
    expect(first.actions).not.toBeNull();
    const back = nextEdit(own, shown, first.keys, toggleStep(shown, 2), fresh);
    expect(back).toEqual({ actions: null, keys: [0, 1, 2] });
  });

  it("treats a parameter set back to its saved value as no change", () => {
    const yes = setStepParam(defaultRepair, 0, "ignore_orientation", true, catalog);
    const edit: ScriptEdit = nextEdit(defaultRepair, defaultRepair, indices(10), yes, fresh);
    expect(edit.actions?.[0].params).toEqual({ tolerance: 0.01, ignore_orientation: true });
    const shown = editedScript(defaultRepair, edit);
    expect(nextEdit(defaultRepair, shown, edit.keys, setStepParam(shown, 0, "ignore_orientation", false, catalog), fresh).actions).toBeNull();
  });
});

describe("dragging rows", () => {
  // Rows 20 px tall starting at y = 100: middles at 110, 130, 150, 170.
  const middles = [110, 130, 150, 170];

  it("finds the gap the pointer is over", () => {
    expect(insertionIndex(middles, 90)).toBe(0);
    expect(insertionIndex(middles, 109)).toBe(0);
    expect(insertionIndex(middles, 111)).toBe(1);
    expect(insertionIndex(middles, 160)).toBe(3);
    expect(insertionIndex(middles, 400)).toBe(4);
    expect(insertionIndex([], 5)).toBe(0);
  });

  it("turns a gap into moveStep's target", () => {
    // Dropping into either gap next to the row leaves it where it is.
    expect(moveTarget(1, 1)).toBe(1);
    expect(moveTarget(1, 2)).toBe(1);
    expect(moveTarget(1, 0)).toBe(0);
    expect(moveTarget(1, 4)).toBe(3);
    expect(moveTarget(3, 0)).toBe(0);
    // The last row dropped at the top, the first at the bottom.
    const script = { ...own, actions: [step("close_all_holes"), step("close_trivial_holes"), step("wrap_part_surface"), step("fix_flipped_triangles")] };
    expect(moveStep(script, 0, moveTarget(0, 4)).actions.map((s) => s.type)).toEqual(["close_trivial_holes", "wrap_part_surface", "fix_flipped_triangles", "close_all_holes"]);
    expect(moveStep(script, 3, moveTarget(3, 0)).actions[0].type).toBe("fix_flipped_triangles");
  });
});

describe("labels", () => {
  it("marks scripts with unsaved edits in the dropdown", () => {
    expect(optionLabel(own, false)).toBe("Mine");
    expect(optionLabel(defaultRepair, true)).toBe("Default repair (edited)");
  });

  it("offers Save as copy for built-in scripts", () => {
    expect(saveLabel(defaultRepair)).toBe("Save as copy");
    expect(saveLabel(own)).toBe("Save");
  });

  it("says what the script is under the picker", () => {
    expect(scriptInfo(defaultRepair, false)).toBe("Built-in, 10 repair actions.");
    expect(scriptInfo(defaultRepair, true)).toBe("Built-in, 10 repair actions. Save your changes as a copy to keep them.");
    expect(scriptInfo(own, false)).toBe("3 repair actions.");
    expect(scriptInfo({ ...own, actions: [step("close_all_holes")] }, true)).toBe("1 repair action, changes not saved.");
  });

  it("shows float parameters with the spec's decimals and no separators", () => {
    expect(paramText(mm("tolerance", "Tolerance", 3, 0.01), 0.01)).toBe("0.010");
    expect(paramText(mm("tolerance", "Tolerance", 4, 0.002), 0.002)).toBe("0.0020");
    expect(paramText(mm("min_volume", "Minimum volume", 4, 0.0001), 12345.5)).toBe("12345.5000");
    expect(paramText({ id: "x", label: "X", kind: "float", default: 1 }, 2.5)).toBe("2.5");
  });

  it("adds up the time of a run", () => {
    const report = (millis: number): ActionReport => ({ action: "close_all_holes", label: "Close all holes", message: "", warnings: [], resolved: [], changed: false, millis });
    expect(runTime([])).toBe("0 ms");
    expect(runTime([report(120), report(300)])).toBe("420 ms");
    expect(runTime([report(2100), report(2100)])).toBe("4.2 s");
  });
});

describe("executeBlocked", () => {
  it("lets a runnable script run", () => {
    expect(executeBlocked(defaultRepair, catalog, false)).toBeNull();
  });

  it("waits for a run in progress", () => {
    expect(executeBlocked(defaultRepair, catalog, true)).toBe("Wait for the current repair to finish.");
  });

  it("names the first problem of the script", () => {
    const soon = { ...own, actions: [step("close_all_holes"), step("remesh")] };
    expect(executeBlocked(soon, catalog, false)).toBe("Step 2, Remesh, is not available yet.");
    // A switched-off Soon step is skipped, so it does not block.
    expect(executeBlocked({ ...soon, actions: [step("close_all_holes"), step("remesh", {}, false)] }, catalog, false)).toBeNull();
  });

  it("needs at least one step that is switched on", () => {
    expect(executeBlocked({ ...own, actions: [] }, catalog, false)).toBe("Add an action first.");
    expect(executeBlocked({ ...own, actions: [step("close_all_holes", {}, false)] }, catalog, false)).toBe("Switch on at least one action.");
  });
});

describe("StepList", () => {
  const render = (script: RepairScript, keys = indices(script.actions.length)) =>
    renderToStaticMarkup(<StepList script={script} keys={keys} catalog={catalog} onChange={() => {}} />);
  const items = (html: string) => [...html.matchAll(/<li ([^>]*)>(.*?)<\/li>/g)].map((m) => ({ attrs: m[1], body: m[2] }));
  const text = (html: string) => html.replace(/<svg.*?<\/svg>/g, "").replace(/<[^>]+>/g, " ").replace(/\s+/g, " ").trim();

  it("draws one tree item per step, numbered, with its switch", () => {
    const html = render(defaultRepair);
    expect(html).toMatch(/^<ul class="rp-steps" role="tree" aria-label="Repair actions">/);
    const rows = items(html);
    expect(rows).toHaveLength(10);
    expect(rows[0].attrs).toContain('aria-label="1. Stitch triangles"');
    expect(rows[9].attrs).toContain('aria-label="10. Remove tiny shells"');
    for (const r of rows) {
      expect(r.attrs).toContain('role="treeitem"');
      expect(r.body).toMatch(/<input type="checkbox" class="rp-step-check" tabindex="-1"[^>]* checked=""/);
    }
    // Only the first row is in the tab order (one tab stop for the whole tree).
    expect(rows.filter((r) => r.attrs.includes('tabindex="0"'))).toHaveLength(1);
    expect(rows[0].attrs).toContain('tabindex="0"');
  });

  it("shows the parameters of a closed row in one line, and an expander only where there are parameters", () => {
    const rows = items(render(defaultRepair));
    expect(text(rows[0].body)).toBe("1 Stitch triangles Tolerance 0.010 mm, Ignore orientation No");
    expect(text(rows[5].body)).toBe("6 Stitch triangles Tolerance 0.010 mm, Ignore orientation Yes");
    expect(text(rows[8].body)).toBe("9 Remove degenerate faces Calculate tolerance by bounding box Yes, Tolerance 0.0020 mm");
    expect(text(rows[1].body)).toBe("2 Close trivial holes");
    expect(rows[0].attrs).toContain('aria-expanded="false"');
    expect(rows[1].attrs).not.toContain("aria-expanded");
    expect(rows[0].body).toContain('<button type="button" class="rp-expand" tabindex="-1" aria-label="Show the parameters of Stitch triangles">');
    expect(rows[1].body).toContain('<span class="rp-expand" aria-hidden="true"></span>');
  });

  it("marks switched-off, Soon and unknown steps", () => {
    const script: RepairScript = {
      ...own,
      actions: [step("close_all_holes", {}, false), step("remesh"), { type: "nonsense" as ScriptStep["type"], enabled: true, params: {} }],
    };
    const rows = items(render(script, [7, 8, 9]));
    expect(rows[0].attrs).toContain('class="rp-step is-off"');
    expect(rows[0].attrs).toContain('aria-label="1. Close all holes, switched off"');
    expect(rows[0].body).not.toContain('checked=""');
    expect(rows[1].body).toContain('<span class="soon-pill">Soon</span>');
    expect(rows[2].attrs).toContain('class="rp-step is-unknown"');
    expect(text(rows[2].body)).toBe("3 nonsense");
  });

  it("shows a note that can take the focus when there are no steps, so removing the last row does not drop it to the page", () => {
    expect(render({ ...own, actions: [] })).toBe('<p class="rpanel-muted rp-empty" tabindex="-1">No actions yet. Pick one below and click Add.</p>');
  });
});

describe("onEmptyStepsKeyDown", () => {
  /** Whether the empty list's note marks the key handled, which keeps it from the window's shortcuts. */
  const handled = (key: string, isMac = false) => {
    let prevented = false;
    onEmptyStepsKeyDown({ key, preventDefault: () => (prevented = true) }, isMac);
    return prevented;
  };

  it("keeps Del, pressed again or still held after the last step went, from removing the selected triangles", () => {
    expect(handled("Delete")).toBe(true);
    expect(handled("Backspace", true)).toBe(true);
    // Backspace is no shortcut outside macOS, and other keys (Tab to move on) work as usual.
    expect(handled("Backspace")).toBe(false);
    expect(handled("Tab")).toBe(false);
    expect(handled("Escape")).toBe(false);
  });
});

describe("LastRun", () => {
  it("lists each action with its message and time, the run's total in the heading", () => {
    const reports: ActionReport[] = [
      { action: "stitch_triangles", label: "Stitch triangles", message: "Stitched 24 edges.", warnings: [], resolved: [], changed: true, millis: 120 },
      { action: "close_all_holes", label: "Close all holes", message: "No holes to close.", warnings: [], resolved: ["2 holes could not be closed. Closed by Wrap part surface."], changed: false, millis: 4 },
      { action: "wrap_part_surface", label: "Wrap part surface", message: "Wrapped the surface.", warnings: ["3 small self-intersections remain."], resolved: [], changed: true, millis: 2600 },
    ];
    const html = renderToStaticMarkup(<LastRun reports={reports} />);
    expect(html).toContain('aria-expanded="true"');
    expect(html).toMatch(/Last run<span class="rp-fold-time">2\.7 s<\/span>/);
    const rows = [...html.matchAll(/<li class="([^"]+)">(.*?)<\/li>/g)];
    expect(rows.map((r) => r[1])).toEqual(["rp-report", "rp-report is-unchanged", "rp-report"]);
    expect(rows[0][2]).toContain('<span class="rp-report-label">Stitch triangles</span><span class="rp-report-time">120 ms</span>');
    expect(rows[0][2]).toContain('<div class="rp-report-msg">Stitched 24 edges.</div>');
    expect(rows[2][2]).toContain('<div class="rp-report-warn">3 small self-intersections remain.</div>');
    // A warning a later action put right shows as resolved, not as a warning.
    expect(rows[1][2]).toContain('<div class="rp-report-resolved">2 holes could not be closed. Closed by Wrap part surface.</div>');
    expect(rows[1][2]).not.toContain("rp-report-warn");
  });
});
