import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";

// Applying is the session's work (one main undo step); the dialog only hands over the choice.
const session = vi.hoisted(() => ({ applyRepair: vi.fn() }));
vi.mock("../session", () => session);

import type { RepairStatus } from "../types";
import { ApplyRepairDialog, ApplyRepairForm, applyChoices, applyWarning, confirmApply } from "./ApplyRepairDialog";

const render = (props: { changed?: boolean; warning?: string | null } = {}) =>
  renderToStaticMarkup(<ApplyRepairForm partName="Bracket" changed={props.changed ?? true} warning={props.warning ?? null} onClose={() => {}} />);

/** The radio input right before the choice labelled `label`. */
const radioBefore = (html: string, label: string) =>
  html.match(new RegExp(`(<input type="radio"[^>]*>)<span><span class="dlg-check-label">${label}<`))?.[1] ?? "";

const FIXED: RepairStatus = {
  edges: 18,
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

beforeEach(() => {
  session.applyRepair.mockReset();
});

describe("ApplyRepairForm", () => {
  it("offers the three choices in order with Remove old part picked", () => {
    const html = render();
    expect(html).toMatch(/class="dlg-title">Apply repair<\/h2>/);
    expect([...html.matchAll(/class="dlg-check-label">(.*?)<\/span>/g)].map((m) => m[1])).toEqual([
      "Remove old part",
      "Keep old part",
      "Keep old part and compare",
    ]);
    expect(radioBefore(html, "Remove old part")).toContain('checked=""');
    expect(radioBefore(html, "Keep old part")).not.toContain("checked");
    expect(radioBefore(html, "Keep old part and compare")).not.toContain("checked");
    expect(html).toContain('role="radiogroup" aria-label="Apply repair"');
    expect(html).toMatch(/<button[^>]*class="dlg-btn dlg-btn-primary"[^>]*>Apply<\/button>/);
    expect(html).toContain(">Cancel</button>");
  });

  it("says what each choice does, naming the repaired copy", () => {
    const html = render();
    expect(html).toContain("The repaired part takes the old part&#x27;s place, name and colour.");
    expect(html).toContain("The repaired part is added after it as “Bracket (repaired)”.");
    expect(html).toContain("Both parts stay, with the old one made transparent.");
  });

  it("notes when nothing was changed, instead of any warning", () => {
    const html = render({ changed: false, warning: "Needs repair: 4 holes" });
    expect(html).toContain("Nothing was changed, so the part stays as it was.");
    expect(html).not.toContain("Needs repair");
  });

  it("warns about what the working copy still needs", () => {
    const html = render({ warning: "Needs repair: 4 holes, 12 inverted triangles" });
    expect(html).toMatch(/<p class="rp-warning"><svg[^>]*lucide-triangle-alert[^>]*>.*<\/svg>Needs repair: 4 holes, 12 inverted triangles<\/p>/);
    expect(render()).not.toContain("rp-warning");
    expect(render()).not.toContain("Nothing was changed");
  });
});

describe("applyChoices and applyWarning", () => {
  it("keeps the modes in the designed order", () => {
    expect(applyChoices("X").map((c) => c.mode)).toEqual(["remove", "keep", "compare"]);
  });

  it("is quiet while the status is unknown or fine, and names the problems otherwise", () => {
    expect(applyWarning(null)).toBeNull();
    expect(applyWarning(FIXED)).toBeNull();
    expect(applyWarning({ ...FIXED, closed: false, borderEdges: 9, holes: 2 })).toBe("Needs repair: 2 holes");
    expect(applyWarning({ ...FIXED, oriented: false, invertedTriangles: 6354 })).toBe("Needs repair: 6,354 inverted triangles");
  });
});

describe("ApplyRepairDialog", () => {
  it("draws nothing outside repair mode", () => {
    // Server rendering reads the repair store's initial state, which has no session.
    expect(renderToStaticMarkup(<ApplyRepairDialog onClose={() => {}} />)).toBe("");
  });
});

describe("confirmApply", () => {
  it("closes the dialog first, then applies the repair with the chosen mode", () => {
    const calls: string[] = [];
    session.applyRepair.mockImplementation((mode: string) => calls.push(`apply ${mode}`));
    for (const mode of ["remove", "keep", "compare"] as const) confirmApply(mode, () => calls.push("close"));
    expect(calls).toEqual(["close", "apply remove", "close", "apply keep", "close", "apply compare"]);
  });
});
