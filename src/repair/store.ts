// Repair mode's state: the session (working copy, selection, inner history, status), the run in progress, the
// repair scripts, and the panel settings. src/repair/session.ts is the only code that changes `session` and
// `busy`, and src/repair/scripts.ts the only code that changes the catalog and the scripts; the settings below
// have plain setters for the panel and the ribbon.
import { create } from "zustand";
import type { Catalog, EditTool, HighlightSettings, RepairBusy, RepairScript, RepairSession, SelectTool, ViewSettings } from "./types";

export type RepairTab = "status" | "actions" | "scripts" | "shells" | "view";

/** Status tab highlighting: everything on, the dihedral edge highlight off. */
export const DEFAULT_HIGHLIGHT: HighlightSettings = { holes: true, triangles: true, edgesFrom: null, degenerate: true, errors: true };

/** View tab: slim faces not shown, 10° smallest angle; Surface selection crosses every edge (180°). */
export const DEFAULT_VIEW: ViewSettings = { slimColored: false, slimEdges: false, slimAngle: 10, surfaceTolerance: 180 };

export interface RepairState {
  /** Every action and the built-in scripts, from Rust; null until loaded (src/repair/scripts.ts). */
  catalog: Catalog | null;
  /** The user's scripts and the picker order (loaded once from the config folder). */
  ownScripts: RepairScript[];
  /** Script ids, built-in and own, in picker order; ids not listed go at the end. */
  order: string[];
  /** The script selected in the Repair scripts tab (id). */
  editingScriptId: string | null;
  /** Null outside repair mode. */
  session: RepairSession | null;
  /** The run in progress, for the progress dialog; null when none runs. */
  busy: RepairBusy | null;
  /** What a click in the viewport selects. */
  selectTool: SelectTool;
  /** Move nodes or Cut surfaces, which take the viewport's clicks instead of the select tool; null for neither. */
  editTool: EditTool | null;
  panelTab: RepairTab;
  /** Recompute the status after every action. */
  autoUpdate: boolean;
  highlight: HighlightSettings;
  view: ViewSettings;
  /** Shells tab: clicking a row selects that shell's triangles. */
  shellsAutoSelect: boolean;

  /** Picks a select tool, which ends Move nodes or Cut surfaces. */
  setSelectTool(tool: SelectTool): void;
  setEditTool(tool: EditTool | null): void;
  setPanelTab(tab: RepairTab): void;
  setAutoUpdate(on: boolean): void;
  /** Changes some highlighting switches, keeping the others. */
  setHighlight(patch: Partial<HighlightSettings>): void;
  /** Changes some View tab settings, keeping the others. */
  setView(patch: Partial<ViewSettings>): void;
  setShellsAutoSelect(on: boolean): void;
  setEditingScriptId(id: string | null): void;
}

export const useRepair = create<RepairState>()((set) => ({
  catalog: null,
  ownScripts: [],
  order: [],
  editingScriptId: null,
  session: null,
  busy: null,
  selectTool: "triangle",
  editTool: null,
  panelTab: "status",
  autoUpdate: false,
  highlight: DEFAULT_HIGHLIGHT,
  view: DEFAULT_VIEW,
  shellsAutoSelect: true,

  setSelectTool(selectTool) {
    set({ selectTool, editTool: null });
  },
  setEditTool(editTool) {
    set({ editTool });
  },
  setPanelTab(panelTab) {
    set({ panelTab });
  },
  setAutoUpdate(autoUpdate) {
    set({ autoUpdate });
  },
  setHighlight(patch) {
    set((s) => ({ highlight: { ...s.highlight, ...patch } }));
  },
  setView(patch) {
    set((s) => ({ view: { ...s.view, ...patch } }));
  },
  setShellsAutoSelect(shellsAutoSelect) {
    set({ shellsAutoSelect });
  },
  setEditingScriptId(editingScriptId) {
    set({ editingScriptId });
  },
}));
