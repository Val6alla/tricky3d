// Measurements as a project file holds them: JSON in each part's metadata (src-tauri/src/project.rs). Reading checks
// every field, since the file may come from anywhere; whatever does not fit is left out.
import type { Vec3 } from "../store";
import { toolById } from "./tools";
import type { MeasureFeature, Measurement } from "./types";

const isVec3 = (v: unknown): v is Vec3 => Array.isArray(v) && v.length === 3 && v.every((x) => typeof x === "number" && Number.isFinite(x));
const isLength = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v) && v >= 0;

function feature(raw: unknown): MeasureFeature | null {
  if (typeof raw !== "object" || raw === null) return null;
  const f = raw as Record<string, unknown>;
  switch (f.kind) {
    case "point":
      return isVec3(f.p) ? { kind: "point", p: f.p } : null;
    case "edge":
      return isVec3(f.a) && isVec3(f.b) ? { kind: "edge", a: f.a, b: f.b } : null;
    case "circle":
      return isVec3(f.centre) && isVec3(f.normal) && isLength(f.radius) ? { kind: "circle", centre: f.centre, normal: f.normal, radius: f.radius } : null;
    case "sphere":
      return isVec3(f.centre) && isLength(f.radius) ? { kind: "sphere", centre: f.centre, radius: f.radius } : null;
    case "surface":
      return isVec3(f.point) && isVec3(f.normal) ? { kind: "surface", point: f.point, normal: f.normal } : null;
    case "wall":
      return isVec3(f.from) && isVec3(f.to) ? { kind: "wall", from: f.from, to: f.to } : null;
    default:
      return null;
  }
}

/** The measurement a saved entry describes, or null when it is not one. */
export function measurementOf(raw: unknown): Measurement | null {
  if (typeof raw !== "object" || raw === null) return null;
  const m = raw as Record<string, unknown>;
  const tool = typeof m.tool === "string" ? toolById(m.tool) : undefined;
  if (typeof m.id !== "string" || !tool || !Array.isArray(m.features)) return null;
  const features = m.features.map(feature);
  if (features.length !== tool.needs.length || features.some((f) => f === null)) return null;
  const axes: [boolean, boolean, boolean] =
    Array.isArray(m.axes) && m.axes.length === 3 && m.axes.every((a) => typeof a === "boolean") ? [m.axes[0], m.axes[1], m.axes[2]] : [false, false, false];
  return { id: m.id, tool: tool.id, features: features as MeasureFeature[], axes, diameter: m.diameter === true };
}

/** The measurements in a part's saved JSON; undefined when there are none (or none that fit). */
export function readMeasurements(json: string | undefined): Measurement[] | undefined {
  if (!json) return undefined;
  let raw: unknown;
  try {
    raw = JSON.parse(json);
  } catch {
    return undefined;
  }
  if (!Array.isArray(raw)) return undefined;
  const list = raw.map(measurementOf).filter((m): m is Measurement => m !== null);
  return list.length > 0 ? list : undefined;
}
