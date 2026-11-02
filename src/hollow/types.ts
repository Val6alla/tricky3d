// Hollow's data as Rust sends and takes it (src-tauri/src/ops/hollow.rs, src-tauri/src/jobs/hollow.rs).
import type { Vec3 } from "../store";

/** `HollowSettings` in src-tauri/src/ops/hollow.rs, mm. */
export interface HollowSettings {
  /** Wall thickness. */
  thickness: number;
  /** The grid step: smaller is finer and slower. */
  accuracy: number;
  /** 0 (off) to 3 (high): how much the cavity's surface is smoothed. */
  smoothness: number;
}

/** `Hole` in src-tauri/src/ops/hollow.rs: where a drain hole goes and its shape (mm, degrees). */
export interface Hole {
  /** The point on the outer surface it starts at (world). */
  at: Vec3;
  /** The outer surface's outward normal there; the hole runs the other way, through the wall. */
  normal: Vec3;
  /** Diameter where it meets the cavity. */
  diameter: number;
  /** Lean of its side from its axis, wider towards the outside. */
  taper: number;
  /** Radius of the rounding of its rim on the outer surface. */
  flare: number;
}

/** The shape new holes get: Hole without its place. */
export type HoleShape = Pick<Hole, "diameter" | "taper" | "flare">;

/** `HoleCheck` in src-tauri/src/ops/hollow.rs. */
export interface HoleCheck {
  ok: boolean;
  /** Why it would not be cut. */
  reason: string | null;
  /** How far it runs through the wall, mm. */
  depth: number | null;
  room: number | null;
}

/** Walls too thin to hollow: their share of the surface, a box round them, and Generate shell's sentence. */
export interface ThinWalls {
  share: number;
  bounds: [Vec3, Vec3];
  centre: Vec3;
  message: string;
}

/** `HollowProgress` in src-tauri/src/jobs/hollow.rs. */
export interface HollowProgress {
  job: number;
  stage: string | null;
  fraction: number;
}

/** A part Hollow would not run on: not closed or not oriented. */
export interface HollowRefusal {
  closed: boolean;
  oriented: boolean;
  holes: number;
  invertedTriangles: number;
}
