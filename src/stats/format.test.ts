import { describe, expect, it } from "vitest";
import { cellText, exportCells, percentText, sortRows, statusText, tableRows, toCsv, toTsv, VALID_MESH } from "./format";
import type { PartStats, StatsSummary } from "./ipc";

const cube: PartStats = {
  triangles: 12,
  edges: 18,
  points: 8,
  shells: 1,
  volume: 8000,
  area: 2400,
  min: [10, 20, 0],
  max: [30, 40, 20],
  closed: true,
  oriented: true,
  holes: 0,
  borderEdges: 0,
  nonManifoldEdges: 0,
  invertedTriangles: 0,
  degenerateTriangles: 0,
};
const open: PartStats = { ...cube, volume: null, closed: false, holes: 2, borderEdges: 8, area: 123456.7, triangles: 1_234_567 };
const parts = [
  { id: "part-1", name: "Bracket, left", index: 0 },
  { id: "part-2", name: "Lid", index: 1 },
];
const summary: StatsSummary = {
  parts: 2,
  min: [10, 20, 0],
  max: [30, 40, 20],
  onPlatform: 2,
  volume: 8000,
  area: 2400,
  packingDensity: 0.1,
  withObjectHeight: 1.0005,
  top: 20,
};

describe("platform statistics rows", () => {
  it("say Valid mesh, or what is wrong in the parts tree badge's words", () => {
    expect(statusText(cube)).toBe(VALID_MESH);
    expect(statusText(open)).toBe("2 holes");
    expect(statusText({ ...cube, oriented: false, invertedTriangles: 3 })).toBe("3 inverted triangles");
    expect(statusText({ ...cube, oriented: false })).toBe("The mesh is not oriented");
  });

  it("turn Rust's numbers into the Information panel's units and formats", () => {
    const [a, b] = tableRows(parts, [cube, open]);
    expect(a).toMatchObject({ id: 1, name: "Bracket, left", length: 20, width: 20, height: 20, volume: 8, area: 24, valid: true, origin: [10, 20, 0] });
    expect(["length", "volume", "area", "triangles", "status"].map((k) => cellText(a, k as never))).toEqual(["20.00", "8.000", "24.000", "12", "Valid mesh"]);
    expect([cellText(b, "volume"), cellText(b, "area"), cellText(b, "triangles"), b.valid]).toEqual(["—", "1,234.567", "1,234,567", false]);
    expect(percentText(1.0005)).toBe("1.00 %");
    expect(percentText(null)).toBe("—");
  });

  it("sort by any column either way, with unknown values last", () => {
    const rows = tableRows(parts, [cube, open]);
    expect(sortRows(rows, "volume", "asc").map((r) => r.id)).toEqual([1, 2]);
    expect(sortRows(rows, "volume", "desc").map((r) => r.id)).toEqual([1, 2]);
    expect(sortRows(rows, "triangles", "desc").map((r) => r.id)).toEqual([2, 1]);
    expect(sortRows(rows, "name", "asc").map((r) => r.name)).toEqual(["Bracket, left", "Lid"]);
    expect(sortRows(rows, "name", "desc").map((r) => r.name)).toEqual(["Lid", "Bracket, left"]);
  });
});

describe("Export data", () => {
  const rows = tableRows(parts, [cube, open]);

  it("writes the table with plain numbers and the box origin, quoting names with commas", () => {
    const csv = toCsv(exportCells({ layout: "table", namesOnly: false }, rows, summary, [200, 200, 200]));
    expect(csv.split("\r\n")).toEqual([
      "Part,ID,Status,Length (mm),Width (mm),Height (mm),Volume (cm³),Area (cm²),Triangles,Edges,Points,Origin X (mm),Origin Y (mm),Origin Z (mm)",
      '"Bracket, left",1,Valid mesh,20.00,20.00,20.00,8.000,24.000,12,18,8,10.00,20.00,0.00',
      "Lid,2,2 holes,20.00,20.00,20.00,,1234.567,1234567,18,8,10.00,20.00,0.00",
      "",
    ]);
  });

  it("writes only the names, the table and the summary, or the summary alone", () => {
    expect(exportCells({ layout: "table", namesOnly: true }, rows, summary, [200, 200, 200])).toEqual([["Part"], ["Bracket, left"], ["Lid"]]);
    const both = exportCells({ layout: "table-summary", namesOnly: true }, rows, summary, [200, 210, 220]);
    expect(both.slice(3)).toEqual([
      [],
      ["Part positions", "X", "Y", "Z"],
      ["Minimum (mm)", "10.00", "20.00", "0.00"],
      ["Maximum (mm)", "30.00", "40.00", "20.00"],
      ["Size (mm)", "20.00", "20.00", "20.00"],
      [],
      ["Platform", "X", "Y", "Z"],
      ["Platform size (mm)", "200.00", "210.00", "220.00"],
      ["Volume (cm³)", "8.000"],
      ["Area (cm²)", "24.000"],
      ["Packing density (%)", "0.10"],
      ["With object height (%)", "1.00"],
      ["Parts counted", "2 of 2"],
    ]);
    expect(exportCells({ layout: "summary", namesOnly: true }, rows, summary, [200, 200, 200])[0]).toEqual(["Part positions", "X", "Y", "Z"]);
  });

  it("copies tab-separated text for spreadsheets", () => {
    expect(toTsv([["a\tb", "c"], ["d\ne"]])).toBe("a b\tc\nd e\n");
    expect(toCsv([['say "hi"', "x"]])).toBe('"say ""hi""",x\r\n');
  });
});
