// Part analyses (Analyze tab, Home › New analysis): what Rust answers (src-tauri/src/analysis) and what a part keeps.
import type { Vec3 } from "../store";

export type AnalysisKind = "standard" | "gravity" | "shadow" | "support" | "skin" | "wall" | "compare" | "removability";

/** The name of each kind, as the tree row, the panel and the undo step say it. */
export const KIND_LABELS: Record<AnalysisKind, string> = {
  standard: "Standard analysis",
  gravity: "Centre of gravity",
  shadow: "Shadow area",
  support: "Support volume",
  skin: "Up- and downskins",
  wall: "Wall thickness",
  compare: "Mesh compare",
  removability: "Z-removability",
};

/** Minimum, maximum, average and standard deviation of a list of values (Rust `Spread`). */
export interface Spread {
  min: number;
  max: number;
  average: number;
  deviation: number;
}

/** Rust `Standard`. World mm, mm², mm³. */
export interface StandardResult {
  min: Vec3;
  max: Vec3;
  size: Vec3;
  volume: number;
  /** False when the mesh has open edges: like the Information panel, no volume is shown then. */
  volumeValid: boolean;
  area: number;
  points: number;
  triangles: number;
  edges: number;
  shells: number;
  holes: number;
  boundaryEdges: number;
  boundaryLength: number;
  flippedTriangles: number;
  badEdges: number;
  closed: boolean;
  orientable: boolean;
  edgesPerPoint: Spread;
  trianglesPerEdge: Spread;
  triangleQuality: Spread;
  edgeLength: Spread;
}

/** Rust `Gravity`. World mm. */
export interface GravityResult {
  centre: Vec3;
  min: Vec3;
  max: Vec3;
  size: Vec3;
  volume: number;
}

export interface ShadowResult {
  /** mm². */
  area: number;
}

export interface SupportSettings {
  /** Degrees, 0 to 90: downskins tilted at most this much against the platform need support. */
  angle: number;
}

/** Rust `Support`. */
export interface SupportResult {
  angle: number;
  /** mm³. */
  volume: number;
  /** mm², seen from below. */
  area: number;
  triangles: number;
}

/** Rust `SkinSettings`. Degrees and mm². */
export interface SkinSettings {
  upskins: boolean;
  upAngle: number;
  downskins: boolean;
  downAngle: number;
  minComponent: number;
  filterSmall: boolean;
}

/** Rust `SkinTotals`. */
export interface SkinTotals {
  analyzed: boolean;
  angle: number;
  area: number;
  components: number;
}

export interface SkinResult {
  up: SkinTotals;
  down: SkinTotals;
}

/** How Wall thickness colours the part. */
export type WallColors = "red-green" | "red-green-gradient" | "blue-yellow-gradient";

/** Rust `WallSettings` plus the window's own: the colours and the coloured part made when the analysis closes. */
export interface WallSettings {
  /** mm. */
  threshold: number;
  /** Percent of the surface that may be thinner before the test fails. */
  acceptance: number;
  stopEarly: boolean;
  /** Stop at the first wall thinner than the threshold. */
  simple: boolean;
  colors: WallColors;
  colouredPart: boolean;
}

/** Rust `WallResult`. */
export interface WallResult {
  threshold: number;
  acceptance: number;
  passed: boolean;
  /** False when the simple mode stopped at a thin wall before the test could tell (projects saved before it: absent). */
  decided?: boolean;
  /** The simple mode stopped at the first thin wall. */
  stoppedAtThin?: boolean;
  thinPercent: number;
  thinArea: number;
  totalArea: number;
  minThickness: number | null;
  minFrom: Vec3 | null;
  minTo: Vec3 | null;
  complete: boolean;
  unmeasured: number;
}

export type CompareDirection = "both" | "reference-to-comparison" | "comparison-to-reference";

/** Rust `CompareSettings` plus the comparison part, which the analysis is kept with the reference to name. */
export interface CompareSettings {
  comparisonId: string;
  comparisonName: string;
  /** The comparison part's matrix when it ran: when it differs now, the results are out of date. */
  comparisonMatrix: number[];
  direction: CompareDirection;
  align: boolean;
}

/** Rust `SideResult`: one direction's distances, mm. */
export interface SideResult {
  maxPositive: number;
  maxNegative: number;
  averageSigned: number;
  maxUnsigned: number;
  averageUnsigned: number;
  points: number;
}

/** Rust `Fit`: how Align turned (rotation by rows) and moved the comparison copy. */
export interface CompareFit {
  rotation: [Vec3, Vec3, Vec3];
  translation: Vec3;
  angle: number;
  rms: number;
}

export interface CompareResult {
  /** The reference's points against the comparison's surface, and the other way. */
  forward: SideResult | null;
  backward: SideResult | null;
  fit: CompareFit | null;
}

/** Rust `RemovabilitySettings`. */
export interface RemovabilitySettings {
  /** mm: a body lifted out must keep at least this far from the other bodies; 0: it only must not hit them. */
  minDistance: number;
}

export const DEFAULT_REMOVABILITY: RemovabilitySettings = { minDistance: 0 };

/** Rust `Removability`: the bodies of one part lifted straight up, each alone. */
export interface RemovabilityResult {
  /** mm, the minimum distance it ran with (projects saved before it: absent, which was 0). */
  minDistance?: number;
  removable: boolean;
  bodies: number;
  trappedBodies: number;
  /** mm². */
  trappedArea: number;
}

/** Rust `Interlock`: the parts (indices of those sent) in the order they lift out, round by round, and the rest. */
export interface InterlockResult {
  rounds: number[][];
  trapped: number[];
}

/** A point clicked on a compared surface, with the distance there. */
export interface Probe {
  id: string;
  point: Vec3;
  value: number;
}

/** A refined world copy of a compared part, with a signed distance per vertex. */
export interface CompareSurface {
  role: "reference" | "comparison";
  positions: Float32Array;
  indices: Uint32Array;
  values: Float32Array;
}

/** What the view draws for an analysis besides the part. */
export interface AnalysisDrawing {
  /** A colour class per triangle of the part (see src/analysis/colors.ts). */
  classes?: Uint8Array;
  /** A value per triangle of the part (wall thickness, mm; NaN where none). */
  values?: Float32Array;
  /** Mesh compare's coloured copies. */
  surfaces?: CompareSurface[];
  /** World triangles, x, y, z per corner. */
  triangles?: Float32Array;
  /** World line segments, x, y, z per end. */
  lines?: Float32Array;
}

interface Base {
  id: string;
  /** The part's matrix when the analysis ran: when it differs now, the results are out of date. */
  matrix: number[];
  drawing: AnalysisDrawing;
}

/** An analysis a part keeps: shown in the tree under it, saved in the project. */
export type PartAnalysis =
  | (Base & { kind: "standard"; settings: null; result: StandardResult })
  | (Base & { kind: "gravity"; settings: null; result: GravityResult })
  | (Base & { kind: "shadow"; settings: null; result: ShadowResult })
  | (Base & { kind: "support"; settings: SupportSettings; result: SupportResult })
  | (Base & { kind: "skin"; settings: SkinSettings; result: SkinResult })
  | (Base & { kind: "wall"; settings: WallSettings; result: WallResult })
  | (Base & { kind: "compare"; settings: CompareSettings; result: CompareResult; probes?: Probe[] })
  | (Base & { kind: "removability"; settings: RemovabilitySettings | null; result: RemovabilityResult });

export type AnalysisSettings = SupportSettings | SkinSettings | WallSettings | CompareSettings | RemovabilitySettings | null;

/** `AnalysisProgress` in src-tauri/src/jobs/analysis.rs. */
export interface AnalysisProgress {
  job: number;
  fraction: number;
}

export const DEFAULT_SUPPORT: SupportSettings = { angle: 40 };

export const DEFAULT_SKIN: SkinSettings = {
  upskins: true,
  upAngle: 45,
  downskins: true,
  downAngle: 45,
  minComponent: 10,
  filterSmall: false,
};

/** Highest threshold the up- and downskin angles take (src-tauri/src/analysis/skin.rs MAX_ANGLE). */
export const MAX_SKIN_ANGLE = 89.9;

export const DEFAULT_WALL: WallSettings = { threshold: 0.4, acceptance: 5, stopEarly: false, simple: false, colors: "red-green", colouredPart: false };

/** How the view shows a comparison: the panel's controls, kept while the analysis is on show. */
export interface CompareView {
  /** mm: distances within ± this are green. */
  tolerance: number;
  gradient: boolean;
  triangleEdges: boolean;
  toleranceEdges: boolean;
  transparent: boolean;
  /** The histogram bars picked (indices), or null for all: points outside them are drawn grey. */
  bars: number[] | null;
  tab: "distances" | "chart";
  /** What a click on the compared part does. */
  probe: "select" | "add" | "remove";
}
