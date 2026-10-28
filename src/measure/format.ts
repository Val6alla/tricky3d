// A measurement's words and numbers, as the view's labels and the panel's list show them. Pure.
import { formatFixed2, formatFixed3 } from "../info";
import type { Measurement, MeasureResult } from "./types";

const AXES = ["X", "Y", "Z"] as const;

/** "12.345 mm" with "X 1.000 Y 2.000" added for the axes asked for; "90.00°"; "R 10.000 mm" or "Ø 20.000 mm". */
export function valueText(m: Pick<Measurement, "axes" | "diameter">, r: MeasureResult): string {
  switch (r.kind) {
    case "distance":
    case "thickness": {
      const main = `${formatFixed3(r.value)} mm`;
      const parts = AXES.flatMap((a, k) => (m.axes[k] && r.components ? [`${a} ${formatFixed3(r.components[k])}`] : []));
      return parts.length ? `${main}  (${parts.join("  ")})` : main;
    }
    case "angle":
      return `${formatFixed2(r.value)}°`;
    case "radius":
      return m.diameter ? `Ø ${formatFixed3(2 * r.value)} mm` : `R ${formatFixed3(r.value)} mm`;
    case "position":
      return r.components ? AXES.map((a, k) => `${a} ${formatFixed3(r.components![k])}`).join("  ") : "";
  }
}

/** The lines the panel adds under a measurement's value: each axis of a distance, a surface's normal, the note. */
export function detailLines(r: MeasureResult): string[] {
  const out: string[] = [];
  if ((r.kind === "distance" || r.kind === "thickness") && r.components) {
    out.push(AXES.map((a, k) => `Δ${a} ${formatFixed3(r.components![k])} mm`).join(", "));
  }
  if (r.kind === "radius") out.push(`Diameter ${formatFixed3(2 * r.value)} mm`);
  if (r.normal) out.push(`Normal ${r.normal.map((v) => formatFixed3(v)).join(", ")}`);
  if (r.apex) out.push(`Apex ${r.apex.map((v) => formatFixed3(v)).join(", ")} mm`);
  if (r.note) out.push(r.note);
  return out;
}
