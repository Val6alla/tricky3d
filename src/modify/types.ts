// Modify › Manipulate mesh and the mesh tools next to it: the settings each tool sends to Rust
// (src-tauri/src/modify) and what comes back. Lengths in mm, angles in degrees.

/** The Manipulate mesh panel's tabs, in the panel's order. */
export type ModifyTab = "reduce" | "smooth" | "refine" | "remesh" | "zcomp" | "extrude" | "offset" | "holes";

export const TAB_LABELS: Record<ModifyTab, string> = {
  reduce: "Triangle reduction",
  smooth: "Smoothing",
  refine: "Refine",
  remesh: "Remesh",
  zcomp: "Z-compensation",
  extrude: "Extrude",
  offset: "Offset",
  holes: "Hole edges",
};

export const TABS: ModifyTab[] = ["reduce", "smooth", "refine", "remesh", "zcomp", "extrude", "offset", "holes"];

/** The tabs whose tool works on selected triangles only (in repair mode). */
export const NEEDS_SELECTION: ReadonlySet<ModifyTab> = new Set(["extrude", "offset"]);

/** Rust `remesh::Method`; Quad is listed as Soon. */
export type RemeshMethod = "volumetric" | "uniform" | "adaptive" | "manifold";

export const REMESH_METHODS: { value: RemeshMethod | "quad"; label: string; soon?: boolean }[] = [
  { value: "volumetric", label: "Volumetric" },
  { value: "uniform", label: "Uniform" },
  { value: "adaptive", label: "Adaptive" },
  { value: "quad", label: "Quad", soon: true },
  { value: "manifold", label: "Manifold" },
];

/** Rust `ReduceSettings`. */
export interface ReduceSettings {
  /** Triangles to end with. */
  target: number;
  /** Set target as low as possible. */
  lowest: boolean;
  /** Max. deformation. */
  maxDeviation: number;
  /** Max. edge length. */
  maxEdge: number;
  selectedOnly: boolean;
  fast: boolean;
}

/** Rust `SmoothSettings`. */
export interface SmoothSettings {
  /** 0 to 1. */
  strength: number;
  iterations: number;
  keepEdges: boolean;
  edgeAngle: number;
  selectedOnly: boolean;
  keepVolume: boolean;
}

/** Rust `RefineSettings`. */
export interface RefineSettings {
  maxEdge: number;
  selectedOnly: boolean;
}

/** Rust `RemeshSettings`, plus Show slim faces, which only colours the view. */
export interface RemeshSettings {
  method: RemeshMethod;
  /** Target edge length; Adaptive's small edge length. */
  edge: number;
  /** Adaptive's large edge length. */
  largeEdge: number;
  smoothing: number;
  keepEdges: boolean;
  edgeAngle: number;
  fast: boolean;
  selectedOnly: boolean;
  /** Show slim faces: colour triangles with an angle under `slimAngle`. */
  slimColored: boolean;
  slimAngle: number;
}

/** Rust `ZCompSettings`. */
export interface ZCompSettings {
  distance: number;
  angle: number;
}

export type ExtrudeDirection = "normal" | "x" | "y" | "z";

/** Rust `ExtrudeSettings`. */
export interface ExtrudeSettings {
  distance: number;
  direction: ExtrudeDirection;
  walls: boolean;
}

/** Rust `OffsetSettings`. */
export interface OffsetSettings {
  distance: number;
  transition: number;
}

/** Rust `HoleOffsetSettings`. */
export interface HoleOffsetSettings {
  distance: number;
  selectedOnly: boolean;
}

export interface TabSettings {
  reduce: ReduceSettings;
  smooth: SmoothSettings;
  refine: RefineSettings;
  remesh: RemeshSettings;
  zcomp: ZCompSettings;
  extrude: ExtrudeSettings;
  offset: OffsetSettings;
  holes: HoleOffsetSettings;
}

/** What `modify_run` gets: Rust `Tool` (tagged by `tool`). */
export type ToolSettings =
  | ({ tool: "reduce" } & ReduceSettings)
  | ({ tool: "smooth" } & SmoothSettings)
  | ({ tool: "refine" } & RefineSettings)
  | ({ tool: "remesh" } & Omit<RemeshSettings, "slimColored" | "slimAngle">)
  | ({ tool: "z-compensation" } & ZCompSettings)
  | ({ tool: "extrude" } & ExtrudeSettings)
  | ({ tool: "offset-selection" } & OffsetSettings)
  | ({ tool: "offset-holes" } & HoleOffsetSettings)
  | { tool: "invert" }
  | { tool: "invert-negative" };

/** The tools' names, as the busy box, the toasts and the undo step say them (Rust `Tool::label`). */
export const TOOL_LABELS: Record<ToolSettings["tool"], string> = {
  reduce: "Triangle reduction",
  smooth: "Smoothing",
  refine: "Refine",
  remesh: "Remesh",
  "z-compensation": "Z-compensation",
  extrude: "Extrude",
  "offset-selection": "Offset selection",
  "offset-holes": "Offset hole edges",
  invert: "Invert",
  "invert-negative": "Invert negative shells",
};

/**
 * The defaults the panel opens with. Target and Max. edge length depend on the part, so they are filled in when the
 * panel opens (half the triangles, the part's box diagonal).
 */
export const DEFAULT_SETTINGS: TabSettings = {
  reduce: { target: 0, lowest: false, maxDeviation: 0.05, maxEdge: 10, selectedOnly: false, fast: false },
  smooth: { strength: 0.5, iterations: 10, keepEdges: true, edgeAngle: 40, selectedOnly: false, keepVolume: true },
  refine: { maxEdge: 1, selectedOnly: false },
  remesh: {
    method: "uniform",
    edge: 1,
    largeEdge: 4,
    smoothing: 0,
    keepEdges: true,
    edgeAngle: 40,
    fast: false,
    selectedOnly: false,
    slimColored: false,
    slimAngle: 10,
  },
  zcomp: { distance: 0.1, angle: 30 },
  extrude: { distance: 1, direction: "normal", walls: true },
  offset: { distance: 0.5, transition: 0 },
  holes: { distance: 1, selectedOnly: false },
};

/** `ModifyProgress` in src-tauri/src/jobs/modify.rs. */
export interface ModifyProgress {
  job: number;
  stage: string | null;
  fraction: number;
}
