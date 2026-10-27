// Repair mode's shared types: the IPC shapes of src-tauri/src/repair (catalog, scripts, status,
// reports) and the session the repair store keeps. Field names match the Rust serde output.
import type { MeshStats } from "../io/ipc";
import type { MeshData } from "../store";

/** Stable action ids, the serde names of `ActionId` in src-tauri/src/repair/mod.rs. */
export type ActionId =
  | "sanitize_duplicate_nodes"
  | "stitch_triangles"
  | "close_trivial_holes"
  | "close_all_holes"
  | "close_defined_holes"
  | "remove_double_triangles"
  | "fix_flipped_triangles"
  | "invert_negative_shells"
  | "remove_degenerate_faces"
  | "remove_tiny_shells"
  | "split_non_oriented_edges"
  | "split_non_manifold_edges"
  | "detect_self_intersections"
  | "split_off_self_intersections"
  | "remove_self_intersections"
  | "remove_problem_areas"
  | "wrap_part_surface"
  | "remove_selected_triangles"
  | "flip_selected_triangles"
  | "smooth_triangles"
  // Home › Repair only.
  | "close_holes_curved"
  | "bridge_gap"
  | "grow_hole_band"
  | "subdivide_triangles"
  | "add_triangle"
  | "add_point"
  | "move_node"
  | "cut_surface"
  | "select_problem_triangles"
  | "find_overlaps"
  // Catalog only (Soon).
  | "prefix_suffix"
  | "reduce_triangles"
  | "refine_triangles"
  | "remesh"
  | "scale"
  | "z_compensation";

export type ParamValue = number | boolean | string;

export interface ParamChoice {
  id: string;
  label: string;
}

/** `ParamSpec` in src-tauri/src/repair/defaults.rs. */
export interface ParamSpec {
  /** The key in a step's `params`. */
  id: string;
  label: string;
  kind: "float" | "bool" | "choice";
  /** "mm", "mm³" or "mm²" for numbers. */
  unit?: string;
  /** Decimals the editor shows: 0.01 with 3 reads "0.010". */
  decimals?: number;
  min?: number;
  default: ParamValue;
  choices?: ParamChoice[];
}

/** `ActionSpec` in defaults.rs. */
export interface ActionSpec {
  id: ActionId;
  label: string;
  /** False: listed with a Soon badge, cannot be added or run. */
  implemented: boolean;
  /** Listed in the Repair scripts tab's add-action dropdown. */
  inScripts: boolean;
  /** Needs a triangle selection. */
  needsSelection: boolean;
  /** Type number in the reference tool's .repair files. */
  repairType?: number;
  hint: string;
  params: ParamSpec[];
}

/** One action of a script with its own parameter values; missing params take the catalog default. */
export interface ScriptStep {
  type: ActionId;
  enabled: boolean;
  params: Record<string, ParamValue>;
}

/** `Script` in src-tauri/src/repair/scripts.rs. Built-in ids start with "builtin.". */
export interface RepairScript {
  id: string;
  name: string;
  builtIn: boolean;
  actions: ScriptStep[];
}

export interface Catalog {
  actions: ActionSpec[];
  /** Built-in scripts, in picker order. */
  scripts: RepairScript[];
}

/** What the config folder's repair-scripts.json holds: the user's scripts and the picker order of all scripts. */
export interface StoredScripts {
  scripts: RepairScript[];
  /** Script ids (built-in and own) in picker order; ids not listed go at the end. */
  order: string[];
}

/** `RepairStatus` in src-tauri/src/repair/status.rs. */
export interface RepairStatus {
  edges: number;
  triangles: number;
  vertices: number;
  shells: number;
  borderEdges: number;
  nonManifoldEdges: number;
  invertedTriangles: number;
  holes: number;
  degenerateTriangles: number;
  closed: boolean;
  oriented: boolean;
  /** mm³, signed. */
  volume: number;
  /** mm². */
  area: number;
}

/** What the viewport highlights for the current working copy; arrives with a status. */
export interface Highlights {
  /** Border edge segments (x, y, z, x, y, z per edge): holes, thick yellow. */
  border: Float32Array;
  /** Non-manifold edge segments: errors, orange. */
  nonManifold: Float32Array;
  /** Inverted triangles, red. */
  inverted: Uint32Array;
  /** Degenerate triangles, magenta. */
  degenerate: Uint32Array;
}

/** `Report` in src-tauri/src/repair/mod.rs: one action of a run. */
export interface ActionReport {
  action: ActionId;
  label: string;
  message: string;
  /** What is still wrong once the whole run is done. */
  warnings: string[];
  /** Warnings a later action of the run put right, saying which one: "1 hole could not be closed. Closed by …". */
  resolved: string[];
  changed: boolean;
  millis: number;
}

/** `ShellRow` in src-tauri/src/repair/shells.rs (mm, mm², mm³). */
export interface ShellRow {
  triangles: number;
  area: number;
  bboxVolume: number;
  /** Null for an open shell. */
  volume: number | null;
  watertight: boolean;
}

/** The damage badge's check of one part mesh (`RepairCheck` in jobs.rs). */
export interface RepairCheck {
  closed: boolean;
  oriented: boolean;
  holes: number;
  borderEdges: number;
  nonManifoldEdges: number;
  invertedTriangles: number;
}

/** `repair-progress` event payload. */
export interface RepairProgress {
  job: number;
  /** 0-based action index and the action count. */
  step: number;
  steps: number;
  label: string;
  /** The step the running action is on ("Finding intersections"); null for actions that name none. */
  detail?: string | null;
  /** 0..1 over the running action alone. */
  local?: number;
  /** 0..1 over the whole run. */
  fraction: number;
}

/** A finished run (repair_run). */
export interface RunResult {
  changed: boolean;
  /** The working copy after the run; the input mesh object itself when nothing changed. */
  mesh: MeshData;
  /** Its stats and problem edges, for meshCache, when it changed. */
  stats: MeshStats | null;
  edges: Float32Array | null;
  /** The selection carried through the run (sorted triangle ids). */
  selection: Uint32Array;
  /** True when an action set the selection (Select by criteria, or an edit that leaves none), so it applies even
   *  when the mesh did not change. */
  selected: boolean;
  reports: ActionReport[];
  /** Present when the run was asked for a status. */
  status: RepairStatus | null;
  highlights: Highlights | null;
  /** Triangles to highlight as self-intersecting (Detect), when the last such action left some. */
  intersecting: Uint32Array | null;
  warnings: string[];
}

// ---------------------------------------------------------------------------------------------
// Session (src/repair/store.ts)

export type SelectTool = "triangle" | "surface" | "shell" | "area" | "brush";

/** The edit tools that take over the viewport's clicks from the select tools: Move nodes and Cut surfaces. */
export type EditTool = "move-nodes" | "cut";

/** One state of the working copy in the inner undo history. */
export interface RepairSnapshot {
  label: string;
  mesh: MeshData;
  /** Per-triangle selection mask (1 = selected). */
  selection: Uint8Array;
}

export interface RepairSession {
  /** The part being repaired. */
  partId: string;
  partName: string;
  /** The working copy as it was on entering (world space), to tell whether anything changed. */
  initialMesh: MeshData;
  mesh: MeshData;
  /** Per-triangle mask, length = triangle count of `mesh`. */
  selection: Uint8Array;
  selectedCount: number;
  /** Oldest first; each entry is the state before a step. */
  past: RepairSnapshot[];
  /** Next redo first; each entry is the state after a step. */
  future: RepairSnapshot[];
  /** Null: not computed for the current mesh ("?" in the Status tab). */
  status: RepairStatus | null;
  highlights: Highlights | null;
  /** From Detect self-intersections; cleared when the mesh changes. */
  intersecting: Uint32Array | null;
  /** The last run's reports, for the Repair scripts tab's "Last run" section. */
  lastRun: ActionReport[] | null;
  /** Triangles left out of the surface and out of picking. Null means every triangle is shown. */
  hiddenTriangles: Uint8Array | null;
}

/** A run in progress: drives the progress dialog's bar and Cancel. */
export interface RepairBusy {
  job: number;
  label: string;
  /** 0..1, or null while unknown. */
  progress: number | null;
  step: string | null;
  /** The step inside the running action, for actions that name theirs, and how far the action is (0..1). */
  detail: string | null;
  local: number | null;
  cancelling: boolean;
}

/** Status tab highlighting switches. */
export interface HighlightSettings {
  holes: boolean;
  triangles: boolean;
  /** Edges whose dihedral angle exceeds this many degrees; null = off (the slider's far end). */
  edgesFrom: number | null;
  degenerate: boolean;
  errors: boolean;
}

/** View tab settings. */
export interface ViewSettings {
  slimColored: boolean;
  slimEdges: boolean;
  /** Degrees: triangles whose smallest angle is below this are slim. */
  slimAngle: number;
  /** Surface selection tolerance in degrees (0–180). */
  surfaceTolerance: number;
}

/** Apply repair's three choices. */
export type ApplyMode = "remove" | "keep" | "compare";
