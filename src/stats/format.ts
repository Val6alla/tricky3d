// Platform statistics as the dialog shows it and as Export data writes it: the table rows, their sort order, the
// status words, and the CSV and clipboard text. Pure: Rust measured everything (src/stats/ipc.ts), this only lays it
// out, in the Information panel's units and number formats.
import { formatCount, formatFixed2, formatFixed3 } from "../info";
import { badgeText } from "../repair/badges";
import type { Vec3 } from "../store";
import type { PartStats, StatsSummary } from "./ipc";

export const VALID_MESH = "Valid mesh";

/** "Valid mesh", or what is wrong: "4 holes, 2 inverted triangles" (the parts tree badge's words). */
export function statusText(s: PartStats): string {
  const text = badgeText(s);
  if (text === null) return VALID_MESH;
  const what = text.replace(/^Needs repair: /, "");
  return what.charAt(0).toUpperCase() + what.slice(1);
}

/** A row of the parts table: numbers in mm, cm³ and cm², not yet formatted. */
export interface TableRow {
  /** 1-based place in the parts list (the ID column). */
  id: number;
  partId: string;
  name: string;
  status: string;
  valid: boolean;
  length: number | null;
  width: number | null;
  height: number | null;
  /** cm³, null when the mesh has holes. */
  volume: number | null;
  /** cm². */
  area: number;
  triangles: number;
  edges: number;
  points: number;
  /** The box's left-front-bottom corner (mm), for the export. */
  origin: Vec3 | null;
}

export type ColumnKey = "name" | "id" | "status" | "length" | "width" | "height" | "volume" | "area" | "triangles" | "edges" | "points";

export interface Column {
  key: ColumnKey;
  label: string;
  numeric: boolean;
}

export const COLUMNS: Column[] = [
  { key: "name", label: "Part", numeric: false },
  { key: "id", label: "ID", numeric: true },
  { key: "status", label: "Status", numeric: false },
  { key: "length", label: "Length (mm)", numeric: true },
  { key: "width", label: "Width (mm)", numeric: true },
  { key: "height", label: "Height (mm)", numeric: true },
  { key: "volume", label: "Volume (cm³)", numeric: true },
  { key: "area", label: "Area (cm²)", numeric: true },
  { key: "triangles", label: "Triangles", numeric: true },
  { key: "edges", label: "Edges", numeric: true },
  { key: "points", label: "Points", numeric: true },
];

/** The table rows for parts (in list order, `id` their place in it) and their stats, in the same order. */
export function tableRows(parts: readonly { id: string; name: string; index: number }[], stats: readonly PartStats[]): TableRow[] {
  return parts.map((p, i) => {
    const s = stats[i];
    const size = (axis: number) => (s.min && s.max ? s.max[axis] - s.min[axis] : null);
    return {
      id: p.index + 1,
      partId: p.id,
      name: p.name,
      status: statusText(s),
      valid: s.closed && s.oriented,
      length: size(0),
      width: size(1),
      height: size(2),
      // mm³ → cm³ and mm² → cm², as the Information panel shows them.
      volume: s.volume === null ? null : s.volume / 1000,
      area: s.area / 100,
      triangles: s.triangles,
      edges: s.edges,
      points: s.points,
      origin: s.min,
    };
  });
}

export type SortDirection = "asc" | "desc";

const collator = new Intl.Collator("en", { numeric: true, sensitivity: "base" });

/** The rows sorted by a column; rows without a value (an open part's volume) go last either way. Stable. */
export function sortRows(rows: readonly TableRow[], key: ColumnKey, direction: SortDirection): TableRow[] {
  const sign = direction === "asc" ? 1 : -1;
  return [...rows].sort((a, b) => {
    const x = a[key];
    const y = b[key];
    if (x === null || y === null) return x === y ? 0 : x === null ? 1 : -1;
    const order = typeof x === "number" && typeof y === "number" ? x - y : collator.compare(String(x), String(y));
    return sign * order;
  });
}

/** A table cell as the dialog shows it. */
export function cellText(row: TableRow, key: ColumnKey): string {
  const value = row[key];
  if (value === null) return "—";
  switch (key) {
    case "length":
    case "width":
    case "height":
      return formatFixed2(value as number);
    case "volume":
    case "area":
      return formatFixed3(value as number);
    case "triangles":
    case "edges":
    case "points":
      return formatCount(value as number);
    default:
      return String(value);
  }
}

/** "0.60 %", or "—". */
export function percentText(value: number | null): string {
  return value === null ? "—" : `${formatFixed2(value)} %`;
}

// ---------------------------------------------------------------------------------------------
// Export data

export type ExportLayout = "table" | "table-summary" | "summary";

export const EXPORT_LAYOUTS: { id: ExportLayout; label: string }[] = [
  { id: "table", label: "Parts table" },
  { id: "table-summary", label: "Parts table and platform summary" },
  { id: "summary", label: "Platform summary only" },
];

/** What Export data writes: the layout, and with `namesOnly` just the Part column of the table. */
export interface ExportChoice {
  layout: ExportLayout;
  namesOnly: boolean;
}

/** Plain numbers for files: no thousands separators, the panel's decimals, empty when unknown. */
const plain = (value: number | null, digits: number) => (value === null ? "" : value.toFixed(digits));

const AXES = ["X", "Y", "Z"] as const;

function tableCells(rows: readonly TableRow[], namesOnly: boolean): string[][] {
  if (namesOnly) return [["Part"], ...rows.map((r) => [r.name])];
  const header = [...COLUMNS.map((c) => c.label), ...AXES.map((a) => `Origin ${a} (mm)`)];
  const body = rows.map((r) => [
    r.name,
    String(r.id),
    r.status,
    plain(r.length, 2),
    plain(r.width, 2),
    plain(r.height, 2),
    plain(r.volume, 3),
    plain(r.area, 3),
    String(r.triangles),
    String(r.edges),
    String(r.points),
    ...AXES.map((_, k) => plain(r.origin ? r.origin[k] : null, 2)),
  ]);
  return [header, ...body];
}

function summaryCells(summary: StatsSummary, platform: Vec3): string[][] {
  const axis = (v: Vec3 | null, k: number) => plain(v ? v[k] : null, 2);
  const size = (k: number) => (summary.min && summary.max ? plain(summary.max[k] - summary.min[k], 2) : "");
  return [
    ["Part positions", ...AXES],
    ["Minimum (mm)", ...AXES.map((_, k) => axis(summary.min, k))],
    ["Maximum (mm)", ...AXES.map((_, k) => axis(summary.max, k))],
    ["Size (mm)", ...AXES.map((_, k) => size(k))],
    [],
    ["Platform", ...AXES],
    ["Platform size (mm)", ...platform.map((v) => plain(v, 2))],
    ["Volume (cm³)", plain(summary.volume === null ? null : summary.volume / 1000, 3)],
    ["Area (cm²)", plain(summary.area / 100, 3)],
    ["Packing density (%)", plain(summary.packingDensity, 2)],
    ["With object height (%)", plain(summary.withObjectHeight, 2)],
    ["Parts counted", `${summary.onPlatform} of ${summary.parts}`],
  ];
}

/** The cells Export data writes, row by row. */
export function exportCells(choice: ExportChoice, rows: readonly TableRow[], summary: StatsSummary, platform: Vec3): string[][] {
  switch (choice.layout) {
    case "table":
      return tableCells(rows, choice.namesOnly);
    case "table-summary":
      return [...tableCells(rows, choice.namesOnly), [], ...summaryCells(summary, platform)];
    case "summary":
      return summaryCells(summary, platform);
  }
}

/** RFC 4180 CSV: commas, CRLF line ends, a field quoted when it holds a comma, quote or line break. */
export function toCsv(cells: readonly string[][]): string {
  const field = (v: string) => (/[",\r\n]/.test(v) ? `"${v.replace(/"/g, '""')}"` : v);
  return cells.map((row) => row.map(field).join(",")).join("\r\n") + "\r\n";
}

/** Tab-separated text for the clipboard, so a paste into a spreadsheet fills one cell per value. */
export function toTsv(cells: readonly string[][]): string {
  return cells.map((row) => row.map((v) => v.replace(/[\t\r\n]+/g, " ")).join("\t")).join("\n") + "\n";
}
