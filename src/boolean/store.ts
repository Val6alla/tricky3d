// Boolean operations mode's state: the session (the green and red lists, the log, the result waiting for OK), the
// run in progress, the viewport tool and the settings. src/boolean/session.ts is the only code that changes `session`
// and `busy`; the tool and the settings have plain setters for the ribbon and the panel.
import { create } from "zustand";
import type { AnalyzedMesh } from "../io/ipc";
import type { BooleanOp } from "../ops/manifoldOps";
import type { MeshData, Part } from "../store";

/** "main": the green list, the parts that are kept. "tool": the red list, the parts to subtract or intersect with. */
export type BooleanList = "main" | "tool";

/** What a click or drag in the viewport does in Boolean operations. */
export type BooleanTool = "select" | "zoom-area";

export type LogKind = "info" | "ok" | "warn" | "error";

export interface LogLine {
  id: number;
  kind: LogKind;
  text: string;
  /** A part this line offers "Repair first" for (Extended repair), by id. */
  repair?: string;
}

export interface BooleanSettings {
  removeOriginals: boolean;
  removeDegenerate: boolean;
  /** mm. */
  degenerateTolerance: number;
  /** Drop result shells smaller than noiseVolume. */
  filterNoise: boolean;
  /** cm³. */
  noiseVolume: number;
}

export const DEFAULT_SETTINGS: BooleanSettings = {
  removeOriginals: true,
  removeDegenerate: false,
  degenerateTolerance: 0.001,
  filterNoise: true,
  noiseVolume: 0.0001,
};

/** An input as the result was computed from it: a result is only kept while every input still has this mesh and matrix. */
export interface InputSnapshot {
  id: string;
  mesh: MeshData;
  matrix: readonly number[];
}

/** A computed result, shown in the viewport until OK adds it as a part or something drops it. */
export interface BooleanResult {
  op: BooleanOp;
  /** "Union of A, B" and so on: the new part's name. */
  name: string;
  /** Drawn in place of the inputs: world space, in the colour the new part will have. Not in the store. */
  part: Part;
  measured: AnalyzedMesh;
  /** Main list first, then the red list. */
  inputs: InputSnapshot[];
}

export interface BooleanSession {
  /** Part ids, in list order. */
  main: string[];
  tool: string[];
  /** The part selected in the lists (and highlighted in the viewport), or null. */
  selected: string | null;
  log: LogLine[];
  result: BooleanResult | null;
}

/** A run in progress (an operation, or Repair first). */
export interface BooleanBusy {
  job: number;
  label: string;
  /** 0..1, or null while unknown. */
  progress: number | null;
  /** What the run is doing now. */
  step: string | null;
  cancelling: boolean;
}

export interface BooleanState {
  /** Null outside Boolean operations. */
  session: BooleanSession | null;
  busy: BooleanBusy | null;
  tool: BooleanTool;
  settings: BooleanSettings;

  setTool(tool: BooleanTool): void;
  /** Changes some settings, keeping the others. */
  setSettings(patch: Partial<BooleanSettings>): void;
}

export const useBoolean = create<BooleanState>()((set) => ({
  session: null,
  busy: null,
  tool: "select",
  settings: DEFAULT_SETTINGS,

  setTool(tool) {
    set({ tool });
  },
  setSettings(patch) {
    set((s) => ({ settings: { ...s.settings, ...patch } }));
  },
}));
