import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { IDENTITY, makeBox } from "../mesh";
import { primeCheck } from "../repair/badges";
import type { RepairCheck } from "../repair/types";
import type { Part } from "../store";
import { PartRow, RepairRow, rowCount, rowsInRange, visibleRange, type PartRowProps } from "./PartsTree";

describe("visibleRange", () => {
  it("draws every row of a short list", () => {
    expect(visibleRange(5, 0, 600)).toEqual([0, 5]);
    expect(visibleRange(0, 0, 600)).toEqual([0, 0]);
  });

  it("draws the rows in view plus a margin on each side of a long list", () => {
    // 26 px rows below the 6 px padding and the 26 px Parts row: row 1000 starts at 26032.
    const [first, end] = visibleRange(13431, 26032, 260);
    expect(first).toBe(1000 - 20);
    expect(end).toBe(1010 + 20);
  });

  it("stays inside the list when scrolled past its end, as after an undo shrinks it", () => {
    expect(visibleRange(1, 100000, 600)).toEqual([1, 1]);
    const [first, end] = visibleRange(13431, 13431 * 26, 600);
    expect(end).toBe(13431);
    expect(13431 - first).toBeLessThan(23 + 2 * 20);
  });
});

describe("the Part repair row in the windowed list", () => {
  const kinds = (rows: { kind: string; index: number }[]) => rows.map((r) => (r.kind === "repair" ? `R${r.index}` : String(r.index)));

  it("adds one row while a listed part is repaired, and none otherwise", () => {
    expect(rowCount(5, -1)).toBe(5);
    expect(rowCount(5, 0)).toBe(6);
    expect(rowCount(5, 4)).toBe(6);
    // A repaired part that is not in the list (removed meanwhile) has no row to sit under.
    expect(rowCount(5, 5)).toBe(5);
    expect(rowCount(0, 0)).toBe(0);
  });

  it("puts the row right after the repaired part and shifts the parts below it by one", () => {
    expect(kinds(rowsInRange(5, -1, 0, 5))).toEqual(["0", "1", "2", "3", "4"]);
    expect(kinds(rowsInRange(5, 2, 0, 6))).toEqual(["0", "1", "2", "R2", "3", "4"]);
    expect(kinds(rowsInRange(5, 0, 0, 6))).toEqual(["0", "R0", "1", "2", "3", "4"]);
    expect(kinds(rowsInRange(5, 4, 0, 6))).toEqual(["0", "1", "2", "3", "4", "R4"]);
    expect(kinds(rowsInRange(1, 0, 0, 2))).toEqual(["0", "R0"]);
  });

  it("maps a window in the middle of the list, on either side of the repaired part", () => {
    expect(kinds(rowsInRange(10, 3, 2, 6))).toEqual(["2", "3", "R3", "4"]);
    expect(kinds(rowsInRange(10, 3, 4, 7))).toEqual(["R3", "4", "5"]);
    expect(kinds(rowsInRange(10, 3, 6, 11))).toEqual(["5", "6", "7", "8", "9"]);
    // The window stays inside the list, so nothing points past the last part.
    expect(kinds(rowsInRange(10, 3, 8, 50))).toEqual(["7", "8", "9"]);
    expect(rowsInRange(10, -1, -5, 2)).toEqual([
      { kind: "part", index: 0 },
      { kind: "part", index: 1 },
    ]);
  });

  it("windows a long list with the extra row: the rows in view are one part behind below it", () => {
    const parts = 13431;
    const rows = rowCount(parts, 5);
    // List row 1000 starts at 26032 px.
    const [first, end] = visibleRange(rows, 26032, 260);
    expect([first, end]).toEqual([980, 1030]);
    const drawn = rowsInRange(parts, 5, first, end);
    expect(drawn).toHaveLength(50);
    expect(drawn[20]).toEqual({ kind: "part", index: 999 });
    expect(drawn.some((r) => r.kind === "repair")).toBe(false);
    // Scrolled to the very end, the last row is the last part.
    const [, last] = visibleRange(rows, rows * 26, 600);
    expect(last).toBe(rows);
    expect(rowsInRange(parts, 5, last - 1, last)).toEqual([{ kind: "part", index: parts - 1 }]);
  });

  it("draws the row when the repaired part is in view", () => {
    const [first, end] = visibleRange(rowCount(13431, 1000), 26032, 260);
    const drawn = rowsInRange(13431, 1000, first, end);
    expect(drawn[20]).toEqual({ kind: "part", index: 1000 });
    expect(drawn[21]).toEqual({ kind: "repair", index: 1000 });
    expect(drawn[22]).toEqual({ kind: "part", index: 1001 });
  });
});

const part = (over: Partial<Part> = {}): Part => ({
  id: "part-1",
  name: "Bracket",
  mesh: makeBox([20, 20, 20]),
  matrix: [...IDENTITY],
  visible: true,
  transparent: false,
  locked: false,
  color: "#a9b1bc",
  ...over,
});

const rowProps = (p: Part, over: Partial<PartRowProps> = {}): PartRowProps => ({
  part: p,
  position: 1,
  count: 1,
  selected: false,
  repairing: false,
  editing: false,
  tabbable: true,
  onFocus: () => {},
  onStartRename: () => {},
  onEndRename: () => {},
  ...over,
});

const good: RepairCheck = { closed: true, oriented: true, holes: 0, borderEdges: 0, nonManifoldEdges: 0, invertedTriangles: 0 };

describe("PartRow", () => {
  it("shows no badge before the part was checked, or when it is closed and oriented", () => {
    const unchecked = part();
    expect(renderToStaticMarkup(createElement(PartRow, rowProps(unchecked)))).not.toContain("ptree-badge");
    const fine = part();
    primeCheck(fine.mesh, good);
    expect(renderToStaticMarkup(createElement(PartRow, rowProps(fine)))).not.toContain("ptree-badge");
  });

  it("shows a warning badge with the problems as its tooltip and accessible name", () => {
    const broken = part();
    primeCheck(broken.mesh, { ...good, closed: false, oriented: false, holes: 4, borderEdges: 40, invertedTriangles: 80437 });
    const html = renderToStaticMarkup(createElement(PartRow, rowProps(broken)));
    const badge = html.match(/<span class="ptree-badge"[^>]*>/)?.[0] ?? "";
    expect(badge).toContain('role="img"');
    expect(badge).toContain('aria-label="Needs repair: 4 holes, 80,437 inverted triangles"');
    expect(badge).toContain('title="Needs repair: 4 holes, 80,437 inverted triangles\nDouble-click to repair."');
    expect(badge).not.toContain("tabindex");
    // Between the name and the lock button, with the warning icon inside.
    expect(html.indexOf("ptree-name")).toBeLessThan(html.indexOf("ptree-badge"));
    expect(html.indexOf("ptree-badge")).toBeLessThan(html.indexOf("ptree-lock"));
    expect(html).toMatch(/<span class="ptree-badge"[^>]*><svg[^>]*lucide-triangle-alert/);
  });

  it("marks the repaired part expanded, since its Part repair row follows", () => {
    expect(renderToStaticMarkup(createElement(PartRow, rowProps(part(), { repairing: true })))).toContain('aria-expanded="true"');
    expect(renderToStaticMarkup(createElement(PartRow, rowProps(part())))).not.toContain("aria-expanded");
  });
});

describe("RepairRow", () => {
  it("is a third-level tree item named Part repair, in the Tab order only when it is the active row", () => {
    const html = renderToStaticMarkup(createElement(RepairRow, { part: part(), tabbable: true, onFocus: () => {} }));
    expect(html).toMatch(/^<div class="ptree-row ptree-repair" role="treeitem" aria-level="3" aria-posinset="1" aria-setsize="1" tabindex="0"/);
    expect(html).toContain('title="Working copy of Bracket"');
    expect(html).toContain('<span class="ptree-name">Part repair</span>');
    const untabbable = renderToStaticMarkup(createElement(RepairRow, { part: part(), tabbable: false, onFocus: () => {} }));
    expect(untabbable).toContain('tabindex="-1"');
  });
});
