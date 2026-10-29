import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { makeBox, translation } from "../mesh";
import { useStore } from "../store";
import { GenerateShellDialog, ShellForm } from "./GenerateShellDialog";
import { DEFAULT_FIELDS, SETTINGS_KEY, type ShellFields } from "./settings";

// The form only; running needs the desktop app (src/shell/session.test.ts runs it against a stand-in).
vi.mock("./ipc", () => ({ estimateShells: vi.fn(() => new Promise(() => {})), generateShells: vi.fn() }));

const form = (fields: ShellFields, targets = 1) =>
  renderToStaticMarkup(<ShellForm fields={fields} setFields={() => {}} targets={targets} onRun={() => {}} onClose={() => {}} />);

const ok = (html: string) => html.match(/<button[^>]*dlg-btn-primary[^>]*>[^<]*<\/button>/)?.[0] ?? "";
const text = (html: string) => html.replace(/<[^>]+>/g, " ").replace(/\s+/g, " ");

function fakeStorage(entries: Record<string, string> = {}) {
  const data = new Map(Object.entries(entries));
  return { data, getItem: (key: string) => data.get(key) ?? null, setItem: (key: string, value: string) => void data.set(key, value) };
}

beforeEach(() => {
  vi.stubGlobal("localStorage", fakeStorage());
  useStore.getState().newProject();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("Generate shell's dialog", () => {
  it("asks in order: thickness, type, output, accuracy, smoothing, removing the original; then OK and Cancel", () => {
    const html = form(DEFAULT_FIELDS);
    const words = text(html);
    const order = ["Shell thickness", "Shell type", "Output", "Create part", "Create slice", "Accuracy", "Smooth result", "Remove original part", "Cancel", "OK"];
    const at = order.map((w) => words.indexOf(w));
    expect(at.every((i) => i >= 0)).toBe(true);
    expect([...at].sort((x, y) => x - y)).toEqual(at);
    expect(html).toMatch(/class="dlg-title">Generate shell<\/h2>/);
  });

  it("starts at 2 mm, Hollow part, Create part, 0.25 mm, not smoothed, keeping the original", () => {
    const html = form(DEFAULT_FIELDS);
    expect(html).toMatch(/<input[^>]*value="2"/);
    expect(html).toMatch(/<input[^>]*value="0.25"/);
    expect(html).toContain('<option value="hollow-part" selected="">Hollow part</option>');
    for (const label of ["Inner offset", "Outer offset", "Hollowing shell", "Hollow part, improved"]) expect(html).toContain(`>${label}</option>`);
    expect(html).toContain("The part with a closed cavity inside");
    const radios = [...html.matchAll(/<input type="radio"[^>]*>/g)].map((m) => m[0]);
    expect(radios[0]).toContain('checked=""');
    // Create slice shows, greyed out and marked Soon.
    expect(radios[1]).toContain('disabled=""');
    expect(html).toContain('Create slice <span class="soon-pill">Soon</span>');
    expect([...html.matchAll(/<input type="checkbox"[^>]*>/g)].map((m) => m[0]).some((c) => c.includes("checked"))).toBe(false);
    expect(ok(html)).not.toContain("disabled");
  });

  it("keeps OK disabled, saying why, until the fields make sense and a part is selected", () => {
    let html = form({ ...DEFAULT_FIELDS, accuracy: "1.5" });
    expect(ok(html)).toContain("disabled");
    expect(html).toContain("Enter an accuracy of at most half the thickness (1 mm).");
    expect(html).toMatch(/<input[^>]*aria-invalid="true"[^>]*value="1.5"/);
    html = form({ ...DEFAULT_FIELDS, thickness: "" });
    expect(html).toContain("Enter a shell thickness greater than 0");
    html = form(DEFAULT_FIELDS, 0);
    expect(ok(html)).toContain("disabled");
    expect(html).toContain("Select a shown, unlocked part first.");
  });

  it("opens with the settings used last", () => {
    vi.stubGlobal(
      "localStorage",
      fakeStorage({ [SETTINGS_KEY]: JSON.stringify({ kind: "outer-offset", thickness: 1.5, accuracy: 0.4, smooth: true, removeOriginal: true }) }),
    );
    const ids = useStore.getState().addParts([{ name: "A", mesh: makeBox([10, 10, 10]), matrix: translation(0, 0, 5) }]);
    useStore.getState().select(ids);
    const html = renderToStaticMarkup(<GenerateShellDialog onClose={() => {}} />);
    expect(html).toContain('<option value="outer-offset" selected="">Outer offset</option>');
    expect(html).toMatch(/<input[^>]*value="1.5"/);
    expect(html).toMatch(/<input[^>]*value="0.4"/);
    expect([...html.matchAll(/<input type="checkbox"[^>]*>/g)].map((m) => m[0]).every((c) => c.includes('checked=""'))).toBe(true);
  });
});
