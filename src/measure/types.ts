// Measure mode's data. Features and results come from Rust (src-tauri/src/measure.rs), which does every bit of
// geometry; the field names match its serde names.
import type { Vec3 } from "../store";

/** Something measured on a part, in the part-local frame of the part the measurement belongs to. */
export type MeasureFeature =
  | { kind: "point"; p: Vec3 }
  | { kind: "edge"; a: Vec3; b: Vec3 }
  | { kind: "circle"; centre: Vec3; normal: Vec3; radius: number }
  | { kind: "sphere"; centre: Vec3; radius: number }
  | { kind: "surface"; point: Vec3; normal: Vec3 }
  | { kind: "wall"; from: Vec3; to: Vec3 };

/** What one click picks. */
export type PickKind = "point" | "edge" | "circle" | "surface" | "wall";

/** What a tool needs for each of its features: a click, or a circle or sphere fitted to 3 or 4 point clicks. */
export type Need = PickKind | "circle3" | "sphere4";

/** What Rust works out (src-tauri/src/measure.rs Op). */
export type MeasureOp =
  | "distance"
  | "centre-distance"
  | "angle"
  | "angle-to-yz"
  | "angle-to-xz"
  | "angle-to-xy"
  | "radius"
  | "position"
  | "thickness"
  | "angle3";

/** How a clicked point snaps (src-tauri/src/measure.rs Snap). */
export type SnapMode = "auto" | "free" | "edge" | "node" | "clip-line" | "clip-corner";

/** A measurement on a part: kept with the part (undoable, saved in the project), moving with it. */
export interface Measurement {
  id: string;
  /** Its tool's id (src/measure/tools.ts). */
  tool: string;
  features: MeasureFeature[];
  /** Which of X, Y and Z the label adds to a distance, as Options said when it was made. */
  axes: [boolean, boolean, boolean];
  /** A radius shows as a diameter. */
  diameter: boolean;
}

/** A measurement worked out in world space (src-tauri/src/measure.rs Evaluation). */
export interface MeasureResult {
  kind: "distance" | "angle" | "radius" | "position" | "thickness";
  /** mm, or degrees for an angle. */
  value: number;
  /** A distance along X, Y and Z, or a position's coordinates. */
  components: Vec3 | null;
  /** The dimension line, from and to. */
  line: [Vec3, Vec3] | null;
  apex: Vec3 | null;
  arms: [Vec3, Vec3] | null;
  normal: Vec3 | null;
  /** Where the label goes. */
  anchor: Vec3;
  /** The features in world space, for drawing. */
  features: MeasureFeature[];
  note: string | null;
}
