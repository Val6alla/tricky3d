// Measure mode's state: the open session (the tool, the clicks of the measurement in progress, the measurement picked
// in the list, the panel tab), the options, and the results Rust worked out for the parts' measurements.
// src/measure/session.ts is the only code that changes the session and the results; the options have a setter for the
// panel. The measurements themselves live with their parts in the main store (Part.measurements).
import { create } from "zustand";
import { loadSetting } from "../store";
import type { MeasureFeature, MeasureResult, SnapMode } from "./types";
import { DEFAULT_TOOL, toolById, type ToolGroup } from "./tools";

/** A feature of the measurement in progress: in its owner's frame, and in world space for drawing. */
export interface PendingFeature {
  feature: MeasureFeature;
  world: MeasureFeature;
  /** A surface's outline, world space, x, y, z, x, y, z per segment. */
  outline: number[];
}

export type MeasurePanelTab = "measurements" | "tools" | "options";

export interface MeasureSession {
  tool: string;
  /** The features picked so far for the measurement in progress. */
  picks: PendingFeature[];
  /** Point clicks gathered towards a 3-point circle or a 4-point sphere. */
  points: PendingFeature[];
  /** The part the measurement in progress belongs to: the first one clicked. */
  owner: string | null;
  /** The measurement picked in the list (highlighted), by id. */
  selected: string | null;
  panelTab: MeasurePanelTab;
}

export interface MeasureOptions {
  snap: SnapMode;
  /** X, Y and Z of a distance on the label of the next measurement. */
  axes: [boolean, boolean, boolean];
  diameter: boolean;
  /** Percent. */
  markerSize: number;
  /** Arrowheads keep their size on screen as the view zooms; off, they keep their size on the part. */
  scaleArrows: boolean;
  /** Percent. */
  arrowSize: number;
}

export const DEFAULT_OPTIONS: MeasureOptions = {
  snap: "auto",
  axes: [false, false, false],
  diameter: false,
  markerSize: 100,
  scaleArrows: true,
  arrowSize: 100,
};

/** A measurement's result under the matrix it was worked out for, or why it could not be. */
export interface ResultEntry {
  matrix: readonly number[];
  result: MeasureResult | null;
  error: string | null;
}

export interface MeasureState {
  session: MeasureSession | null;
  options: MeasureOptions;
  /** By measurement id. */
  results: Record<string, ResultEntry>;
  /** A click is being worked out in Rust. */
  busy: boolean;
  /** The tool used last, which Measure opens on. Kept when the mode closes, and remembered for the next app start. */
  lastTool: string;
  /** The tool last picked from each group, which its button in the Tools tab picks again. Kept when the mode closes. */
  lastTools: Partial<Record<ToolGroup, string>>;

  setOptions(patch: Partial<MeasureOptions>): void;
}

export const emptySession = (tool = DEFAULT_TOOL): MeasureSession => ({ tool, picks: [], points: [], owner: null, selected: null, panelTab: "measurements" });

/** Where the tool used last is remembered between app starts. */
export const LAST_TOOL_KEY = "tricky3d.measureTool";

/** The tool remembered from the last run, or Distance point to point when none is (or it is no tool any more). */
function savedTool(): string {
  const id = loadSetting(LAST_TOOL_KEY);
  return id !== null && toolById(id) ? id : DEFAULT_TOOL;
}

export const useMeasure = create<MeasureState>()((set) => ({
  session: null,
  options: DEFAULT_OPTIONS,
  results: {},
  busy: false,
  lastTool: savedTool(),
  lastTools: {},
  setOptions(patch) {
    set((s) => ({ options: { ...s.options, ...patch } }));
  },
}));
