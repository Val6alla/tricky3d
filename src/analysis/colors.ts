// The colours an analysis paints the part with, by class (the classes Rust gives each triangle), and what each means.
// Fixed colours rather than theme tokens: green, red and blue carry the meaning in both themes.
import type { CompareSurface, CompareView, PartAnalysis, WallSettings } from "./types";

/** Colour class → colour. 0 is the part itself, drawn grey while an analysis is on show. */
export const ANALYSIS_COLORS = ["#a3a9b1", "#3aa757", "#d64541", "#3b6fd4"] as const;

export const PART_CLASS = 0;
export const GREEN_CLASS = 1;
export const RED_CLASS = 2;

/** The legend under the results: the classes the view uses for this analysis, with what they mean. */
export function legendOf(a: PartAnalysis): { cls: number; label: string }[] {
  switch (a.kind) {
    case "skin":
      return [
        ...(a.result.up.analyzed ? [{ cls: GREEN_CLASS, label: "Upskins" }] : []),
        ...(a.result.down.analyzed ? [{ cls: RED_CLASS, label: "Downskins" }] : []),
        { cls: PART_CLASS, label: "The rest of the part" },
      ];
    case "support":
      // Rust marks a triangle that needs support with class 1; the view paints it red.
      return [
        { cls: RED_CLASS, label: "Needs support" },
        { cls: PART_CLASS, label: "The rest of the part" },
      ];
    case "removability":
      return [
        { cls: GREEN_CLASS, label: "Lifts out" },
        { cls: RED_CLASS, label: "Trapped" },
      ];
    case "wall": {
      const t = a.settings.threshold;
      if (a.settings.colors === "red-green")
        return [
          { cls: RED_CLASS, label: `Thinner than ${t} mm` },
          { cls: GREEN_CLASS, label: `${t} mm or thicker` },
        ];
      return [];
    }
    default:
      return [];
  }
}

/** The colour class the view paints a triangle of Rust class `c` with. */
export function paintClass(a: PartAnalysis, c: number): number {
  if (a.kind === "support") return c === 1 ? RED_CLASS : PART_CLASS;
  return c < ANALYSIS_COLORS.length ? c : PART_CLASS;
}

// ---------------------------------------------------------------------------------------------
// Wall thickness and mesh compare: colours from values, worked out per triangle or per point.

export type Rgb = [number, number, number];

const hex = (c: string): Rgb => [parseInt(c.slice(1, 3), 16) / 255, parseInt(c.slice(3, 5), 16) / 255, parseInt(c.slice(5, 7), 16) / 255];
const GREY = hex(ANALYSIS_COLORS[PART_CLASS]);
const GREEN = hex(ANALYSIS_COLORS[GREEN_CLASS]);
const RED = hex(ANALYSIS_COLORS[RED_CLASS]);
const BLUE = hex(ANALYSIS_COLORS[3]);
const YELLOW = hex("#f2c230");
const mix = (a: Rgb, b: Rgb, t: number): Rgb => [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
const clamp01 = (v: number) => Math.min(1, Math.max(0, v));

/**
 * The colour of a wall `thickness` (mm; NaN: not measured) against the threshold: red under it and green over it, or a
 * gradient from red (none) through yellow (the threshold) to green (twice it), or from blue to yellow.
 */
export function wallColor(thickness: number, s: Pick<WallSettings, "threshold" | "colors">): Rgb {
  if (Number.isNaN(thickness)) return GREY;
  if (s.colors === "red-green") return thickness < s.threshold ? RED : GREEN;
  const t = clamp01(thickness / (2 * s.threshold));
  if (s.colors === "blue-yellow-gradient") return mix(BLUE, YELLOW, t);
  return t < 0.5 ? mix(RED, YELLOW, t * 2) : mix(YELLOW, GREEN, (t - 0.5) * 2);
}

/** Wall thickness colours as bytes, r, g, b per triangle: what the coloured part keeps. */
export function wallTriangleColors(values: Float32Array, s: Pick<WallSettings, "threshold" | "colors">): Uint8Array {
  const out = new Uint8Array(values.length * 3);
  values.forEach((v, i) => {
    const [r, g, b] = wallColor(v, s);
    out[i * 3] = Math.round(r * 255);
    out[i * 3 + 1] = Math.round(g * 255);
    out[i * 3 + 2] = Math.round(b * 255);
  });
  return out;
}

/** Bars of the distance histogram. */
export const HISTOGRAM_BARS = 24;

export interface Histogram {
  lo: number;
  hi: number;
  counts: number[];
}

/** The histogram of every distance of the comparison's surfaces, from the lowest to the highest. */
export function histogram(surfaces: readonly CompareSurface[], bars = HISTOGRAM_BARS): Histogram {
  let lo = Infinity;
  let hi = -Infinity;
  for (const s of surfaces) for (const v of s.values) {
    if (v < lo) lo = v;
    if (v > hi) hi = v;
  }
  if (!Number.isFinite(lo)) return { lo: 0, hi: 0, counts: new Array<number>(bars).fill(0) };
  const counts = new Array<number>(bars).fill(0);
  for (const s of surfaces) for (const v of s.values) counts[barOf(v, lo, hi, bars)] += 1;
  return { lo, hi, counts };
}

/** The bar a distance falls in. */
export function barOf(v: number, lo: number, hi: number, bars = HISTOGRAM_BARS): number {
  if (hi <= lo) return 0;
  return Math.min(bars - 1, Math.max(0, Math.floor(((v - lo) / (hi - lo)) * bars)));
}

/**
 * The colour of a signed distance: green within ± tolerance, red above it and blue below; with the gradient, shading
 * from green at the tolerance to full red or blue at the largest distance. A distance outside the picked bars is grey.
 */
export function compareColor(d: number, view: Pick<CompareView, "tolerance" | "gradient" | "bars">, largest: number, h?: Histogram): Rgb {
  if (view.bars && h && !view.bars.includes(barOf(d, h.lo, h.hi))) return GREY;
  if (Math.abs(d) <= view.tolerance) return GREEN;
  const end = d > 0 ? RED : BLUE;
  if (!view.gradient) return end;
  const t = clamp01((Math.abs(d) - view.tolerance) / Math.max(largest - view.tolerance, 1e-12));
  return mix(GREEN, end, 0.35 + 0.65 * t);
}

/** The largest distance of a comparison either way, where its colour gradient ends. */
export function largestDistance(a: Extract<PartAnalysis, { kind: "compare" }>): number {
  return Math.max(a.result.forward?.maxUnsigned ?? 0, a.result.backward?.maxUnsigned ?? 0);
}

/** Which band a distance is in: -1 below the tolerance, 0 within, 1 above. */
export const band = (d: number, tolerance: number) => (d > tolerance ? 1 : d < -tolerance ? -1 : 0);
