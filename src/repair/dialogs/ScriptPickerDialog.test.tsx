import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";

// Running and reordering are the session's and scripts.ts's work; the picker hands over the script and the move.
const session = vi.hoisted(() => ({ runScript: vi.fn() }));
const scripts = vi.hoisted(() => ({ moveScript: vi.fn(), loadScripts: vi.fn() }));
vi.mock("../session", () => session);
vi.mock("../scripts", async (importOriginal) => ({ ...(await importOriginal<typeof import("../scripts")>()), ...scripts }));

import type { ActionSpec, Catalog, RepairScript } from "../types";
import {
  ScriptPickerDialog,
  ScriptPickerForm,
  executeScript,
  filterScripts,
  initialPick,
  listKey,
  pickedIn,
  runProblem,
  stepPick,
  type ScriptPickerFormProps,
} from "./ScriptPickerDialog";

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
    spec("fix_flipped_triangles", "Fix flipped triangles"),
    spec("close_trivial_holes", "Close trivial holes"),
    spec("close_all_holes", "Close all holes"),
    spec("wrap_part_surface", "Wrap part surface"),
    spec("remesh", "Remesh", { implemented: false }),
  ],
  scripts: [],
};

const step = (type: ActionSpec["id"], enabled = true) => ({ type, enabled, params: {} });

const SIMPLE: RepairScript = {
  id: "builtin.simple",
  name: "Simple repair",
  builtIn: true,
  actions: [step("fix_flipped_triangles"), step("close_trivial_holes"), step("close_all_holes")],
};
const EXTENDED: RepairScript = {
  id: "builtin.extended",
  name: "Extended repair",
  builtIn: true,
  actions: Array.from({ length: 11 }, () => step("close_all_holes")),
};
const MINE: RepairScript = { id: "own.1", name: "My wrap", builtIn: false, actions: [step("wrap_part_surface")] };
const SOON: RepairScript = { id: "own.2", name: "Remesh later", builtIn: false, actions: [step("remesh")] };
const OFF: RepairScript = { id: "own.3", name: "All off", builtIn: false, actions: [step("close_all_holes", false)] };
const ALL = [SIMPLE, EXTENDED, MINE];

const render = (over: Partial<ScriptPickerFormProps> = {}) =>
  renderToStaticMarkup(<ScriptPickerForm scripts={ALL} catalog={catalog} state="ready" onClose={() => {}} {...over} />);

/** Every option row as [name, tag or "", count text, selected]. */
function rows(html: string): [string, string, string, boolean][] {
  return [...html.matchAll(/<li [^>]*aria-selected="(true|false)"[^>]*>(.*?)<\/li>/g)].map((m) => {
    const name = m[2].match(/class="rp-script-name">(.*?)<\/span>/)?.[1] ?? "";
    const tag = m[2].match(/class="rp-tag">(.*?)<\/span>/)?.[1] ?? "";
    const count = m[2].match(/class="rp-script-count">(.*?)<\/span>/)?.[1] ?? "";
    return [name, tag, count, m[1] === "true"];
  });
}
const button = (html: string, label: string) => html.match(new RegExp(`<button[^>]*aria-label="${label}"[^>]*>`))?.[0] ?? "";
const execute = (html: string) => html.match(/<button[^>]*>Execute<\/button>/)?.[0] ?? "";

beforeEach(() => {
  session.runScript.mockReset();
  session.runScript.mockResolvedValue(undefined);
  scripts.moveScript.mockReset();
  scripts.loadScripts.mockReset();
  scripts.loadScripts.mockResolvedValue(true);
});

describe("filterScripts", () => {
  it("matches names by any part, ignoring case and the spaces around the query", () => {
    expect(filterScripts(ALL, "")).toEqual(ALL);
    expect(filterScripts(ALL, "   ")).toEqual(ALL);
    expect(filterScripts(ALL, "  REPAIR ")).toEqual([SIMPLE, EXTENDED]);
    expect(filterScripts(ALL, "wrap")).toEqual([MINE]);
    expect(filterScripts(ALL, "zzz")).toEqual([]);
  });

  it("returns a new list, leaving the picker order alone", () => {
    const out = filterScripts(ALL, "");
    expect(out).not.toBe(ALL);
    out.reverse();
    expect(ALL[0]).toBe(SIMPLE);
  });
});

describe("picking", () => {
  it("acts on the picked script while it is listed, else on the first listed", () => {
    expect(pickedIn(ALL, "own.1")).toBe(MINE);
    expect(pickedIn([SIMPLE, EXTENDED], "own.1")).toBe(SIMPLE);
    expect(pickedIn(ALL, null)).toBe(SIMPLE);
    expect(pickedIn([], "own.1")).toBeNull();
  });

  it("steps through the list and stops at its ends", () => {
    expect(stepPick(ALL, "builtin.simple", 1)).toBe("builtin.extended");
    expect(stepPick(ALL, "builtin.extended", -1)).toBe("builtin.simple");
    expect(stepPick(ALL, "builtin.simple", -1)).toBe("builtin.simple");
    expect(stepPick(ALL, "own.1", 1)).toBe("own.1");
    expect(stepPick(ALL, "builtin.extended", Infinity)).toBe("own.1");
    expect(stepPick(ALL, "own.1", -Infinity)).toBe("builtin.simple");
    expect(stepPick([], null, 1)).toBeNull();
  });

  it("maps the list keys, leaving Home and End to the search field's text", () => {
    expect(listKey({ key: "ArrowDown", altKey: false }, false)).toEqual({ pick: 1 });
    expect(listKey({ key: "ArrowUp", altKey: false }, true)).toEqual({ pick: -1 });
    expect(listKey({ key: "ArrowUp", altKey: true }, false)).toEqual({ move: -1 });
    expect(listKey({ key: "ArrowDown", altKey: true }, true)).toEqual({ move: 1 });
    expect(listKey({ key: "Home", altKey: false }, false)).toEqual({ pick: -Infinity });
    expect(listKey({ key: "End", altKey: false }, false)).toEqual({ pick: Infinity });
    expect(listKey({ key: "Home", altKey: false }, true)).toBeNull();
    expect(listKey({ key: "End", altKey: true }, false)).toBeNull();
    expect(listKey({ key: "Enter", altKey: false }, false)).toBeNull();
    expect(listKey({ key: "a", altKey: false }, true)).toBeNull();
  });
});

describe("runProblem", () => {
  it("names what keeps a script from running", () => {
    expect(runProblem(SIMPLE, catalog)).toBeNull();
    expect(runProblem(SOON, catalog)).toBe("Step 1, Remesh, is not available yet.");
    expect(runProblem(OFF, catalog)).toBe("This script has no enabled actions.");
    expect(runProblem({ ...MINE, actions: [] }, catalog)).toBe("This script has no enabled actions.");
    // Without the catalog only the enabled actions can be checked.
    expect(runProblem(SOON, null)).toBeNull();
    expect(runProblem(OFF, null)).toBe("This script has no enabled actions.");
  });
});

describe("ScriptPickerForm", () => {
  it("lists the scripts in picker order with their action counts and a Default tag on built-ins", () => {
    const html = render();
    expect(html).toMatch(/class="dlg-title">Run repair script<\/h2>/);
    expect(html.startsWith('<div class="dlg-backdrop"><div class="dlg dlg-rp-picker" role="dialog"')).toBe(true);
    expect(rows(html)).toEqual([
      ["Simple repair", "Default", "3 repair actions", true],
      ["Extended repair", "Default", "11 repair actions", false],
      ["My wrap", "", "1 repair action", false],
    ]);
    expect(execute(html)).not.toContain("disabled");
    expect(html).toContain(">Cancel</button>");
  });

  it("starts with the search field, which controls the list", () => {
    const html = render();
    const search = html.match(/<input [^>]*>/)?.[0] ?? "";
    expect(html.indexOf("<input")).toBeLessThan(html.indexOf('role="listbox"'));
    expect(search).toContain('type="search"');
    expect(search).toContain('placeholder="Search scripts"');
    expect(search).toContain('aria-label="Search scripts"');
    const listId = html.match(/<ul [^>]*id="([^"]+)"[^>]*role="listbox"/)?.[1];
    expect(listId).toBeTruthy();
    expect(search).toContain(`aria-controls="${listId}"`);
    // The list takes focus itself and points screen readers at the picked row.
    const list = html.match(/<ul [^>]*role="listbox"[^>]*>/)?.[0] ?? "";
    expect(list).toContain('tabindex="0"');
    expect(list).toContain('aria-label="Repair scripts"');
    expect(list).toContain(`aria-activedescendant="${listId}-0"`);
    expect(html).toContain(`<li id="${listId}-0" class="rp-script" role="option" aria-selected="true"`);
  });

  it("makes the search field a combobox over the list, so a screen reader names each script Up and Down pick there", () => {
    const search = (html: string) => html.match(/<input [^>]*>/)?.[0] ?? "";
    const listIdOf = (html: string) => html.match(/<ul [^>]*id="([^"]+)"[^>]*role="listbox"/)?.[1];
    const html = render();
    expect(search(html)).toContain('role="combobox"');
    expect(search(html)).toContain('aria-expanded="true"');
    expect(search(html)).toContain('aria-autocomplete="list"');
    expect(search(html)).toContain(`aria-activedescendant="${listIdOf(html)}-0"`);
    // Filtered, it points at the picked script's row in the shorter list; with no match, at nothing.
    const filtered = render({ initialQuery: "wrap" });
    expect(search(filtered)).toContain(`aria-activedescendant="${listIdOf(filtered)}-0"`);
    expect(search(render({ initialQuery: "zzz" }))).not.toContain("aria-activedescendant");
  });

  it("offers the order arrows for the picked script, up disabled at the top", () => {
    const html = render();
    expect(html).toContain('role="group" aria-label="Picker order"');
    expect(button(html, "Move script up")).toContain('disabled=""');
    expect(button(html, "Move script up")).toContain('title="Move the script up (Alt+↑)"');
    expect(button(html, "Move script down")).not.toContain("disabled");
    expect(button(html, "Move script down")).toContain('title="Move the script down (Alt+↓)"');
  });

  it("filters by the search text and pauses the order arrows meanwhile", () => {
    const html = render({ initialQuery: "repair" });
    expect(rows(html).map((r) => r[0])).toEqual(["Simple repair", "Extended repair"]);
    for (const label of ["Move script up", "Move script down"]) {
      expect(button(html, label)).toContain('disabled=""');
      expect(button(html, label)).toContain('title="Clear the search to change the order."');
    }
    expect(html).toMatch(/<input [^>]*value="repair"/);
  });

  it("says when nothing matches, and cannot execute", () => {
    const html = render({ initialQuery: " zzz " });
    expect(rows(html)).toEqual([]);
    expect(html).toContain("No script matches “zzz”.");
    expect(execute(html)).toContain('disabled=""');
    expect(button(html, "Move script up")).toContain('disabled=""');
    expect(button(html, "Move script down")).toContain('disabled=""');
    expect(html).not.toContain("aria-activedescendant");
  });

  it("shows why the picked script cannot run, and cannot execute it", () => {
    const html = render({ scripts: [SOON, SIMPLE] });
    expect(rows(html)[0]).toEqual(["Remesh later", "", "1 repair action", true]);
    expect(html).toContain('<p class="dlg-error" role="alert">Step 1, Remesh, is not available yet.</p>');
    expect(execute(html)).toContain('disabled=""');
  });

  it("says when the scripts are loading or could not be loaded", () => {
    const loading = render({ scripts: [], catalog: null, state: "loading" });
    expect(loading).toContain("Loading the repair scripts…");
    expect(execute(loading)).toContain('disabled=""');
    expect(loading).not.toContain("No script matches");
    const failed = render({ scripts: [], catalog: null, state: "failed" });
    expect(failed).toContain('<p class="dlg-error" role="alert">The repair scripts could not be loaded.</p>');
    expect(render({ scripts: [] })).toContain("There are no repair scripts.");
  });
});

describe("ScriptPickerDialog", () => {
  it("shows the loading note while the catalog is not in the store", () => {
    // Server rendering reads the repair store's initial state, which has no catalog yet.
    const html = renderToStaticMarkup(<ScriptPickerDialog onClose={() => {}} />);
    expect(html).toContain("Loading the repair scripts…");
  });
});

// Last: executing remembers the script for the next time the picker opens.
describe("executeScript", () => {
  it("closes the picker first, then runs the script", () => {
    const calls: string[] = [];
    session.runScript.mockImplementation((s: RepairScript) => {
      calls.push(`run ${s.id}`);
      return Promise.resolve();
    });
    executeScript(EXTENDED, () => calls.push("close"));
    expect(calls).toEqual(["close", "run builtin.extended"]);
    expect(session.runScript).toHaveBeenCalledWith(EXTENDED);
  });

  it("opens next time with the script last executed, or the first one when that is gone", () => {
    executeScript(MINE, () => {});
    expect(initialPick(ALL)).toBe("own.1");
    const html = render();
    expect(rows(html)[2]).toEqual(["My wrap", "", "1 repair action", true]);
    // The search field, which has the focus, names that script too.
    const listId = html.match(/<ul [^>]*id="([^"]+)"[^>]*role="listbox"/)?.[1];
    expect(html.match(/<input [^>]*>/)?.[0]).toContain(`aria-activedescendant="${listId}-2"`);
    expect(initialPick([SIMPLE, EXTENDED])).toBe("builtin.simple");
    expect(initialPick([])).toBeNull();
  });
});
