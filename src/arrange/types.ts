// Arrange's data as Rust takes and sends it (src-tauri/src/jobs/arrange.rs, src-tauri/src/arrange/).

/** `Align` in src-tauri/src/arrange/align.rs. "front" is the platform's near edge, the smallest Y. */
export type AlignHow =
  | "left"
  | "right"
  | "front"
  | "back"
  | "bottom"
  | "top"
  | "centre-x"
  | "centre-y"
  | "centre-z"
  | "platform-centre"
  | "origin"
  | "drop";

/** One axis of a grid: places along it (the originals' included), the spacing, and which way the copies go. */
export interface GridAxis {
  count: number;
  gap: number;
  negative: boolean;
}

/** `Duplicate` in src-tauri/src/arrange/duplicate.rs. */
export type DuplicateHow =
  | { mode: "in-place"; copies: number }
  | { mode: "grid"; axes: [GridAxis, GridAxis, GridAxis]; spacing: "boxes" | "origins" }
  | { mode: "fill"; spacing: number; wall: number; limit: number; stack: boolean };

export type DuplicateMode = DuplicateHow["mode"];

export type BoxAxes = "z" | "all";

/** `Goal` in src-tauri/src/arrange/orient.rs. */
export type OrientGoal = "least-support" | "lowest-height" | "smallest-footprint";

/** `Scores` in src-tauri/src/arrange/orient.rs: mm², mm, mm². */
export interface OrientScores {
  supportArea: number;
  height: number;
  footprint: number;
}

export interface OrientCandidate extends OrientScores {
  matrix: number[];
}

/** `ArrangeProgress` in src-tauri/src/jobs/arrange.rs. */
export interface ArrangeProgress {
  job: number;
  fraction: number;
}
