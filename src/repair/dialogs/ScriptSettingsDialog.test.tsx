import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";

// Creating, renaming and saving are scripts.ts's work; the dialog hands over the name.
const scripts = vi.hoisted(() => ({ createOwnScript: vi.fn(), renameOwnScript: vi.fn(), loadScripts: vi.fn() }));
vi.mock("../scripts", async (importOriginal) => ({ ...(await importOriginal<typeof import("../scripts")>()), ...scripts }));
vi.mock("../session", () => ({ runScript: vi.fn() }));

import type { RepairScript } from "../types";
import { ScriptSettingsDialog, ScriptSettingsForm, initialName, nameProblem, submitName } from "./ScriptSettingsDialog";

const BUILTIN: RepairScript = { id: "builtin.extended", name: "Extended repair", builtIn: true, actions: [] };
const MINE: RepairScript = {
  id: "own.1",
  name: "My wrap",
  builtIn: false,
  actions: [
    { type: "close_all_holes", enabled: true, params: {} },
    { type: "wrap_part_surface", enabled: false, params: {} },
  ],
};
const NEW: RepairScript = { id: "own.2", name: "New script", builtIn: false, actions: [] };
const ALL = [BUILTIN, MINE];

const render = (script: RepairScript | null, all: RepairScript[] = ALL) =>
  renderToStaticMarkup(<ScriptSettingsForm script={script} scripts={all} onClose={() => {}} />);
const nameInput = (html: string) => html.match(/<input [^>]*>/)?.[0] ?? "";
const buttons = (html: string) => [...html.matchAll(/<button[^>]*>(.*?)<\/button>/g)].map((m) => m[1]);

beforeEach(() => {
  scripts.createOwnScript.mockReset();
  scripts.renameOwnScript.mockReset();
  scripts.loadScripts.mockReset();
  scripts.loadScripts.mockResolvedValue(true);
});

describe("nameProblem", () => {
  it("needs a name that is not blank", () => {
    expect(nameProblem("", ALL, null)).toBe("Enter a name.");
    expect(nameProblem("   ", ALL, "own.1")).toBe("Enter a name.");
  });

  it("refuses another script's name, ignoring the spaces around it, but not the script's own", () => {
    expect(nameProblem(" Extended repair ", ALL, null)).toBe("Another script is already called “Extended repair”.");
    expect(nameProblem("My wrap", ALL, null)).toBe("Another script is already called “My wrap”.");
    expect(nameProblem("Extended repair", ALL, "own.1")).toBe("Another script is already called “Extended repair”.");
    expect(nameProblem("My wrap", ALL, "own.1")).toBeNull();
    expect(nameProblem("My wrap 2", ALL, "own.1")).toBeNull();
    expect(nameProblem("Fresh", ALL, null)).toBeNull();
  });
});

describe("initialName", () => {
  it("is the script's name, or a free New script name for a new one", () => {
    expect(initialName(MINE, ALL)).toBe("My wrap");
    expect(initialName(null, ALL)).toBe("New script");
    expect(initialName(null, [...ALL, NEW])).toBe("New script 2");
    expect(initialName(null, [...ALL, NEW, { ...NEW, id: "own.3", name: "New script 2" }])).toBe("New script 3");
  });
});

describe("ScriptSettingsForm", () => {
  it("names a new script, starting from a free name, with Create", () => {
    const html = render(null);
    expect(html).toMatch(/class="dlg-title">New repair script<\/h2>/);
    expect(html.startsWith('<div class="dlg-backdrop"><div class="dlg dlg-rp-settings" role="dialog"')).toBe(true);
    expect(html).toContain('<span class="dlg-label">Name</span>');
    expect(nameInput(html)).toContain('value="New script"');
    expect(nameInput(html)).toContain('aria-invalid="false"');
    expect(buttons(html)).toEqual(["Cancel", "Create"]);
    expect(html).not.toMatch(/<button[^>]*disabled/);
    expect(html).not.toContain("dlg-error");
  });

  it("renames one of the user's scripts, starting from its name, and shows its action count", () => {
    const html = render(MINE);
    expect(html).toMatch(/class="dlg-title">Script settings<\/h2>/);
    expect(nameInput(html)).toContain('value="My wrap"');
    // React writes the attribute as readOnly; HTML attribute names ignore case.
    expect(nameInput(html)).not.toMatch(/readonly/i);
    expect(buttons(html)).toEqual(["Cancel", "Rename"]);
    expect(html).toContain('<p class="dlg-note">2 repair actions</p>');
  });

  it("shows a built-in script's name read-only, with the way to make an own copy and only Close", () => {
    const html = render(BUILTIN);
    expect(html).toMatch(/class="dlg-title">Script settings<\/h2>/);
    expect(nameInput(html)).toContain('value="Extended repair"');
    expect(nameInput(html)).toMatch(/readonly=""/i);
    expect(html).toContain("Built-in scripts keep their names. Use Save as copy to make a version of your own.");
    expect(buttons(html)).toEqual(["Close"]);
  });

  it("explains a name it cannot take and disables OK", () => {
    // Opened for a script whose name another script took meanwhile.
    const html = render({ ...MINE, name: "Extended repair" });
    expect(html).toMatch(/<p id="([^"]+)" class="dlg-error" role="alert">Another script is already called “Extended repair”.<\/p>/);
    const errorId = html.match(/<p id="([^"]+)" class="dlg-error"/)?.[1];
    expect(nameInput(html)).toContain('aria-invalid="true"');
    expect(nameInput(html)).toContain(`aria-describedby="${errorId}"`);
    expect(html).toMatch(/<button[^>]*disabled=""[^>]*>Rename<\/button>/);
  });
});

describe("ScriptSettingsDialog", () => {
  it("shows the loading note while the catalog is not in the store", () => {
    // Server rendering reads the repair store's initial state, which has no catalog yet.
    const fresh = renderToStaticMarkup(<ScriptSettingsDialog scriptId={null} onClose={() => {}} />);
    expect(fresh).toMatch(/class="dlg-title">New repair script<\/h2>/);
    expect(fresh).toContain("Loading the repair scripts…");
    expect(buttons(fresh)).toEqual(["Close"]);
    const rename = renderToStaticMarkup(<ScriptSettingsDialog scriptId="own.1" onClose={() => {}} />);
    expect(rename).toMatch(/class="dlg-title">Script settings<\/h2>/);
    expect(rename).toContain("Loading the repair scripts…");
  });
});

describe("submitName", () => {
  it("closes the dialog first, then creates a new script with the name", () => {
    const calls: string[] = [];
    scripts.createOwnScript.mockImplementation((name: string) => {
      calls.push(`create ${name}`);
      return Promise.resolve(null);
    });
    submitName(null, "Fresh", () => calls.push("close"));
    expect(calls).toEqual(["close", "create Fresh"]);
    expect(scripts.renameOwnScript).not.toHaveBeenCalled();
  });

  it("closes the dialog first, then renames the script", () => {
    const calls: string[] = [];
    scripts.renameOwnScript.mockImplementation((id: string, name: string) => {
      calls.push(`rename ${id} ${name}`);
      return Promise.resolve(true);
    });
    submitName(MINE, "Our wrap", () => calls.push("close"));
    expect(calls).toEqual(["close", "rename own.1 Our wrap"]);
    expect(scripts.createOwnScript).not.toHaveBeenCalled();
  });
});
