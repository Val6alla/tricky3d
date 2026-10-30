import { create } from "zustand";
import { emptyHistory, pushEntry, redoStep, undoStep, type History, type Snapshot } from "./history";
import { cachedProblemEdges, cachedStats, primeMeshCaches } from "./meshCache";
import { boundsOf, IDENTITY, makeBox, translation, unionBounds } from "./mesh";
import { IS_MAC, selectHint } from "./platform";
import type { MeshStats } from "./io/ipc";
import type { Measurement } from "./measure/types";

export type Vec3 = [number, number, number];

/** Indexed triangle mesh in part-local millimetres. Never mutated in place. */
export interface MeshData {
  positions: Float32Array; // x, y, z per vertex
  indices: Uint32Array; // 3 per triangle, counter-clockwise = outward
}

/** How packing may turn a part (src-tauri/src/arrange/pack.rs Rotation): "default" takes the packing method's setting. */
export type PartRotation = "default" | "none" | "z" | "free";

/** A part's own packing settings (right-click › Packing settings). Its position lock is the part's `locked`. */
export interface PartPacking {
  /** 0 to 10; higher packs first and lowest. */
  priority: number;
  rotation: PartRotation;
}

/** What a part without settings of its own packs with. */
export const DEFAULT_PACKING: PartPacking = { priority: 5, rotation: "default" };

export interface Part {
  id: string;
  name: string;
  mesh: MeshData;
  /** Column-major 4×4 local-to-world matrix, same order as three.js Matrix4.elements. */
  matrix: number[];
  visible: boolean;
  transparent: boolean;
  locked: boolean;
  color: string; // CSS hex
  /** Measure mode's measurements on the part, in its own frame; absent when it has none. A new mesh drops them. */
  measurements?: readonly Measurement[];
  /** Its own packing settings; absent means DEFAULT_PACKING. */
  packing?: PartPacking;
  /**
   * The file the part was imported from, or the source of the part it was made from (cut pieces, shells, merges,
   * Boolean results, repairs, copies); absent for parts that never came from a file. Save and Export suggest its
   * name and open in its folder.
   */
  source?: string;
}

export type Projection = "perspective" | "orthographic";
export type Shading = "shaded" | "wireframe" | "shaded-edges";

export interface ViewSettings {
  projection: Projection;
  shading: Shading;
  /** Remembered between sessions; off by default, so the screen-edge rulers are the scale reference. */
  showPlatform: boolean;
  showAxes: boolean;
  showRulers: boolean;
  /** Show the move and rotate gizmo whenever parts are selected, not only after Move, Rotate, W, E or a scale. Remembered. */
  alwaysShowGizmo: boolean;
}

/** The active tool, shown in the status bar. */
export interface Tool {
  name: string;
  hint: string;
}

/** A numeric Move / Rotate / Scale dialog, and the undo label of a gizmo drag that does the same. */
export type TransformMode = "translate" | "rotate" | "scale";

/** Which gizmo the selection shows: the combined move/rotate gizmo, or the scale gizmo. */
export type GizmoMode = "combined" | "scale";

/** What to do with "Place on platform?" when importing a file. */
export type ImportPlacement = "ask" | "always" | "never";

/** Export's options, remembered between runs: how STL is written, and where several parts go. */
export interface ExportOptions {
  stl: "binary" | "ascii";
  /** All parts in one file, or one file per part named after it. */
  parts: "one-file" | "per-part";
}

export type SelectMode = "replace" | "add" | "toggle";

/**
 * Click modifiers → selection mode, shared by the viewport and the parts tree: Shift adds, Ctrl or Cmd toggles. On
 * macOS only Cmd toggles: Control-click is the right click there.
 */
export function selectMode(e: { shiftKey: boolean; ctrlKey: boolean; metaKey: boolean }, isMac = IS_MAC): SelectMode {
  if (e.metaKey || (e.ctrlKey && !isMac)) return "toggle";
  if (e.shiftKey) return "add";
  return "replace";
}

export interface ClipboardPart {
  name: string;
  mesh: MeshData;
  matrix: number[];
  color: string;
  locked: boolean;
  packing?: PartPacking;
  stats: MeshStats | null;
  edges: Float32Array | null;
  source?: string;
}

export interface NewPart {
  name: string;
  mesh: MeshData;
  matrix?: number[];
  color?: string;
  visible?: boolean;
  transparent?: boolean;
  locked?: boolean;
  measurements?: readonly Measurement[];
  packing?: PartPacking;
  source?: string;
}

/** Parts to take out (by id) and the parts that take their place. */
export interface Replacement {
  remove: string[];
  add: NewPart[];
  /** When nothing is removed: the part the new parts go right after (Apply repair › keep), instead of the end. */
  after?: string;
}

/** Display and naming fields that patchParts may change. */
export type PartPatch = Partial<Pick<Part, "name" | "visible" | "transparent" | "locked" | "color" | "packing">>;

/** A patch for parts that stay, applied in the same undo step as a replacement (Apply repair › compare). */
export interface PartsPatch {
  ids: string[];
  patch: PartPatch;
}

export const SELECT_TOOL: Tool = {
  name: "Select",
  hint: selectHint(IS_MAC),
};

export const GIZMO_TOOLS: Record<GizmoMode, Tool> = {
  combined: {
    name: "Move and rotate",
    hint: "Drag an arrow to move along an axis, a square to move in its plane, the centre ball to move across the view, or an arc to rotate. Hold Shift to snap to 1 mm or 15°. Double-click a handle to type exact values. Esc returns to Select.",
  },
  scale: {
    name: "Scale",
    hint: "Drag a handle to scale the selection. Hold Shift to snap to 5%. Double-click the gizmo to type exact values. Esc returns to the move and rotate gizmo.",
  },
};

/** Muted tints so parts stay near-neutral but can be told apart. The first is plain neutral. */
export const PART_COLORS = ["#a9b1bc", "#b9ae9d", "#a0b6a9", "#aaa4c0", "#bda3a3", "#9fb4c3", "#b6b59a", "#c0a9b9"];

const HIDE_SOON_KEY = "tricky3d.hideSoon";
const IMPORT_PLACEMENT_KEY = "tricky3d.importPlacement";
const EXPORT_STL_KEY = "tricky3d.exportStl";
const EXPORT_PARTS_KEY = "tricky3d.exportParts";
// View settings that are remembered, as "1" or "0". Anything else stored (or nothing) means the default.
const SHOW_PLATFORM_KEY = "tricky3d.showPlatform";
const ALWAYS_SHOW_GIZMO_KEY = "tricky3d.alwaysShowGizmo";

/** A remembered setting ("tricky3d." keys in the window's storage), or null when there is none or no storage. */
export function loadSetting(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

export function saveSetting(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Storage can be unavailable; the setting then lasts for this session only.
  }
}

function loadImportPlacement(): ImportPlacement {
  const value = loadSetting(IMPORT_PLACEMENT_KEY);
  return value === "always" || value === "never" ? value : "ask";
}

/** The export options remembered from an earlier run; the default (binary STL, one file) for any missing or unknown. */
export function loadExportOptions(): ExportOptions {
  return {
    stl: loadSetting(EXPORT_STL_KEY) === "ascii" ? "ascii" : "binary",
    parts: loadSetting(EXPORT_PARTS_KEY) === "per-part" ? "per-part" : "one-file",
  };
}

let partSeq = 0;
const nextPartId = () => `part-${++partSeq}`;

/** Applies `change` to the parts named in `ids`. Returns the same array when nothing changed, so no undo step is recorded. */
function mapParts(parts: Part[], ids: Iterable<string>, change: (p: Part) => Part): Part[] {
  const target = new Set(ids);
  let changed = false;
  const out = parts.map((p) => {
    if (!target.has(p.id)) return p;
    const next = change(p);
    if (next !== p) changed = true;
    return next;
  });
  return changed ? out : parts;
}

function patchPart(p: Part, patch: PartPatch): Part {
  for (const key of Object.keys(patch) as (keyof PartPatch)[]) {
    if (patch[key] !== undefined && patch[key] !== p[key]) return { ...p, ...patch };
  }
  return p;
}

function sameMatrix(a: readonly number[], b: readonly number[]): boolean {
  return a.length === b.length && a.every((v, i) => v === b[i]);
}

function buildParts(newParts: NewPart[], colorStart: number): Part[] {
  return newParts.map(
    (p, i): Part => ({
      id: nextPartId(),
      name: p.name,
      mesh: p.mesh,
      matrix: p.matrix ? [...p.matrix] : [...IDENTITY],
      visible: p.visible ?? true,
      transparent: p.transparent ?? false,
      locked: p.locked ?? false,
      color: p.color ?? PART_COLORS[(colorStart + i) % PART_COLORS.length],
      ...(p.measurements && p.measurements.length > 0 ? { measurements: p.measurements } : {}),
      ...(p.packing ? { packing: p.packing } : {}),
      ...(p.source ? { source: p.source } : {}),
    }),
  );
}

export interface ProjectData {
  name: string;
  /** File the project was opened from, or null for a new project. */
  path: string | null;
  platform: Vec3;
  parts: NewPart[];
}

export interface AppState {
  projectName: string;
  /** Where Save writes; null until the project is saved or opened. */
  projectPath: string | null;
  parts: Part[];
  /** Selected part ids, in the order they were selected. */
  selection: string[];
  /** Build platform size in mm (X, Y, Z). The platform spans 0..size on each axis. */
  platform: Vec3;
  /** Parts and platform as last saved or opened; see isDirty(). */
  saved: { parts: Part[]; platform: Vec3 };
  /** Bumped by New project and Open project, so slow work (imports, saves) can tell its project was replaced. */
  generation: number;
  history: History;
  view: ViewSettings;
  tool: Tool;
  /** The gizmo tool picked with Move, Rotate, Scale, W, E or R; null is the Select tool. See shownGizmo(). */
  gizmoMode: GizmoMode | null;
  /** Settings → "Hide unavailable features". */
  hideSoon: boolean;
  importPlacement: ImportPlacement;
  exportOptions: ExportOptions;
  /** Parts last copied, in parts-list order. Empty after New project and before the first copy. */
  clipboard: ClipboardPart[];
  /** How many times the current clipboard has been pasted and not undone. The next paste steps by one more gap. */
  pasteCount: number;
  /**
   * The folder of the last file picked or dropped (import, open, save, export) in this session, or null. Save and
   * Export open there for parts that never came from a file, or whose folder is gone.
   */
  lastFolder: string | null;

  /** Adds parts as one undo step and selects them. Returns the new ids. */
  addParts(parts: NewPart[], label?: string): string[];
  addTestCube(): void;
  /** Removes unlocked parts among `ids`. Returns how many were skipped because they are locked. */
  deleteParts(ids: string[]): number;
  /** Copies parts beside the originals and selects the copies. Returns the new ids. */
  duplicateParts(ids: string[]): string[];
  copySelected(): number;
  pasteClipboard(): string[];
  /**
   * Swaps parts for new ones as one undo step labelled `label` (Cut, Union, Split shells, Repair...). Each
   * replacement's new parts take the place in the list of the first part it removes, or when it removes none
   * go right after its `after` part, else at the end; all the new parts become the selection. Locked parts
   * are removed too: callers decide what may be replaced. `patch` changes display fields of parts that stay,
   * in the same undo step. Returns the new ids in order.
   */
  replaceParts(replacements: Replacement[], label: string, patch?: PartsPatch): string[];
  renamePart(id: string, name: string): void;
  setPartsVisible(ids: string[], visible: boolean): void;
  setPartsTransparent(ids: string[], transparent: boolean): void;
  setPartLocked(id: string, locked: boolean): void;
  /** Changes display or naming fields of several parts as one undo step labelled `label`. */
  patchParts(ids: string[], patch: PartPatch, label: string): void;
  /** patchParts with a patch of its own for each part (by id). */
  patchEach(patches: ReadonlyMap<string, PartPatch>, label: string): void;
  /** Sets the measurements of parts (by id; an empty list removes them) as one undo step labelled `label`. */
  setMeasurements(changes: ReadonlyMap<string, readonly Measurement[]>, label: string): void;
  /**
   * Sets part matrices (column-major, as Part.matrix). With a label this is one undo step. With
   * null it is a live preview, e.g. during a gizmo drag, that recordChange() later makes one step.
   */
  setPartMatrices(matrices: ReadonlyMap<string, number[]>, label: string | null): void;
  /**
   * Moves parts (by id, to these matrices) and adds new ones as one undo step labelled `label`, and selects the new
   * ones (Duplicate › Fill the platform moves the originals and adds the copies). Returns the new ids.
   */
  arrangeParts(matrices: ReadonlyMap<string, number[]>, add: NewPart[], label: string): string[];
  /** The current parts and selection, to pass to recordChange() after live previews. */
  snapshot(): Snapshot;
  /** Ends a live preview without an undo step: the parts and selection go back to `before` (Orient's Cancel). */
  discardPreview(before: Snapshot): void;
  /** Records everything that changed since `before` as one undo step. Does nothing if the parts did not change. */
  recordChange(label: string, before: Snapshot): void;
  undo(): void;
  redo(): void;
  select(ids: string[], mode?: SelectMode): void;
  clearSelection(): void;
  setPlatform(size: Vec3): void;
  /** Changes view settings; the remembered ones (showPlatform, alwaysShowGizmo) in the patch are saved too. */
  setView(patch: Partial<ViewSettings>): void;
  setTool(tool: Tool): void;
  /** Picks a gizmo tool, or returns to the Select tool (null). */
  setGizmoMode(mode: GizmoMode | null): void;
  setHideSoon(hide: boolean): void;
  setImportPlacement(value: ImportPlacement): void;
  /** Remembers Export's options for the next export, also after a restart. */
  setExportOptions(options: ExportOptions): void;
  setLastFolder(folder: string | null): void;
  newProject(): void;
  /** Replaces the scene with an opened project. Clears the undo history; the result counts as saved. */
  loadProject(project: ProjectData): void;
  /**
   * Marks the project as saved to `path`. `written` is what the file holds when it differs from the
   * current state (edits made while it was written stay unsaved); by default the current state.
   */
  markSaved(path: string, name: string, written?: { parts: Part[]; platform: Vec3 }): void;
}

const initialParts: Part[] = [];
const initialPlatform: Vec3 = [200, 200, 200];

export const useStore = create<AppState>()((set, get) => {
  /** Bumped on each copy. Pastes record it so undo of an older copy does not move the next paste. */
  let clipboardGen = 0;

  /** Records the change from `before` to the current state as one undo step, if the parts changed. */
  function commit(label: string, before: Snapshot) {
    const s = get();
    if (s.parts === before.parts) return;
    const entry = { label, before, after: { parts: s.parts, selection: s.selection } };
    set({ history: pushEntry(s.history, entry, s.parts) });
  }

  const snap = (): Snapshot => ({ parts: get().parts, selection: get().selection });

  /** The state before a live preview (a gizmo drag) that recordChange() has not made an undo step yet. */
  let preview: Snapshot | null = null;

  /**
   * Snapshot before an edit. A preview still running becomes its own undo step first, so an edit
   * that lands mid-drag (an import finishing, Delete, undo) never folds the preview into its step.
   */
  function begin(): Snapshot {
    if (preview) {
      const previewBefore = preview;
      preview = null;
      commit("Transform", previewBefore);
    }
    return snap();
  }

  /** Restores a snapshot; the selection keeps only ids that exist in it. */
  function restore(target: Snapshot, history: History) {
    const known = new Set(target.parts.map((p) => p.id));
    set({ history, parts: target.parts, selection: target.selection.filter((id) => known.has(id)) });
  }

  return {
    projectName: "Untitled",
    projectPath: null,
    parts: initialParts,
    selection: [],
    platform: initialPlatform,
    saved: { parts: initialParts, platform: initialPlatform },
    generation: 0,
    history: emptyHistory(),
    view: {
      projection: "perspective",
      shading: "shaded",
      showPlatform: loadSetting(SHOW_PLATFORM_KEY) === "1",
      showAxes: true,
      showRulers: true,
      alwaysShowGizmo: loadSetting(ALWAYS_SHOW_GIZMO_KEY) !== "0",
    },
    tool: SELECT_TOOL,
    gizmoMode: null,
    hideSoon: loadSetting(HIDE_SOON_KEY) === "1",
    importPlacement: loadImportPlacement(),
    exportOptions: loadExportOptions(),
    clipboard: [],
    pasteCount: 0,
    lastFolder: null,

    addParts(newParts, label = "Add parts") {
      if (newParts.length === 0) return [];
      const before = begin();
      const parts = buildParts(newParts, get().parts.length);
      const ids = parts.map((p) => p.id);
      set((s) => ({ parts: [...s.parts, ...parts], selection: ids }));
      commit(label, before);
      return ids;
    },

    addTestCube() {
      const [px, py] = get().platform;
      const size = 20;
      const count = get().parts.filter((p) => p.name.startsWith("Test cube")).length;
      get().addParts(
        [
          {
            name: count === 0 ? "Test cube" : `Test cube ${count + 1}`,
            mesh: makeBox([size, size, size]),
            // The box is centred on its local origin; lift it so it sits on the platform centre.
            matrix: translation(px / 2 + count * (size + 5), py / 2, size / 2),
          },
        ],
        "Add test cube",
      );
    },

    deleteParts(ids) {
      const doomed = new Set(ids);
      const parts = get().parts;
      const locked = parts.filter((p) => doomed.has(p.id) && p.locked).length;
      const removed = new Set(parts.filter((p) => doomed.has(p.id) && !p.locked).map((p) => p.id));
      if (removed.size === 0) return locked;
      const before = begin();
      set((s) => ({
        parts: s.parts.filter((p) => !removed.has(p.id)),
        selection: s.selection.filter((id) => !removed.has(id)),
      }));
      commit("Delete", before);
      return locked;
    },

    duplicateParts(ids) {
      const wanted = new Set(ids);
      const sources = get().parts.filter((p) => wanted.has(p.id));
      // Every copy moves along +X past the whole group plus a 5 mm gap, as Paste does: the copies keep their places
      // relative to each other, and none lands on an original. They pack as their originals do (lock included).
      const box = unionBounds(sources.map((p) => boundsOf(p.mesh, p.matrix)));
      const dx = box ? box.max[0] - box.min[0] + 5 : 10;
      const copies = sources.map((p): NewPart => {
        const m = [...p.matrix];
        m[12] += dx;
        return { name: `${p.name} (copy)`, mesh: p.mesh, matrix: m, color: p.color, locked: p.locked, packing: p.packing, source: p.source };
      });
      return get().addParts(copies, "Duplicate");
    },

    copySelected() {
      const wanted = new Set(get().selection);
      const sources = get().parts.filter((p) => wanted.has(p.id));
      if (sources.length === 0) return 0;
      clipboardGen += 1;
      set({
        clipboard: sources.map((p) => ({
          name: p.name,
          mesh: p.mesh,
          matrix: [...p.matrix],
          color: p.color,
          locked: p.locked,
          ...(p.packing ? { packing: p.packing } : {}),
          stats: cachedStats(p.mesh) ?? null,
          edges: cachedProblemEdges(p.mesh) ?? null,
          source: p.source,
        })),
        pasteCount: 0,
      });
      return sources.length;
    },

    pasteClipboard() {
      const clip = get().clipboard;
      if (clip.length === 0) return [];
      const box = unionBounds(clip.map((p) => boundsOf(p.mesh, p.matrix)));
      const dx = box ? box.max[0] - box.min[0] + 5 : 10;
      const shift = dx * (get().pasteCount + 1);
      for (const p of clip) {
        if (p.stats || p.edges) primeMeshCaches(p.mesh, { stats: p.stats, edges: p.edges });
      }
      const copies = clip.map((p): NewPart => {
        const matrix = [...p.matrix];
        matrix[12] += shift;
        return { name: p.name, mesh: p.mesh, matrix, color: p.color, locked: p.locked, packing: p.packing, source: p.source };
      });
      const label = clip.length === 1 ? `Paste ${clip[0].name}` : `Paste ${clip.length} parts`;
      const ids = get().addParts(copies, label);
      if (ids.length > 0) {
        const entry = get().history.past[get().history.past.length - 1];
        if (entry) entry.clipboard = clipboardGen;
        set({ pasteCount: get().pasteCount + 1 });
      }
      return ids;
    },

    replaceParts(replacements, label, patch) {
      const before = begin();
      const index = new Map(before.parts.map((p, i) => [p.id, i]));
      const doomed = new Set(replacements.flatMap((r) => r.remove).filter((id) => index.has(id)));
      const added = buildParts(
        replacements.flatMap((r) => r.add),
        before.parts.length,
      );
      const kept = patch
        ? mapParts(
            before.parts,
            patch.ids.filter((id) => !doomed.has(id)),
            (p) => patchPart(p, patch.patch),
          )
        : before.parts;
      if (added.length === 0 && doomed.size === 0 && kept === before.parts) return [];
      // New parts by the list index they go in front of, or (`after`) behind; the rest go at the end.
      const slots = new Map<number, Part[]>();
      const behind = new Map<number, Part[]>();
      const tail: Part[] = [];
      const put = (map: Map<number, Part[]>, at: number, mine: Part[]) => map.set(at, [...(map.get(at) ?? []), ...mine]);
      let next = 0;
      for (const r of replacements) {
        const mine = added.slice(next, (next += r.add.length));
        const at = Math.min(...r.remove.map((id) => index.get(id) ?? Infinity));
        const anchor = r.after === undefined ? undefined : index.get(r.after);
        if (at !== Infinity) put(slots, at, mine);
        else if (anchor === undefined) tail.push(...mine);
        // An anchor that is removed itself leaves its place to them.
        else put(doomed.has(before.parts[anchor].id) ? slots : behind, anchor, mine);
      }
      const parts = kept.flatMap((p, i) => [...(slots.get(i) ?? []), ...(doomed.has(p.id) ? [] : [p]), ...(behind.get(i) ?? [])]);
      set({ parts: [...parts, ...tail], selection: added.map((p) => p.id) });
      commit(label, before);
      return added.map((p) => p.id);
    },

    renamePart(id, name) {
      get().patchParts([id], { name: name.trim() || undefined }, "Rename");
    },

    setPartsVisible(ids, visible) {
      get().patchParts(ids, { visible }, visible ? "Show" : "Hide");
    },

    setPartsTransparent(ids, transparent) {
      get().patchParts(ids, { transparent }, transparent ? "Make transparent" : "Make solid");
    },

    setPartLocked(id, locked) {
      get().patchParts([id], { locked }, locked ? "Lock" : "Unlock");
    },

    patchParts(ids, patch, label) {
      const before = begin();
      const parts = mapParts(before.parts, ids, (p) => patchPart(p, patch));
      if (parts === before.parts) return;
      set({ parts });
      commit(label, before);
    },

    patchEach(patches, label) {
      const before = begin();
      const parts = mapParts(before.parts, patches.keys(), (p) => patchPart(p, patches.get(p.id) ?? {}));
      if (parts === before.parts) return;
      set({ parts });
      commit(label, before);
    },

    setMeasurements(changes, label) {
      const before = begin();
      const parts = mapParts(before.parts, changes.keys(), (p) => {
        const list = changes.get(p.id) ?? [];
        if ((p.measurements ?? []).length === 0 && list.length === 0) return p;
        const { measurements: _old, ...rest } = p;
        return list.length > 0 ? { ...rest, measurements: list } : rest;
      });
      if (parts === before.parts) return;
      set({ parts });
      commit(label, before);
    },

    setPartMatrices(matrices, label) {
      const before = label === null ? snap() : begin();
      const parts = mapParts(before.parts, matrices.keys(), (p) => {
        const m = matrices.get(p.id);
        return m && !sameMatrix(m, p.matrix) ? { ...p, matrix: [...m] } : p;
      });
      if (parts === before.parts) return;
      if (label === null) preview ??= before;
      set({ parts });
      if (label !== null) commit(label, before);
    },

    arrangeParts(matrices, add, label) {
      const before = begin();
      const moved = mapParts(before.parts, matrices.keys(), (p) => {
        const m = matrices.get(p.id);
        return m && !sameMatrix(m, p.matrix) ? { ...p, matrix: [...m] } : p;
      });
      const added = buildParts(add, before.parts.length);
      if (moved === before.parts && added.length === 0) return [];
      const ids = added.map((p) => p.id);
      set({ parts: [...moved, ...added], selection: ids.length ? ids : before.selection });
      commit(label, before);
      return ids;
    },

    snapshot: snap,

    discardPreview(before) {
      preview = null;
      set({ parts: before.parts, selection: before.selection });
    },

    recordChange(label, before) {
      preview = null;
      commit(label, before);
    },

    undo() {
      begin();
      const step = undoStep(get().history);
      if (!step) return;
      restore(step.entry.before, step.history);
      if (step.entry.clipboard === clipboardGen) set({ pasteCount: Math.max(0, get().pasteCount - 1) });
    },

    redo() {
      // Mid-drag the preview is the newest change; recording it would clear the redo list.
      if (preview) return;
      const step = redoStep(get().history);
      if (!step) return;
      restore(step.entry.after, step.history);
      if (step.entry.clipboard === clipboardGen) set({ pasteCount: get().pasteCount + 1 });
    },

    select(ids, mode = "replace") {
      const known = new Set(get().parts.map((p) => p.id));
      const valid = ids.filter((id) => known.has(id));
      set((s) => {
        if (mode === "replace") return { selection: [...new Set(valid)] };
        if (mode === "add") return { selection: [...new Set([...s.selection, ...valid])] };
        const current = new Set(s.selection);
        for (const id of valid) {
          if (current.has(id)) current.delete(id);
          else current.add(id);
        }
        return { selection: [...current] };
      });
    },

    clearSelection() {
      if (get().selection.length) set({ selection: [] });
    },

    setPlatform(size) {
      // Same size again (OK in the dialog without changes) is not an edit.
      if (size.every((v, i) => v === get().platform[i])) return;
      if (size.every((v) => Number.isFinite(v) && v > 0)) set({ platform: size });
    },

    setView(patch) {
      if (patch.showPlatform !== undefined) saveSetting(SHOW_PLATFORM_KEY, patch.showPlatform ? "1" : "0");
      if (patch.alwaysShowGizmo !== undefined) saveSetting(ALWAYS_SHOW_GIZMO_KEY, patch.alwaysShowGizmo ? "1" : "0");
      set((s) => ({ view: { ...s.view, ...patch } }));
    },

    setTool(tool) {
      set({ tool });
    },

    setGizmoMode(mode) {
      set({ gizmoMode: mode, tool: mode ? GIZMO_TOOLS[mode] : SELECT_TOOL });
    },

    setHideSoon(hide) {
      saveSetting(HIDE_SOON_KEY, hide ? "1" : "0");
      set({ hideSoon: hide });
    },

    setImportPlacement(value) {
      saveSetting(IMPORT_PLACEMENT_KEY, value);
      set({ importPlacement: value });
    },

    setExportOptions(options) {
      saveSetting(EXPORT_STL_KEY, options.stl);
      saveSetting(EXPORT_PARTS_KEY, options.parts);
      set({ exportOptions: { stl: options.stl, parts: options.parts } });
    },

    setLastFolder(folder) {
      if (folder !== get().lastFolder) set({ lastFolder: folder });
    },

    newProject() {
      preview = null;
      const parts: Part[] = [];
      set((s) => ({
        projectName: "Untitled",
        projectPath: null,
        parts,
        selection: [],
        saved: { parts, platform: s.platform },
        generation: s.generation + 1,
        history: emptyHistory(),
        tool: SELECT_TOOL,
        gizmoMode: null,
        clipboard: [],
        pasteCount: 0,
      }));
    },

    loadProject({ name, path, platform, parts: newParts }) {
      preview = null;
      const parts = buildParts(newParts, 0);
      set((s) => ({
        projectName: name,
        projectPath: path,
        parts,
        selection: [],
        platform,
        saved: { parts, platform },
        generation: s.generation + 1,
        history: emptyHistory(),
        tool: SELECT_TOOL,
        gizmoMode: null,
      }));
    },

    markSaved(path, name, written) {
      set((s) => ({ projectPath: path, projectName: name, saved: written ?? { parts: s.parts, platform: s.platform } }));
    },
  };
});

/** Ids of the selected parts, or every visible part when nothing is selected. */
export function targetIds(s: AppState): string[] {
  return s.selection.length ? s.selection : s.parts.filter((p) => p.visible).map((p) => p.id);
}

/** The gizmo the selection shows: the picked tool's, else the combined gizmo when View › Always show gizmo is on. */
export function shownGizmo(s: Pick<AppState, "gizmoMode" | "view">): GizmoMode | null {
  return s.gizmoMode ?? (s.view.alwaysShowGizmo ? "combined" : null);
}

/** True when the parts or platform differ from the last save (an undo back to the saved state counts as clean). */
export function isDirty(s: AppState): boolean {
  if (s.platform.some((v, i) => v !== s.saved.platform[i])) return true;
  const saved = s.saved.parts;
  return s.parts.length !== saved.length || s.parts.some((p, i) => p !== saved[i]);
}

export const canUndo = (s: AppState) => s.history.past.length > 0;
export const canRedo = (s: AppState) => s.history.future.length > 0;
