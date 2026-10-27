import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";

// Entering repair mode is the session's work; the dialog only hands over the part picked.
const session = vi.hoisted(() => ({ enterRepair: vi.fn() }));
vi.mock("../session", () => session);

import { IDENTITY, makeBox } from "../../mesh";
import type { Part } from "../../store";
import { primeCheck } from "../badges";
import type { RepairCheck } from "../types";
import { ChoosePartDialog, ChoosePartForm, choosePart, partChoices, type PartChoice } from "./ChoosePartDialog";

const good: RepairCheck = { closed: true, oriented: true, holes: 0, borderEdges: 0, nonManifoldEdges: 0, invertedTriangles: 0 };

const part = (id: string, name: string, color = "#a9b1bc"): Part => ({
  id,
  name,
  mesh: makeBox([20, 20, 20]),
  matrix: [...IDENTITY],
  visible: true,
  transparent: false,
  locked: false,
  color,
});

const CHOICES: PartChoice[] = [
  { id: "a", name: "Bracket", color: "#a9b1bc", triangles: 12, badge: null },
  { id: "b", name: "Bolt", color: "#b9ae9d", triangles: 1, badge: "Needs repair: 4 holes" },
];

const render = (choices: PartChoice[]) => renderToStaticMarkup(<ChoosePartForm choices={choices} onClose={() => {}} />);
const radios = (html: string) => [...html.matchAll(/<input type="radio"[^>]*>/g)].map((m) => m[0]);
const okButton = (html: string) => html.match(/<button[^>]*>Repair<\/button>/)?.[0] ?? "";

beforeEach(() => {
  session.enterRepair.mockReset();
});

describe("partChoices", () => {
  it("lists the given parts that still exist, in the order given, with their triangles and badge", () => {
    const a = part("a", "Bracket", "#111111");
    const b = part("b", "Bolt");
    const c = part("c", "Nut");
    primeCheck(a.mesh, { ...good, closed: false, holes: 2, borderEdges: 8 });
    primeCheck(c.mesh, good);
    expect(partChoices([a, b, c], ["c", "gone", "a", "b"])).toEqual([
      { id: "c", name: "Nut", color: "#a9b1bc", triangles: 12, badge: null },
      { id: "a", name: "Bracket", color: "#111111", triangles: 12, badge: "Needs repair: 2 holes" },
      // Not checked yet: no badge until the check arrives.
      { id: "b", name: "Bolt", color: "#a9b1bc", triangles: 12, badge: null },
    ]);
    expect(partChoices([a], [])).toEqual([]);
  });
});

describe("ChoosePartForm", () => {
  it("asks which part to repair, with the first one picked and Repair ready", () => {
    const html = render(CHOICES);
    expect(html).toMatch(/class="dlg-title">Repair part<\/h2>/);
    expect(html.startsWith('<div class="dlg-backdrop"><div class="dlg dlg-rp-choose" role="dialog"')).toBe(true);
    expect(html).toContain("Several parts are selected. Pick the one to repair.");
    expect(html).toMatch(/<div class="dlg-list" role="radiogroup" aria-labelledby="[^"]+">/);
    const inputs = radios(html);
    expect(inputs).toHaveLength(2);
    expect(inputs[0]).toContain('checked=""');
    expect(inputs[1]).not.toContain("checked");
    // One radio group.
    const names = inputs.map((r) => r.match(/name="([^"]+)"/)?.[1]);
    expect(names[0]).toBeTruthy();
    expect(names[1]).toBe(names[0]);
    expect(okButton(html)).not.toContain("disabled");
    expect(html).toContain(">Cancel</button>");
  });

  it("shows each part's name, colour, triangle count and damage badge", () => {
    const html = render(CHOICES);
    expect([...html.matchAll(/class="dlg-list-name">(.*?)<\/span>/g)].map((m) => m[1])).toEqual(["Bracket", "Bolt"]);
    expect(html).toContain('<span class="rp-swatch" style="background:#b9ae9d" aria-hidden="true"></span>');
    expect(html).toContain('<span class="rp-count">12 triangles</span>');
    expect(html).toContain('<span class="rp-count">1 triangle</span>');
    const badges = [...html.matchAll(/<span class="rp-badge"[^>]*>/g)].map((m) => m[0]);
    expect(badges).toEqual(['<span class="rp-badge" role="img" aria-label="Needs repair: 4 holes" title="Needs repair: 4 holes">']);
  });

  it("keeps Repair disabled when no part is left to choose", () => {
    const html = render([]);
    expect(okButton(html)).toContain('disabled=""');
    expect(radios(html)).toEqual([]);
  });
});

describe("ChoosePartDialog", () => {
  it("draws nothing when none of the parts exists any more", () => {
    // Server rendering reads the store's initial state, which has no parts.
    expect(renderToStaticMarkup(<ChoosePartDialog ids={["gone"]} onClose={() => {}} />)).toBe("");
  });
});

describe("choosePart", () => {
  it("closes the dialog first, then enters repair mode for the part", () => {
    const calls: string[] = [];
    session.enterRepair.mockImplementation((id: string) => calls.push(`repair ${id}`));
    choosePart("b", () => calls.push("close"));
    expect(calls).toEqual(["close", "repair b"]);
    expect(session.enterRepair).toHaveBeenCalledTimes(1);
  });
});
