// Generate shell's dialog fields as plain functions: the kinds and their words, checking what was typed, the settings
// remembered between runs (as Measure remembers its tool), the names of the new parts, and the estimate's sentence.
import { formatCount, linearDet, similarityScale2 } from "../info";
import { boundsOf } from "../mesh";
import { cachedStats } from "../meshCache";
import { loadSetting, saveSetting, type Part } from "../store";
import { formatField, parseField } from "../transformTools";
import type { EstimatePart, ShellEstimate, ShellKind, ShellRefusal, ShellSettings } from "./types";

export interface KindChoice {
  value: ShellKind;
  label: string;
  /** One sentence under the list, for the kind picked. */
  help: string;
  /** What the new part's name gets after the original's. */
  suffix: string;
}

export const KINDS: readonly KindChoice[] = [
  {
    value: "hollow-part",
    label: "Hollow part",
    help: "The part with a closed cavity inside, its walls as thick as set, as one part.",
    suffix: "hollow",
  },
  {
    value: "inner-offset",
    label: "Inner offset",
    help: "A slimmer part: every surface moved inward by the thickness.",
    suffix: "inner offset",
  },
  {
    value: "outer-offset",
    label: "Outer offset",
    help: "A fuller part: every surface moved outward by the thickness, outside edges rounded.",
    suffix: "outer offset",
  },
  {
    value: "hollowing-shell",
    label: "Hollowing shell",
    help: "Only the cavity's surface, facing inward. Merged with the part, it makes the part hollow.",
    suffix: "hollowing shell",
  },
  {
    value: "hollow-part-improved",
    label: "Hollow part, improved",
    help: "Like Hollow part, by a second method that keeps the cavity's sharp edges and leaves out cavity thinner than about two accuracy steps.",
    suffix: "hollow",
  },
];

export const kindChoice = (kind: ShellKind): KindChoice => KINDS.find((k) => k.value === kind) ?? KINDS[0];

/** What the dialog's fields hold: numbers as typed. */
export interface ShellFields {
  kind: ShellKind;
  thickness: string;
  accuracy: string;
  smooth: boolean;
  removeOriginal: boolean;
}

export const DEFAULT_FIELDS: ShellFields = {
  kind: "hollow-part",
  thickness: "2",
  accuracy: "0.25",
  smooth: false,
  removeOriginal: false,
};

/** Where the last settings used are remembered. */
export const SETTINGS_KEY = "tricky3d.generateShell";

/** Limits as Rust checks them (src-tauri/src/ops/shell.rs settings_problem). */
export const MAX_THICKNESS = 1000;
export const MIN_ACCURACY = 0.01;

/** The settings used last, or the defaults; whatever stored value does not fit is left at its default. */
export function loadFields(): ShellFields {
  const raw = loadSetting(SETTINGS_KEY);
  if (!raw) return { ...DEFAULT_FIELDS };
  let stored: unknown;
  try {
    stored = JSON.parse(raw);
  } catch {
    return { ...DEFAULT_FIELDS };
  }
  if (typeof stored !== "object" || stored === null) return { ...DEFAULT_FIELDS };
  const s = stored as Record<string, unknown>;
  const number = (v: unknown, fallback: string) => (typeof v === "number" && Number.isFinite(v) && v > 0 ? formatField(v) : fallback);
  const flag = (v: unknown, fallback: boolean) => (typeof v === "boolean" ? v : fallback);
  return {
    kind: KINDS.some((k) => k.value === s.kind) ? (s.kind as ShellKind) : DEFAULT_FIELDS.kind,
    thickness: number(s.thickness, DEFAULT_FIELDS.thickness),
    accuracy: number(s.accuracy, DEFAULT_FIELDS.accuracy),
    smooth: flag(s.smooth, DEFAULT_FIELDS.smooth),
    removeOriginal: flag(s.removeOriginal, DEFAULT_FIELDS.removeOriginal),
  };
}

/** Remembers the settings of a run. */
export function saveFields(settings: ShellSettings, removeOriginal: boolean): void {
  saveSetting(SETTINGS_KEY, JSON.stringify({ ...settings, removeOriginal }));
}

export interface FieldCheck {
  /** What OK runs; null keeps it disabled. */
  settings: ShellSettings | null;
  problem: string | null;
  bad: "thickness" | "accuracy" | null;
}

/** The settings the fields make, or what is wrong with them (the same rules and words as Rust's). */
export function checkFields(f: ShellFields): FieldCheck {
  const fail = (problem: string, bad: FieldCheck["bad"]): FieldCheck => ({ settings: null, problem, bad });
  const thickness = parseField(f.thickness);
  if (!(thickness > 0 && thickness <= MAX_THICKNESS)) return fail(`Enter a shell thickness greater than 0 and up to ${MAX_THICKNESS} mm.`, "thickness");
  const accuracy = parseField(f.accuracy);
  if (!(accuracy >= MIN_ACCURACY)) return fail(`Enter an accuracy of at least ${MIN_ACCURACY} mm.`, "accuracy");
  if (accuracy > thickness / 2) return fail(`Enter an accuracy of at most half the thickness (${formatField(thickness / 2)} mm).`, "accuracy");
  return { settings: { kind: f.kind, thickness, accuracy, smooth: f.smooth }, problem: null, bad: null };
}

/** "Bracket (hollow)": the original's name and what was made of it. */
export const resultName = (name: string, kind: ShellKind) => `${name} (${kindChoice(kind).suffix})`;

const plural = (n: number, one: string, many: string) => `${formatCount(n)} ${n === 1 ? one : many}`;

/** "“Bracket” is not closed (4 holes) and not oriented (12 inverted triangles)." */
export function refusalText(name: string, r: ShellRefusal): string {
  const parts: string[] = [];
  if (!r.closed) {
    const what = r.holes > 0 ? plural(r.holes, "hole", "holes") : plural(r.borderEdges + r.nonManifoldEdges, "open edge", "open edges");
    parts.push(`not closed (${what})`);
  }
  if (!r.oriented) {
    parts.push(r.invertedTriangles > 0 ? `not oriented (${plural(r.invertedTriangles, "inverted triangle", "inverted triangles")})` : "not oriented");
  }
  return `“${name}” is ${parts.join(" and ")}.`;
}

/** The parts' world boxes, surface areas and triangle counts, for the estimate. */
export function estimateParts(parts: readonly Part[]): EstimatePart[] {
  return parts.map((p) => {
    const b = boundsOf(p.mesh, p.matrix);
    const size: [number, number, number] = b ? [0, 1, 2].map((k) => b.max[k] - b.min[k]) as [number, number, number] : [0, 0, 0];
    const local = cachedStats(p.mesh)?.area;
    const scale2 = similarityScale2(p.matrix) ?? Math.abs(linearDet(p.matrix)) ** (2 / 3);
    // Before the stats arrive, the box's surface stands in for the part's.
    const area = local !== undefined ? local * scale2 : 2 * (size[0] * size[1] + size[1] * size[2] + size[2] * size[0]);
    return { size, area, triangles: p.mesh.indices.length / 3 };
  });
}

function duration(seconds: number): string {
  if (seconds < 5) return "a few seconds";
  if (seconds < 90) return `about ${Math.round(seconds / 5) * 5} seconds`;
  if (seconds < 90 * 60) return `about ${Math.round(seconds / 60)} minutes`;
  return "over an hour";
}

function memory(megabytes: number): string {
  if (megabytes < 1000) return `${Math.max(1, Math.round(megabytes / 10) * 10)} MB`;
  return `${(megabytes / 1000).toFixed(1)} GB`;
}

/** "About 2.1 million grid points: about 40 MB, a few seconds." */
export function estimateText(e: ShellEstimate): string {
  const points = e.points >= 1e9 ? `${(e.points / 1e9).toFixed(1)} billion` : e.points >= 1e6 ? `${(e.points / 1e6).toFixed(1)} million` : formatCount(Math.round(e.points));
  return `About ${points} grid points: ${memory(e.megabytes)} of memory, ${duration(e.seconds)}.`;
}

/** The warning OK gives before a run that is very slow or needs a lot of memory; null for one that is not. */
export function heavyWarning(e: ShellEstimate): string | null {
  if (!e.slow && !e.heavy) return null;
  const what = e.slow && e.heavy ? "will take a long time and need a lot of memory" : e.slow ? "will take a long time" : "will need a lot of memory";
  return `This run ${what}. A larger accuracy value is faster and lighter; you can cancel the run at any time.`;
}
