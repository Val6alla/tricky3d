// Generate shell's data as Rust sends and takes it (src-tauri/src/ops/shell.rs, src-tauri/src/jobs.rs shell_generate).

/** The five kinds of shell, in the dialog's order. */
export type ShellKind = "hollow-part" | "inner-offset" | "outer-offset" | "hollowing-shell" | "hollow-part-improved";

/** `ShellSettings` in src-tauri/src/ops/shell.rs: the dialog's fields, mm. */
export interface ShellSettings {
  kind: ShellKind;
  thickness: number;
  /** The grid step: smaller is finer and slower. */
  accuracy: number;
  smooth: boolean;
}

/** `ShellProgress` in src-tauri/src/jobs.rs. */
export interface ShellProgress {
  job: number;
  /** 0-based index of the part being worked on, and how many there are. */
  part: number;
  parts: number;
  stage: string | null;
  /** 0..1 over the whole run. */
  fraction: number;
}

/** `Estimate` in src-tauri/src/ops/shell.rs. */
export interface ShellEstimate {
  points: number;
  measured: number;
  megabytes: number;
  seconds: number;
  /** Longer than a minute. */
  slow: boolean;
  /** More than 4 GB. */
  heavy: boolean;
  /** Why it would not run at all, or null. */
  refused: string | null;
}

/** One part for the estimate: its world box size, surface area and triangle count. */
export interface EstimatePart {
  size: [number, number, number];
  area: number;
  triangles: number;
}

/** A part Generate shell would not run on: it is not closed or not oriented. */
export interface ShellRefusal {
  source: number;
  closed: boolean;
  oriented: boolean;
  holes: number;
  borderEdges: number;
  nonManifoldEdges: number;
  invertedTriangles: number;
}
