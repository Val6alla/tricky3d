// Text for repair mode: parameter values as the editor shows them, the Status tab's marks and counts, the Shells
// tab's units, the selection info toast, and the toast after a run. Pure: no React, Tauri or store.
import { formatCount } from "../info";
import type { SelectionInfo } from "./selection";
import type { ActionReport, ActionSpec, ParamSpec, ParamValue, RepairStatus, ScriptStep } from "./types";

export { formatCount };

const numberFormats = new Map<string, Intl.NumberFormat>();

/**
 * A number with thousands separators and exactly `decimals` decimals; with none given, as many as it has (up to 6).
 * Minus zero (and anything that rounds to it) shows as plain zero.
 */
export function formatNumber(value: number, decimals?: number): string {
  const key = decimals === undefined ? "auto" : String(decimals);
  let format = numberFormats.get(key);
  if (!format) {
    format = new Intl.NumberFormat(
      "en-US",
      decimals === undefined ? { maximumFractionDigits: 6 } : { minimumFractionDigits: decimals, maximumFractionDigits: decimals },
    );
    numberFormats.set(key, format);
  }
  const text = format.format(value);
  return /^-[0.,]*$/.test(text) ? text.slice(1) : text;
}

export const yesNo = (value: boolean) => (value ? "Yes" : "No");

/**
 * A parameter value as the editor shows it: "0.010 mm" (the spec's decimals and unit), "Yes" / "No", or the
 * choice's label. A missing value shows the default.
 */
export function formatParam(spec: ParamSpec, value: ParamValue | undefined): string {
  const v = value ?? spec.default;
  switch (spec.kind) {
    case "float": {
      if (typeof v !== "number") return String(v);
      const text = formatNumber(v, spec.decimals);
      return spec.unit ? `${text} ${spec.unit}` : text;
    }
    case "bool":
      return typeof v === "boolean" ? yesNo(v) : String(v);
    case "choice":
      return spec.choices?.find((c) => c.id === v)?.label ?? String(v);
  }
}

/** A step's parameters in one line, "Tolerance 0.010 mm, Ignore orientation No"; "" for an action without any. */
export function stepParamsText(step: ScriptStep, spec: ActionSpec): string {
  return spec.params.map((p) => `${p.label} ${formatParam(p, step.params[p.id])}`).join(", ");
}

/** "Mesh is closed" / "Mesh is oriented": ✓, ✗, or ? while not computed. */
export function mark(value: boolean | null | undefined): "✓" | "✗" | "?" {
  if (value === null || value === undefined) return "?";
  return value ? "✓" : "✗";
}

export const closedMark = (status: RepairStatus | null) => mark(status?.closed);
export const orientedMark = (status: RepairStatus | null) => mark(status?.oriented);

/** A Status tab count with thousands separators, or "?" while not computed. */
export function statusCount(value: number | null | undefined): string {
  return value === null || value === undefined ? "?" : formatCount(value);
}

/** The Status tab's statistics, in display order. */
export function statusRows(status: RepairStatus | null): { label: string; text: string }[] {
  const rows: [string, keyof RepairStatus][] = [
    ["Edges", "edges"],
    ["Triangles", "triangles"],
    ["Shells", "shells"],
    ["Border edges", "borderEdges"],
    ["Inv. orientation", "invertedTriangles"],
    ["Holes", "holes"],
    ["Non-manifold edges", "nonManifoldEdges"],
  ];
  return rows.map(([label, key]) => ({ label, text: statusCount(status ? (status[key] as number) : null) }));
}

/** mm² as cm² with 3 decimals: 1234.5 → "12.345". */
export function cm2(mm2: number): string {
  return formatNumber(mm2 / 100, 3);
}

/** mm³ as cm³ with 3 decimals: 12000 → "12.000". */
export function cm3(mm3: number): string {
  return formatNumber(mm3 / 1000, 3);
}

/** The script picker's "11 repair actions". */
export function actionCountText(n: number): string {
  return `${formatCount(n)} ${n === 1 ? "repair action" : "repair actions"}`;
}

const plural = (n: number, one: string, many: string) => `${formatCount(n)} ${n === 1 ? one : many}`;

/** Show selection info: "1,204 triangles selected, area 12.345 cm², in 2 shells". */
export function selectionInfoText(info: SelectionInfo): string {
  if (info.count === 0) return "No triangles selected.";
  return `${plural(info.count, "triangle", "triangles")} selected, area ${cm2(info.area)} cm², in ${plural(info.shells, "shell", "shells")}`;
}

/** "850 ms", "12.3 s", "2 min 5 s". */
export function formatDuration(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`;
  const seconds = Math.round(ms / 1000);
  return `${Math.floor(seconds / 60)} min ${seconds % 60} s`;
}

/**
 * The toast after a run. One action: its own report ("Closed 4 holes, added 212 triangles."). A script: how many
 * actions changed the mesh and how long it took ("Extended repair finished in 4.2 s: 7 of 11 actions changed the
 * mesh.").
 */
export function runSummary(label: string, reports: readonly ActionReport[]): string {
  if (reports.length === 0) return `${label} had no actions to run.`;
  if (reports.length === 1) return reports[0].message || `${label} finished.`;
  const changed = reports.filter((r) => r.changed).length;
  const time = formatDuration(reports.reduce((sum, r) => sum + r.millis, 0));
  if (changed === 0) return `${label} finished in ${time}: nothing needed repairing.`;
  return `${label} finished in ${time}: ${changed} of ${reports.length} actions changed the mesh.`;
}

/** Every warning of a run, each once: the actions' in order, then the run's own. */
export function runWarnings(reports: readonly ActionReport[], warnings: readonly string[]): string[] {
  return [...new Set([...reports.flatMap((r) => r.warnings), ...warnings])];
}
