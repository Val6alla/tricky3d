import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { ShellRow } from "../types";
import {
  COLUMNS,
  DEFAULT_SORT,
  ShellTable,
  fullySelectedShells,
  measureText,
  nextSort,
  rowSelectMode,
  shellCells,
  shellsSummary,
  sortShells,
  tableKey,
  type ShellSort,
} from "./ShellsTab";

const row = (triangles: number, over: Partial<ShellRow> = {}): ShellRow => ({
  triangles,
  area: triangles * 10,
  bboxVolume: triangles * 100,
  volume: triangles * 50,
  watertight: true,
  ...over,
});

const sorted = (rows: ShellRow[], column: ShellSort["column"], descending: boolean) => sortShells(rows, { column, descending });

describe("sortShells", () => {
  it("starts with the biggest shells first and keeps Rust's order between equal ones", () => {
    const rows = [row(10), row(50), row(30), row(50)];
    expect(DEFAULT_SORT).toEqual({ column: "triangles", descending: true });
    expect(sortShells(rows, DEFAULT_SORT)).toEqual([1, 3, 2, 0]);
    expect(sorted(rows, "triangles", false)).toEqual([0, 2, 1, 3]);
  });

  it("sorts by area and by bounding box volume", () => {
    const rows = [row(1, { area: 5, bboxVolume: 1 }), row(2, { area: 1, bboxVolume: 9 }), row(3, { area: 3, bboxVolume: 4 })];
    expect(sorted(rows, "area", true)).toEqual([0, 2, 1]);
    expect(sorted(rows, "area", false)).toEqual([1, 2, 0]);
    expect(sorted(rows, "bboxVolume", true)).toEqual([1, 2, 0]);
  });

  it("puts shells without a volume last in either direction", () => {
    const rows = [row(1, { volume: null, watertight: false }), row(2, { volume: 7 }), row(3, { volume: null, watertight: false }), row(4, { volume: 2 })];
    expect(sorted(rows, "volume", true)).toEqual([1, 3, 0, 2]);
    expect(sorted(rows, "volume", false)).toEqual([3, 1, 0, 2]);
  });

  it("sorts Watertight with No first when ascending", () => {
    const rows = [row(1), row(2, { watertight: false, volume: null }), row(3), row(4, { watertight: false, volume: null })];
    expect(sorted(rows, "watertight", false)).toEqual([1, 3, 0, 2]);
    expect(sorted(rows, "watertight", true)).toEqual([0, 2, 1, 3]);
  });

  it("returns an empty order for no shells and does not touch the rows", () => {
    expect(sortShells([], DEFAULT_SORT)).toEqual([]);
    const rows = [row(3), row(1)];
    const copy = rows.map((r) => ({ ...r }));
    sortShells(rows, DEFAULT_SORT);
    expect(rows).toEqual(copy);
  });
});

describe("nextSort", () => {
  it("turns the sorted column around", () => {
    expect(nextSort({ column: "area", descending: true }, "area")).toEqual({ column: "area", descending: false });
    expect(nextSort({ column: "area", descending: false }, "area")).toEqual({ column: "area", descending: true });
  });

  it("starts a new number column with the biggest values, and Watertight with No", () => {
    for (const column of ["triangles", "area", "bboxVolume", "volume"] as const) {
      expect(nextSort({ column: "watertight", descending: false }, column)).toEqual({ column, descending: true });
    }
    expect(nextSort(DEFAULT_SORT, "watertight")).toEqual({ column: "watertight", descending: false });
  });
});

describe("measureText", () => {
  it("shows four significant digits, three decimals below 10", () => {
    expect(measureText(1.2)).toBe("1.200");
    expect(measureText(9.9994)).toBe("9.999");
    expect(measureText(12.346)).toBe("12.35");
    expect(measureText(120)).toBe("120.0");
    expect(measureText(912.3456)).toBe("912.3");
    expect(measureText(8123.456)).toBe("8,123");
    expect(measureText(81234.56)).toBe("81,235");
  });

  it("moves to fewer decimals where rounding would add a digit", () => {
    expect(measureText(9.9996)).toBe("10.00");
    expect(measureText(99.996)).toBe("100.0");
    expect(measureText(999.96)).toBe("1,000");
  });

  it("reads tiny but real measures as <0.001, and nothing as 0.000", () => {
    expect(measureText(0.00001)).toBe("<0.001");
    expect(measureText(0.0004999)).toBe("<0.001");
    expect(measureText(0)).toBe("0.000");
    expect(measureText(0.012)).toBe("0.012");
  });
});

describe("shellCells", () => {
  it("shows triangles with separators, mm² and mm³ as cm² and cm³, and Yes/No", () => {
    // 1234.6 mm² = 12.346 cm²; 8000 mm³ = 8 cm³; 12000 mm³ = 12 cm³.
    expect(shellCells({ triangles: 233412, area: 1234.6, bboxVolume: 8000, volume: 12000, watertight: true })).toEqual([
      "233,412",
      "12.35",
      "8.000",
      "12.00",
      "Yes",
    ]);
  });

  it("shows — for the volume of a shell that is not watertight", () => {
    expect(shellCells({ triangles: 10, area: 100, bboxVolume: 8000, volume: null, watertight: false })).toEqual(["10", "1.000", "8.000", "—", "No"]);
  });

  it("reads the fixture speck's measures as <0.001", () => {
    // 0.01 mm³ = 0.00001 cm³.
    expect(shellCells({ triangles: 12, area: 0.04, bboxVolume: 0.01, volume: 0.01, watertight: true })).toEqual(["12", "<0.001", "<0.001", "<0.001", "Yes"]);
    expect(shellCells({ triangles: 2, area: 0, bboxVolume: 0, volume: 0, watertight: true }).slice(1, 4)).toEqual(["0.000", "0.000", "0.000"]);
    // 0.5 mm³ rounds up to 0.001 cm³, so it shows as a number.
    expect(shellCells(row(1, { volume: 0.5 }))[3]).toBe("0.001");
  });

  it("keeps the widest cell of a big part short", () => {
    // A large model: 8,123 cm² of surface, a 9.1 litre box.
    const cells = shellCells({ triangles: 698112, area: 812345.6, bboxVolume: 9123456.7, volume: 4123456.7, watertight: true });
    expect(cells).toEqual(["698,112", "8,123", "9,123", "4,123", "Yes"]);
  });
});

describe("shellsSummary", () => {
  it("counts the shells and the ones that are not watertight", () => {
    expect(shellsSummary([row(1)])).toBe("1 shell");
    expect(shellsSummary([])).toBe("0 shells");
    const rows = [...Array.from({ length: 1130 }, (_, i) => row(i + 1)), ...Array.from({ length: 3 }, () => row(5, { watertight: false, volume: null }))];
    expect(shellsSummary(rows)).toBe("1,133 shells, 3 not watertight");
  });
});

describe("picked rows", () => {
  // Three shells: triangles 0-2 are row 0, 3-4 row 1, 5 row 2.
  const shellOf = new Uint32Array([0, 0, 0, 1, 1, 2]);
  const mask = (...ids: number[]) => {
    const out = new Uint8Array(6);
    for (const t of ids) out[t] = 1;
    return out;
  };
  const picked = (selection: Uint8Array) => [...fullySelectedShells(shellOf, selection, 3)].sort();

  it("are the shells whose every triangle is selected, however the selection was made", () => {
    expect(picked(mask(3, 4))).toEqual([1]);
    expect(picked(mask(0, 1, 2, 5))).toEqual([0, 2]);
    // Part of a shell (a click in the viewport) does not pick its row; nothing selected (Esc) picks none.
    expect(picked(mask(0, 1, 3, 4))).toEqual([1]);
    expect(picked(mask())).toEqual([]);
    expect(picked(mask(0, 1, 2, 3, 4, 5))).toEqual([0, 1, 2]);
    // A row no triangle belongs to is never picked.
    expect([...fullySelectedShells(shellOf, mask(0, 1, 2, 3, 4, 5), 4)].sort()).toEqual([0, 1, 2]);
  });

  it("decide what Shift+click does, so the table and the viewport agree: a picked row's shell goes, any other comes in", () => {
    expect(rowSelectMode(true, true)).toBe("remove");
    expect(rowSelectMode(false, true)).toBe("add");
    expect(rowSelectMode(true, false)).toBe("replace");
    expect(rowSelectMode(false, false)).toBe("replace");
  });
});

describe("tableKey", () => {
  const order = [4, 0, 2];
  const key = (k: string, cursor: number | null, shiftKey = false, onTable = true) => tableKey({ key: k, shiftKey }, onTable, order, cursor);

  it("moves the pick with the arrows, Home and End, in display order, and stops at the ends", () => {
    expect(key("ArrowDown", null)).toEqual({ row: 4, toggle: false });
    expect(key("ArrowDown", 4)).toEqual({ row: 0, toggle: false });
    expect(key("ArrowDown", 2)).toEqual({ row: 2, toggle: false });
    expect(key("ArrowUp", 0)).toEqual({ row: 4, toggle: false });
    expect(key("ArrowUp", null)).toEqual({ row: 4, toggle: false });
    expect(key("Home", 2)).toEqual({ row: 4, toggle: false });
    expect(key("End", null)).toEqual({ row: 2, toggle: false });
  });

  it("picks the cursor row again with Enter or Space, Shift adding or removing it", () => {
    expect(key("Enter", 0)).toEqual({ row: 0, toggle: false });
    expect(key(" ", 0, true)).toEqual({ row: 0, toggle: true });
    expect(key("Enter", null)).toBeNull();
    expect(key("a", 0)).toBeNull();
    expect(tableKey({ key: "ArrowDown", shiftKey: false }, true, [], null)).toBeNull();
  });

  it("leaves the keys of a focused column header to it: Enter and Space sort", () => {
    for (const k of ["Enter", " ", "ArrowDown", "Home"]) expect(key(k, 0, false, false)).toBeNull();
  });
});

describe("ShellTable", () => {
  const rows = [row(12), row(1200, { volume: null, watertight: false }), row(300)];
  const render = (over: Partial<Parameters<typeof ShellTable>[0]> = {}) =>
    renderToStaticMarkup(
      <ShellTable rows={rows} order={[1, 2, 0]} sort={DEFAULT_SORT} picked={new Set([2])} cursor={2} stale={false} onSort={() => {}} onPick={() => {}} {...over} />,
    );
  const cellRows = (html: string) => [...html.matchAll(/<tr id="[^"]+"[^>]*>(.*?)<\/tr>/g)].map((m) => [...m[1].matchAll(/<td>(.*?)<\/td>/g)].map((c) => c[1]));

  it("has the designed columns with their units, and marks the sorted one", () => {
    const html = render();
    const heads = [...html.matchAll(/<th scope="col" aria-sort="(\w+)">(.*?)<\/th>/g)];
    expect(heads.map((h) => h[1])).toEqual(["descending", "none", "none", "none", "none"]);
    const text = heads.map((h) => h[2].replace(/<svg.*?<\/svg>/g, "").replace(/<[^>]+>/g, " ").replace(/\s+/g, " ").replace(/­/g, "").trim());
    expect(text).toEqual(["Triangles", "Area cm²", "Box volume cm³", "Volume cm³", "Watertight"]);
    expect(COLUMNS.find((c) => c.id === "bboxVolume")?.title).toBe("Bounding box volume (cm³)");
    // Only the sorted column shows an arrow.
    expect(html.match(/<svg/g)).toHaveLength(1);
  });

  it("lets the keyboard reach every column header, so any column can be sorted without a mouse", () => {
    const buttons = [...render().matchAll(/<th [^>]*><button ([^>]*)>/g)].map((m) => m[1]);
    expect(buttons).toHaveLength(COLUMNS.length);
    for (const b of buttons) expect(b).not.toContain("tabindex");
  });

  it("draws the given rows in order, with picked, cursor and open shells marked", () => {
    const html = render();
    expect(cellRows(html)).toEqual([
      ["1,200", "120.0", "120.0", "—", "No"],
      ["300", "30.00", "30.00", "15.00", "Yes"],
      ["12", "1.200", "1.200", "0.600", "Yes"],
    ]);
    const trs = [...html.matchAll(/<tr id="([^"]+)"([^>]*)>/g)];
    expect(trs[0][2]).toContain('class="is-open"');
    expect(trs[1][2]).toContain('aria-selected="true"');
    expect(trs[1][2]).toContain('class="is-picked is-cursor"');
    expect(html).toContain(`aria-activedescendant="${trs[1][1]}"`);
    expect(html).toContain('aria-rowcount="4"');
    expect(html).not.toContain("is-stale");
  });

  it("fades a list that belongs to an earlier working copy", () => {
    expect(render({ stale: true, cursor: null })).toMatch(/^<div class="rp-table-box"><table class="rp-shells is-stale" role="grid"/);
    expect(render({ cursor: null })).not.toContain("aria-activedescendant");
  });
});
