import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { makeBox, translation, type Bounds } from "../mesh";
import type { PlaneChoice } from "../ops/planes";
import type { Part } from "../store";
import { CutDialog, CutForm, checkCut, defaultCutFields, positionText } from "./CutDialog";

// The dialog only needs the parts a cut would act on; running the cut needs the desktop app.
const ops = vi.hoisted(() => ({ targets: [] as Part[] }));
vi.mock("../ops/meshOps", () => ({ cutTargets: () => ops.targets, runCut: vi.fn() }));

/** A 20 mm cube spanning min..min+20 on every axis. */
function cube(id: string, min: [number, number, number]): Part {
  const [x, y, z] = min;
  return { id, name: id, mesh: makeBox([20, 20, 20]), matrix: translation(x + 10, y + 10, z + 10), visible: true, transparent: false, locked: false, color: "#a9b1bc" };
}

const BOX: Bounds = { min: [0, 0, 0], max: [40, 20, 30] };
const Z_PLANE: PlaneChoice = { axis: "z", label: "Z plane at 12.00 mm", plane: { normal: [0, 0, 1], offset: 12 } };
const X_PLANE: PlaneChoice = { axis: "x", label: "X plane at 5.00 mm", plane: { normal: [1, 0, 0], offset: 5 } };

const form = (planes: PlaneChoice[], targets = 1) =>
  renderToStaticMarkup(<CutForm planes={planes} box={BOX} scope="selected" targets={targets} onClose={() => {}} />);

/** The footer's Execute button tag, to read its disabled attribute. */
const execute = (html: string) => html.match(/<button[^>]*>Execute<\/button>/)?.[0] ?? "";
const radios = (html: string) => [...html.matchAll(/<input type="radio"[^>]*>/g)].map((m) => m[0]);
/** The checkbox tag right before the label `text`. */
const inputBefore = (html: string, text: string) =>
  html.match(new RegExp(`(<input type="checkbox"[^>]*>)(<span>)?<span class="dlg-check-label">${text}<`))?.[1] ?? "";

beforeEach(() => {
  ops.targets = [];
});

describe("CutDialog without clip planes", () => {
  it("asks for an axis and a position, defaulting to Z through the centre of the parts to cut", () => {
    ops.targets = [cube("a", [0, 0, 0]), cube("b", [40, 0, 4])];
    const html = renderToStaticMarkup(<CutDialog onClose={() => {}} />);
    expect(html).toMatch(/class="dlg-title">Cut<\/h2>/);
    expect(html).toContain('<option value="z" selected="">Z</option>');
    // Box z 0..24 → centre 12.
    expect(html).toMatch(/<input[^>]*class="dlg-input tf-num"[^>]*value="12"/);
    expect(html).toContain('<option value="selected" selected="">Cut selected parts</option>');
    expect(html).toContain("Cut all parts");
    expect(execute(html)).not.toContain("disabled");
    expect(html).not.toContain("dlg-error");
  });

  it("starts with Close cut surfaces on and separation off at 5 mm", () => {
    ops.targets = [cube("a", [0, 0, 0])];
    const html = renderToStaticMarkup(<CutDialog onClose={() => {}} />);
    expect(inputBefore(html, "Close cut surfaces")).toContain('checked=""');
    expect(html).toContain("Fill the cut so each piece is a closed solid.");
    const separate = inputBefore(html, "Separate cut parts by");
    expect(separate).toMatch(/^<input/);
    expect(separate).not.toContain("checked");
    const gap = html.match(/<input[^>]*aria-label="Separation in mm"[^>]*>/)?.[0] ?? "";
    expect(gap).toContain('value="5"');
    expect(gap).toContain('disabled=""');
    expect(html).toContain("Execute");
    expect(html).toContain("Cancel");
  });

  it("keeps Execute disabled with a reason when there is nothing to cut", () => {
    const html = renderToStaticMarkup(<CutDialog onClose={() => {}} />);
    expect(execute(html)).toContain('disabled=""');
    expect(html).toContain("Select a shown, unlocked part to cut.");
    // Falls back to the scene box: the 200 mm platform, centre 100.
    expect(html).toMatch(/class="dlg-input tf-num"[^>]*value="100"/);
  });

  it("dims the window lightly and sits aside so the viewport preview stays visible", () => {
    ops.targets = [cube("a", [0, 0, 0])];
    const html = renderToStaticMarkup(<CutDialog onClose={() => {}} />);
    expect(html).toContain('class="dlg-backdrop dlg-backdrop-aside"');
    expect(html).toContain('class="dlg dlg-cut"');
  });
});

describe("CutForm with clip planes on", () => {
  it("preselects the only enabled clip plane and allows Execute", () => {
    const html = form([Z_PLANE]);
    expect(html).toContain('role="radiogroup"');
    expect(radios(html)).toHaveLength(1);
    expect(radios(html)[0]).toContain('checked=""');
    expect(html).toContain("Z plane at 12.00 mm");
    expect(html).not.toContain("Several clip planes are on");
    expect(html).not.toContain("Position");
    expect(execute(html)).not.toContain("disabled");
  });

  it("asks which plane when several are on, and keeps Execute disabled until one is picked", () => {
    const html = form([X_PLANE, Z_PLANE]);
    expect(html).toContain("Several clip planes are on. Pick the one to cut with.");
    expect(radios(html)).toHaveLength(2);
    expect(radios(html).some((r) => r.includes("checked"))).toBe(false);
    expect(execute(html)).toContain('disabled=""');
  });
});

describe("checkCut", () => {
  const fields = defaultCutFields([]);

  it("cuts across the axis at the unrounded box centre while the position is untouched", () => {
    const box: Bounds = { min: [0, 0, 0], max: [10, 10, 10 / 3] };
    expect(positionText(fields, box)).toBe("1.667");
    const c = checkCut(fields, [], box, "all", 2);
    expect(c.options).toEqual({ plane: { normal: [0, 0, 1], offset: 5 / 3 }, scope: "all", cap: true, gap: 0 });
    expect(c.problem).toBeNull();
  });

  it("reads a typed position on the chosen axis, with a decimal comma", () => {
    const c = checkCut({ ...fields, axis: "x", position: "12,5" }, [], BOX, "selected", 1);
    expect(c.options?.plane).toEqual({ normal: [1, 0, 0], offset: 12.5 });
  });

  it("refuses a position that is not a number or out of range, and drops the preview", () => {
    for (const position of ["", "abc", "1e9"]) {
      const c = checkCut({ ...fields, position }, [], BOX, "selected", 1);
      expect(c.options).toBeNull();
      expect(c.plane).toBeNull();
      expect(c.bad).toBe("position");
    }
    expect(checkCut({ ...fields, position: "x" }, [], BOX, "selected", 1).problem).toBe("Enter a number for the position.");
    expect(checkCut({ ...fields, position: "200000" }, [], BOX, "selected", 1).problem).toBe("Enter a position from -100000 to 100000 mm.");
  });

  it("needs a positive separation only while Separate is ticked, and keeps the preview meanwhile", () => {
    for (const gap of ["0", "-2", "x", ""]) {
      const c = checkCut({ ...fields, separate: true, gap }, [], BOX, "selected", 1);
      expect(c.options).toBeNull();
      expect(c.bad).toBe("gap");
      expect(c.problem).toBe("Enter a distance greater than 0 and up to 100000 mm.");
      expect(c.plane).toEqual({ normal: [0, 0, 1], offset: 15 });
    }
    expect(checkCut({ ...fields, separate: false, gap: "x" }, [], BOX, "selected", 1).options?.gap).toBe(0);
    expect(checkCut({ ...fields, separate: true, gap: "2.5 mm", cap: false }, [], BOX, "selected", 1).options).toMatchObject({ gap: 2.5, cap: false });
  });

  it("uses the picked clip plane as it is, ignoring the axis fields", () => {
    const planes = [X_PLANE, Z_PLANE];
    expect(checkCut({ ...defaultCutFields(planes), position: "junk" }, planes, BOX, "selected", 1)).toEqual({
      options: null,
      plane: null,
      problem: null,
      bad: null,
    });
    const c = checkCut({ ...defaultCutFields(planes), picked: "x", position: "junk" }, planes, BOX, "selected", 1);
    expect(c.options?.plane).toBe(X_PLANE.plane);
    // A pick that is no longer listed counts as no pick.
    expect(checkCut({ ...defaultCutFields(planes), picked: "custom" }, planes, BOX, "selected", 1).plane).toBeNull();
  });

  it("says why when the scope has no parts to cut, keeping the preview", () => {
    expect(checkCut(fields, [], BOX, "selected", 0)).toMatchObject({ options: null, problem: "Select a shown, unlocked part to cut.", bad: null });
    const all = checkCut(fields, [], BOX, "all", 0);
    expect(all.problem).toBe("There are no shown, unlocked parts to cut.");
    expect(all.plane).toEqual({ normal: [0, 0, 1], offset: 15 });
  });

  it("shows the box centre on the chosen axis until a position is typed", () => {
    expect(positionText({ ...fields, axis: "x" }, BOX)).toBe("20");
    expect(positionText({ ...fields, axis: "y" }, BOX)).toBe("10");
    expect(positionText({ ...fields, position: "7" }, BOX)).toBe("7");
  });
});
