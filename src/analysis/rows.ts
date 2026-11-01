// An analysis's results as tables: the right panel shows them, Copy and Export to CSV write the same cells. Pure: no
// React, three or Tauri. Volumes and areas are formatted as the Information panel formats them, so the numbers agree.
import { formatCount, formatFixed2, formatFixed3 } from "../info";
import type { Vec3 } from "../store";
import { KIND_LABELS, type PartAnalysis, type SkinTotals, type Spread } from "./types";

/** One table of results: a header row, then rows whose first cell names the quantity. */
export interface ResultTable {
  columns: string[];
  rows: string[][];
}

const AXES = ["X", "Y", "Z"];
const yesNo = (v: boolean) => (v ? "Yes" : "No");
const mm = (v: number) => formatFixed2(v);
const cm3 = (mm3: number) => formatFixed3(mm3 / 1000);
const cm2 = (mm2: number) => formatFixed3(mm2 / 100);
const degrees = (v: number) => formatFixed2(v);
const vec = (label: string, v: Vec3, f: (n: number) => string = mm) => [label, ...v.map(f)];

/** What a cell says when its value could not be measured, and why. */
export const NOT_MEASURABLE = {
  open: "not measurable, surface is open",
  empty: "not measurable, no triangles",
  noEdges: "not measurable, no edges",
  skinOff: "not analysed, turned off in the settings",
  noWall: "not measurable, no surface behind any triangle",
  stopped: "not measured, the search stopped early",
} as const;

function spreadRow(label: string, s: Spread, f: (n: number) => string, missing: string | null): string[] {
  if (missing !== null) return [label, missing, "", "", ""];
  return [label, f(s.min), f(s.max), f(s.average), f(s.deviation)];
}

function skinCells(t: SkinTotals): [string, string, string] {
  if (!t.analyzed) return [NOT_MEASURABLE.skinOff, "", ""];
  return [degrees(t.angle), cm2(t.area), formatCount(t.components)];
}

export function resultTables(a: PartAnalysis): ResultTable[] {
  switch (a.kind) {
    case "standard": {
      const r = a.result;
      const edgeless = r.triangles === 0 ? NOT_MEASURABLE.empty : r.edges === 0 ? NOT_MEASURABLE.noEdges : null;
      return [
        { columns: ["Position (mm)", ...AXES], rows: [vec("Minimum", r.min), vec("Maximum", r.max), vec("Size", r.size)] },
        {
          columns: ["Quantity", "Value"],
          rows: [
            ["Volume (cm³)", r.volumeValid ? cm3(r.volume) : NOT_MEASURABLE.open],
            ["Area (cm²)", cm2(r.area)],
            ["Points", formatCount(r.points)],
            ["Triangles", formatCount(r.triangles)],
            ["Edges", formatCount(r.edges)],
            ["Shells", formatCount(r.shells)],
            ["Holes", formatCount(r.holes)],
            ["Boundary edges", formatCount(r.boundaryEdges)],
            ["Boundary length (mm)", mm(r.boundaryLength)],
            ["Flipped triangles", formatCount(r.flippedTriangles)],
            ["Bad edges", formatCount(r.badEdges)],
            ["Surface is closed", yesNo(r.closed)],
            ["Surface is orientable", yesNo(r.orientable)],
          ],
        },
        {
          columns: ["Mesh", "Min", "Max", "Avg", "Dev"],
          rows: [
            spreadRow("Edges per point", r.edgesPerPoint, formatFixed2, edgeless),
            spreadRow("Triangles per edge", r.trianglesPerEdge, formatFixed2, edgeless),
            spreadRow("Triangle quality", r.triangleQuality, formatFixed2, r.triangles === 0 ? NOT_MEASURABLE.empty : null),
            spreadRow("Edge length (mm)", r.edgeLength, mm, edgeless),
          ],
        },
      ];
    }
    case "gravity": {
      const r = a.result;
      const from: Vec3 = [0, 1, 2].map((k) => r.centre[k] - r.min[k]) as Vec3;
      const share: Vec3 = [0, 1, 2].map((k) => (r.size[k] > 0 ? (from[k] / r.size[k]) * 100 : 50)) as Vec3;
      return [
        {
          columns: ["", ...AXES],
          rows: [
            vec("Centre of gravity (mm)", r.centre),
            vec("From the box's minimum (mm)", from),
            vec("Share of the box (%)", share),
            vec("Box size (mm)", r.size),
          ],
        },
        { columns: ["Quantity", "Value"], rows: [["Volume (cm³)", cm3(r.volume)]] },
      ];
    }
    case "shadow":
      return [
        {
          columns: ["Quantity", "Value"],
          rows: [
            ["Shadow area (cm²)", cm2(a.result.area)],
            ["Shadow area (mm²)", mm(a.result.area)],
          ],
        },
      ];
    case "support": {
      const r = a.result;
      return [
        {
          columns: ["Quantity", "Value"],
          rows: [
            ["Threshold angle (°)", degrees(r.angle)],
            ["Support volume (cm³)", cm3(r.volume)],
            ["Area needing support (cm²)", cm2(r.area)],
            ["Triangles needing support", formatCount(r.triangles)],
          ],
        },
      ];
    }
    case "skin": {
      const up = skinCells(a.result.up);
      const down = skinCells(a.result.down);
      return [
        {
          columns: ["", "Upskins", "Downskins"],
          rows: [
            ["Threshold angle (°)", up[0], down[0]],
            ["Area (cm²)", up[1], down[1]],
            ["Components", up[2], down[2]],
          ],
        },
      ];
    }
    case "wall": {
      const r = a.result;
      const share = `${r.complete ? "" : "at least "}${formatFixed2(r.thinPercent)}`;
      const tables: ResultTable[] = [
        {
          columns: ["Quantity", "Value"],
          rows: [
            ["Test", r.passed ? "Passed" : r.decided === false ? "Not decided" : "Failed"],
            ["Wall thickness threshold (mm)", mm(r.threshold)],
            ["Acceptance: thinner surface allowed (%)", formatFixed2(r.acceptance)],
            ["Surface thinner than the threshold (%)", share],
            ["Thinner area (cm²)", cm2(r.thinArea)],
            ["Thinnest wall found (mm)", r.minThickness === null ? (r.complete ? NOT_MEASURABLE.noWall : NOT_MEASURABLE.stopped) : formatFixed3(r.minThickness)],
            ...(r.unmeasured > 0 ? [["Triangles with nothing behind them", formatCount(r.unmeasured)]] : []),
          ],
        },
      ];
      if (r.minFrom) tables.push({ columns: ["Thinnest wall at (mm)", ...AXES], rows: [vec("Surface point", r.minFrom)] });
      return tables;
    }
    case "removability": {
      const r = a.result;
      return [
        {
          columns: ["Quantity", "Value"],
          rows: [
            ["Minimum distance between items (mm)", mm(r.minDistance ?? 0)],
            ["Lifts out straight up", r.removable ? "Yes" : "No"],
            ["Bodies", formatCount(r.bodies)],
            ["Trapped bodies", formatCount(r.trappedBodies)],
            ["Trapped surface (cm²)", cm2(r.trappedArea)],
          ],
        },
      ];
    }
    case "compare": {
      const r = a.result;
      const sides = [
        ...(r.forward ? [{ label: "Reference → comparison", s: r.forward }] : []),
        ...(r.backward ? [{ label: "Comparison → reference", s: r.backward }] : []),
      ];
      const row = (label: string, f: (s: NonNullable<typeof r.forward>) => string) => [label, ...sides.map((x) => f(x.s))];
      const tables: ResultTable[] = [
        {
          columns: ["Distances (mm)", ...sides.map((x) => x.label)],
          rows: [
            row("Largest above", (s) => formatFixed3(s.maxPositive)),
            row("Largest below", (s) => formatFixed3(s.maxNegative)),
            row("Average (signed)", (s) => formatFixed3(s.averageSigned)),
            row("Largest (unsigned)", (s) => formatFixed3(s.maxUnsigned)),
            row("Average (unsigned)", (s) => formatFixed3(s.averageUnsigned)),
            row("Points compared", (s) => formatCount(s.points)),
          ],
        },
      ];
      if (r.fit) {
        tables.push({
          columns: ["Alignment", "Value"],
          rows: [
            ["Comparison turned by (°)", formatFixed2(r.fit.angle)],
            ["Remaining fit error (mm)", formatFixed3(r.fit.rms)],
          ],
        });
        tables.push({ columns: ["Then moved by (mm)", ...AXES], rows: [vec("Offset", r.fit.translation)] });
      }
      const probes = a.probes ?? [];
      if (probes.length > 0) {
        tables.push({
          columns: ["Measure points", "X", "Y", "Z", "Distance (mm)"],
          rows: probes.map((p, i) => [`Point ${i + 1}`, ...p.point.map(mm), formatFixed3(p.value)]),
        });
      }
      return tables;
    }
  }
}

/** Every cell Copy and Export to CSV write: the part and the analysis, then each table after an empty row. */
export function resultCells(a: PartAnalysis, partName: string): string[][] {
  const cells: string[][] = [
    ["Part", partName],
    ["Analysis", KIND_LABELS[a.kind]],
  ];
  for (const table of resultTables(a)) cells.push([], table.columns, ...table.rows);
  return cells;
}
