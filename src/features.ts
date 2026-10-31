// Every ribbon button, File-menu item and context-menu item comes from FEATURES below. To ship a
// feature, give it a run() and set status: "ready".
import type { LucideIcon } from "lucide-react";
import {
  Activity, AlignCenterHorizontal, AlignCenterVertical, AlignEndHorizontal, AlignEndVertical, AlignStartHorizontal, AlignVerticalJustifyCenter, AppWindow, ArrowUpToLine, Atom, Compass, Construction, FileCode, History, Printer, Sheet, SunDim, Zap, AlignStartVertical, ArrowBigUp, ArrowDownLeft, ArrowDownToDot, ArrowDownToLine,
  ArrowDownUp, ArrowLeftRight, ArrowUpFromLine, Axis3d, BadgeInfo, Ban, Blend, Blocks, Box, Boxes, Bridge, Brush,
  Camera, ChartArea, Check, CheckCheck, ChevronsDownUp, CircleDashed, CircleDot, CircleFadingPlus, CirclePlay,
  CirclePlus, CircleSmall, ClipboardList, ClipboardPaste, Combine, Contrast, Copy, CopyCheck, CopyPlus, CopyX,
  Crosshair, Cuboid, Cylinder, Dices, Diff, Dot, DraftingCompass, Drill, Eraser, Expand, Eye, EyeOff, FileAxis3d,
  FileClock, FileOutput, FilePen, FilePlus, FileSearch, FileSliders, FileStack, FileText, Focus, FolderOpen,
  FolderOutput, Fullscreen, Gem, Globe, Grid2x2, Grid3x2, Grid3x3, Group, HandGrab, Hexagon, Image, Images,
  ImageUp, Import, Info, IterationCcw, Keyboard, Languages, Layers, Layers2, LayoutDashboard, LayoutGrid,
  LayoutTemplate, Library, LibraryBig, Link, Link2, List, ListChecks, ListFilter, ListOrdered, LocateFixed, MapPin,
  MessageSquareWarning, Minimize, Minimize2, MousePointer2, Move, Move3d, MoveDiagonal2, Network, OctagonAlert,
  Package, PackageMinus, PackageOpen, PackagePlus, PaintBucket, Paintbrush, Palette, PanelBottom, PanelLeft,
  PanelRight, PanelsTopLeft, PanelTop, PenLine, PenTool, Pencil, Play, Plus, Proportions, Pyramid, QrCode,
  RectangleHorizontal, Redo2, RefreshCcw, Rotate3d, Rows3, Ruler, RulerDimensionLine, Save, SaveAll, Scale3d, Scan,
  ScanLine, ScanSearch, Scissors, ScissorsLineDashed, ScrollText, Search, Settings, Settings2, Shapes, Shrink,
  SlidersHorizontal, Spline, SplinePointer, Split, Square, SquareArrowDown, SquareArrowOutUpRight, SquareCheckBig,
  SquareDashed, SquareMinus, SquarePlus, SquareSquare, SquareStack,
  SquareX, SquaresIntersect, SquaresUnite, Stamp, SwatchBook, Tag, Tags, Target, TextCursorInput, Timer, ToggleLeft,
  Trash, Triangle, TriangleAlert, TriangleDashed, TriangleRight, Type, Undo2, Unlink, Unlink2, VectorSquare,
  WandSparkles, WavesHorizontal, Waypoints, Wrench, X,
} from "lucide-react";
import { browserOnly, confirmDiscard, exportSelection, importPart, openFile, saveProject, saveProjectAs } from "./io/files";
import { boundsOf, unionBounds, type Bounds } from "./mesh";
import { acceptBoolean, CLOSE_BOOLEAN_FIRST, closeBooleanMode, isBooleanOpen, openBooleanMode, zoomToBooleanPart } from "./boolean/session";
import { useBoolean, type BooleanState, type BooleanTool } from "./boolean/store";
import { meshOperationRunning, mergePartsCommand, openCutDialog, splitShellsCommand, WAIT_MESSAGE } from "./ops/meshOps";
import { IS_MAC, navigationHint, toggleKey } from "./platform";
import {
  CLOSE_REPAIR_FIRST,
  SELECT_PART_FIRST,
  SELECT_TRIANGLES_FIRST,
  canRedoRepair,
  canUndoRepair,
  closeRepair,
  expandSelection,
  extractSelection,
  invertSelection,
  isRepairing,
  redoRepair,
  requestApply,
  runAction,
  selectAll,
  selectAt,
  selectNone,
  selectedBounds,
  showSelectionInfo,
  shrinkSelection,
  startRepair,
  undoRepair,
  workingCopyBounds,
} from "./repair/session";
import { ensureMeshChecks } from "./repair/badges";
import {
  addPoint,
  addTriangle,
  batchRepair,
  closeHolesCurved,
  cutSurfaces,
  damageWarnings,
  editRepairScripts,
  extractSelectedShells,
  findOverlaps,
  fixSelfIntersections,
  hideTriangles,
  invertShells,
  mergeDuplicatePoints,
  moveNodes,
  offsetHoleEdge,
  problemCounts,
  refineTriangles,
  removeDegenerate,
  removeDuplicateFaces,
  removeProblemAreas,
  removeTinyShells,
  runRepairScript,
  selectByCriteria,
  shellList,
  wrapSurface,
} from "./repair/home";
import { useRepair, type RepairState } from "./repair/store";
import { CLOSE_LIBRARY_FIRST, closePartLibrary, createPart, isLibraryOpen, openPartLibrary } from "./library/session";
import {
  applyLabelMode,
  CLOSE_LABEL_FIRST,
  closeLabelMode,
  isLabelOpen,
  labelHistory,
  labelRedo,
  labelUndo,
  openLabelMode,
  pickLabelTool,
  placeKind,
  zoomToLabel,
  zoomToLabelPart,
} from "./label/session";
import type { LabelState } from "./label/store";
import { closeAnalysisView, closeShownAnalysis, newAnalysis, shownAnalysis } from "./analysis/session";
import type { AnalysisKind } from "./analysis/types";
import { closeCollisions, interlockingTest, openCollisions } from "./analysis/collisions";
import type { LabelKind } from "./label/types";
import type { AlignHow, DuplicateMode } from "./arrange/types";
import { openGenerateShell } from "./shell/session";
import { alignParts, layFlatAuto, openDuplicate, openMinBox, openOrient, startLayFlat } from "./arrange/session";
import { openPack, PACK_METHODS, type PackMethod } from "./arrange/pack";
import { GROUP_ICONS, toolIcon } from "./measure/icons";
import { CLOSE_MEASURE_FIRST, closeMeasure, currentTool, isMeasureOpen, openMeasure, pickMeasureTool } from "./measure/session";
import type { MeasureState } from "./measure/store";
import { GROUP_LABELS, TOOLS, describe, nextClickText, type ToolGroup } from "./measure/tools";
import type { ActionId, EditTool, SelectTool } from "./repair/types";
import { canRedo, canUndo, useStore, type AppState, type Part, type Tool, type Vec3, type ViewSettings } from "./store";
import { centreOnPlatform, placeOnPlatform, startTransform } from "./transformTools";
import { toast, useUi, type ViewTool } from "./ui";
import { viewport, type ViewPreset, type ZoomTarget } from "./viewport/api";

/**
 * "repair" is the contextual ribbon tab shown in repair mode, "repair-context" the repair right-click menu, "boolean"
 * the contextual tab of Boolean operations, "library" the Part library's, "measure" the Measurement tab, and "quick" the
 * quick-action strip at the bottom of the right panel.
 */
export type TabId =
  | "file"
  | "home"
  | "analyze"
  | "modify"
  | "arrange"
  | "view"
  | "context"
  | "repair"
  | "repair-context"
  | "boolean"
  | "library"
  | "measure"
  | "label"
  | "analysis"
  | "quick";
export type FeatureStatus = "ready" | "soon";

export interface Feature {
  id: string;
  /** "file" is the File dropdown menu; "context" entries appear only in the part context menu. */
  tab: TabId;
  group: string;
  label: string;
  icon: LucideIcon;
  /** For a menu (an entry that other entries name as `parent`) this is derived from its children. */
  status: FeatureStatus;
  run?: () => void;
  /** Id of the menu entry this item appears in. Items with a parent are not rendered directly in the group. */
  parent?: string;
  /**
   * A split button: the button itself runs, and its arrow opens these rows, built when it opens (Undo and Redo list
   * the steps they would take back or bring back).
   */
  dropdown?: () => (Feature | "-")[];
  /** One sentence for the tooltip and status bar. */
  hint?: string;
  /** Display string, e.g. "Ctrl+Z". Shortcuts themselves are bound in src/keyboard.ts. */
  shortcut?: string;
  /** Large buttons show a big icon with the label underneath. Default "small". */
  size?: "large" | "small";
  /** Pressed state for toggles (projection, shading, show platform...). */
  isActive?: (s: AppState) => boolean;
  /** Pressed state for repair mode toggles (the select tools), from the repair store. */
  repairActive?: (r: RepairState) => boolean;
  /** Pressed state for Boolean operations' viewport tools, from the Boolean store. */
  booleanActive?: (b: BooleanState) => boolean;
  /** Pressed while this Zoom to tool waits for its drag or click (the UI store's viewTool). */
  viewTool?: ViewTool;
  /** Pressed state for Measure mode's tools, from the measure store. */
  measureActive?: (m: MeasureState) => boolean;
  /** Pressed state for label generation's tools, from the label store. */
  labelActive?: (l: LabelState) => boolean;
  /** Repair mode: greyed out and not runnable while no triangle of the working copy is selected. */
  needsTriangles?: boolean;
  /** Repair context menu: acts on the triangle under the cursor; greyed out when the menu did not open on one. */
  underCursor?: boolean;
  /**
   * A main-document command that cannot work while repair mode or Boolean operations is open, because it would change
   * the parts the mode works on or shows a right-panel section the mode's panel replaces: greyed out then.
   */
  outsideRepair?: boolean;
}

/** A ribbon tab. A contextual tab shows only in its mode, after the others, set apart and with an icon. */
export interface RibbonTab {
  id: TabId;
  label: string;
  contextual?: boolean;
  icon?: LucideIcon;
}

export const TABS: RibbonTab[] = [
  { id: "home", label: "Home" },
  { id: "analyze", label: "Analyze" },
  { id: "modify", label: "Modify" },
  { id: "arrange", label: "Arrange" },
  { id: "view", label: "View" },
];

/** The contextual tab of repair mode. Entering repair mode selects it (src/repair/session.ts). */
export const REPAIR_TAB: RibbonTab = { id: "repair", label: "Repair", contextual: true, icon: Wrench };

/** The contextual tab of Boolean operations. Modify › Boolean selects it (src/boolean/session.ts). */
export const BOOLEAN_TAB: RibbonTab = { id: "boolean", label: "Boolean operations", contextual: true, icon: SquaresUnite };

/** The contextual tab of the Part library. Home › Part library selects it (src/library/session.ts). */
export const LIBRARY_TAB: RibbonTab = { id: "library", label: "Part library", contextual: true, icon: Library };

/** The contextual tab of Measure. Home › Measure and Analyze › Measure select it (src/measure/session.ts). */
export const MEASURE_TAB: RibbonTab = { id: "measure", label: "Measurement", contextual: true, icon: Ruler };

/** The contextual tab of label generation. Generate label › any kind selects it (src/label/session.ts). */
export const LABEL_TAB: RibbonTab = { id: "label", label: "Label generation", contextual: true, icon: Tag };

/** The contextual tab of an analysis on show. Picking an analysis in the parts tree selects it (src/analysis/session.ts). */
export const ANALYSIS_TAB: RibbonTab = { id: "analysis", label: "Part analysis", contextual: true, icon: ChartArea };

/** The tabs the ribbon shows: the fixed ones, and the open mode's contextual tab after them. */
export function ribbonTabs(
  repairing: boolean,
  booleanOpen = false,
  libraryOpen = false,
  measureOpen = false,
  labelOpen = false,
  analysisOpen = false,
): RibbonTab[] {
  if (repairing) return [...TABS, REPAIR_TAB];
  if (booleanOpen) return [...TABS, BOOLEAN_TAB];
  if (libraryOpen) return [...TABS, LIBRARY_TAB];
  if (labelOpen) return [...TABS, LABEL_TAB];
  if (measureOpen) return [...TABS, MEASURE_TAB];
  return analysisOpen ? [...TABS, ANALYSIS_TAB] : TABS;
}

export const DEFAULT_TAB: TabId = "modify";

/** The tab the ribbon shows: the selected one while it is on show, else the default (Repair after repair mode). */
export function shownTab(selected: TabId, tabs: readonly RibbonTab[]): TabId {
  return tabs.some((t) => t.id === selected) ? selected : DEFAULT_TAB;
}

// ---------------------------------------------------------------------------------------------
// Commands that appear in more than one place (ribbon, Cuts panel, context menu). Every placement
// shares the same function, defined once here.

const cut = openCutDialog;
const boolean = openBooleanMode;
const splitShells = () => void splitShellsCommand();
const generateShell = openGenerateShell;
const SHELL_HINT = "Make a hollow, slimmer or fuller version of the selected part as a new part.";
const mergeParts = () => void mergePartsCommand();

export { IS_MAC };

/** Shortcut label with the platform's modifier key: Ctrl+Z, or Cmd+Z on macOS. */
const mod = (keys: string) => `${IS_MAC ? "Cmd" : "Ctrl"}+${keys}`;

const store = () => useStore.getState();
const ui = () => useUi.getState();

/**
 * New project and Open leave repair mode first, asking about its changes; they stop when it stays open. The window
 * stays live while confirmDiscard waits for a save, so New asks once more at the end about a repair started
 * meanwhile (Open does the same in src/io/files.ts before the file replaces the project).
 */
async function newProject() {
  if ((await closeModes()) && (await confirmDiscard()) && (await closeModes())) store().newProject();
}

async function openProject() {
  if (await closeModes()) await openFile();
}

/** Leaves repair mode, Boolean operations and the Part library, asking about changes; false when one stays open. */
async function closeModes(): Promise<boolean> {
  if (!((await closeRepair()) && (await closeBooleanMode()))) return false;
  closePartLibrary();
  closeMeasure();
  closeAnalysisView();
  closeCollisions();
  return true;
}

/**
 * In repair mode Undo and Redo step through the repair session's own history; the main one waits until it ends. In
 * Boolean operations they wait too: they would bring back or take away parts in the lists.
 */
function undo() {
  if (isBooleanOpen()) {
    toast(CLOSE_BOOLEAN_FIRST);
    return;
  }
  if (isLabelOpen()) {
    labelUndo();
    return;
  }
  if (isRepairing()) {
    if (canUndoRepair()) undoRepair();
    else toast("Nothing to undo.");
    return;
  }
  if (canUndo(store())) store().undo();
  else toast("Nothing to undo.");
}

function copyParts() {
  const n = store().copySelected();
  toast(n === 0 ? SELECT_PART_FIRST : n === 1 ? "Copied 1 part." : `Copied ${n} parts.`);
}

async function pasteParts() {
  const ids = store().pasteClipboard();
  if (ids.length === 0) {
    toast("Nothing to paste. Copy a part first.");
    return;
  }
  await ensureMeshChecks(ids);
}

function redo() {
  if (isBooleanOpen()) {
    toast(CLOSE_BOOLEAN_FIRST);
    return;
  }
  if (isLabelOpen()) {
    labelRedo();
    return;
  }
  if (isRepairing()) {
    if (canRedoRepair()) redoRepair();
    else toast("Nothing to redo.");
    return;
  }
  if (canRedo(store())) store().redo();
  else toast("Nothing to redo.");
}

/** How many steps the Undo and Redo arrows list. */
export const HISTORY_ROWS = 15;

/**
 * The labels of the steps Undo (or Redo) would take, next one first: repair mode's own steps while it is open, none
 * in Boolean operations (they wait until it closes), else the project's.
 */
export function historyLabels(kind: "undo" | "redo"): string[] {
  if (isBooleanOpen()) return [];
  if (isLabelOpen()) return labelHistory(kind);
  const session = useRepair.getState().session;
  if (session) return kind === "undo" ? session.past.map((p) => p.label).reverse() : session.future.map((p) => p.label);
  const history = store().history;
  return kind === "undo" ? history.past.map((e) => e.label).reverse() : history.future.map((e) => e.label);
}

/** Undo or Redo `count` steps at once, as one click on a row of the arrow's list. */
function repeatHistory(kind: "undo" | "redo", count: number) {
  const current = () => (isRepairing() ? useRepair.getState().session : isLabelOpen() ? labelHistory(kind).length : store().history);
  for (let i = 0; i < count; i++) {
    const before = current();
    if (kind === "undo") undo();
    else redo();
    // Stop at a step that did not go through (a run in progress, nothing left).
    if (current() === before) break;
  }
}

/** The Undo or Redo arrow's rows: one per step, the newest first; a row takes back (or brings back) it and the ones above. */
export function historyMenu(kind: "undo" | "redo"): Feature[] {
  const labels = historyLabels(kind).slice(0, HISTORY_ROWS);
  const verb = kind === "undo" ? "Undo" : "Redo";
  const icon = kind === "undo" ? Undo2 : Redo2;
  if (labels.length === 0) {
    const label = isBooleanOpen() ? CLOSE_BOOLEAN_FIRST : `Nothing to ${kind}.`;
    return [{ id: `home.${kind}.none`, tab: "home", group: "History", label, icon, status: "ready", run: () => toast(label) }];
  }
  return labels.map((label, i) => ({
    id: `home.${kind}.step-${i + 1}`,
    tab: "home",
    group: "History",
    label: `${verb} ${label}`,
    icon,
    status: "ready",
    hint: i === 0 ? `${verb} this change.` : `${verb} this change and the ${i === 1 ? "one" : i} ${kind === "undo" ? "after" : "before"} it.`,
    run: () => repeatHistory(kind, i + 1),
  }));
}

/** Home › Platform statistics: the Platform overview dialog, which measures the parts in Rust as it opens. */
function openPlatformStats() {
  if (browserOnly("Platform statistics")) return;
  if (store().parts.length === 0) toast("There are no parts on the platform.");
  else ui().openDialog({ kind: "platform-stats" });
}

const move = () => startTransform("translate");
const rotate = () => startTransform("rotate");
const scale = () => startTransform("scale");

const zoomTo = (target: ZoomTarget) => () => viewport.zoomTo(target);

/** The status bar's tool while Zoom to › Area or Point waits for its drag or click. */
export const VIEW_TOOLS: Record<ViewTool, Tool> = {
  "zoom-area": { name: "Zoom to area", hint: "Drag a rectangle in the view to zoom into it. Esc cancels." },
  "zoom-point": { name: "Zoom to point", hint: "Click a point on a part: the view centres on it and turns around it from now on. Esc cancels." },
  "lay-flat": { name: "Lay flat", hint: "Click a face of a part: the part turns so that face lies on the platform. Esc cancels." },
};

/** Zoom to › Area and Point: the next drag or click in the view does it (src/viewport/picking.ts); a second click stops. */
const viewTool = (tool: ViewTool) => () => ui().setViewTool(ui().viewTool === tool ? null : tool);

const viewFrom = (preset: ViewPreset) => () => viewport.setViewPreset(preset);
const setView = (patch: Partial<ViewSettings>) => () => store().setView(patch);

const flipPlatform = () => store().setView({ showPlatform: !store().view.showPlatform });
const flipAxes = () => store().setView({ showAxes: !store().view.showAxes });
const flipRulers = () => store().setView({ showRulers: !store().view.showRulers });

/**
 * While Always show gizmo is on, a picked move and rotate tool looks the same as Select, so turning the setting off
 * drops that tool too and the gizmo hides at once. Scale stays; turning the setting on changes no tool.
 */
function flipGizmo() {
  const on = !store().view.alwaysShowGizmo;
  store().setView({ alwaysShowGizmo: on });
  if (!on && store().gizmoMode === "combined") store().setGizmoMode(null);
}

function selectedParts(s: AppState): Part[] {
  const ids = new Set(s.selection);
  return s.parts.filter((p) => ids.has(p.id));
}

function selectionCentre(s: AppState): Vec3 | null {
  const b = unionBounds(selectedParts(s).map((p) => boundsOf(p.mesh, p.matrix)));
  if (!b) return null;
  return [(b.min[0] + b.max[0]) / 2, (b.min[1] + b.max[1]) / 2, (b.min[2] + b.max[2]) / 2];
}

function duplicateSelection() {
  store().duplicateParts(store().selection);
}

function packingSettings() {
  const ids = store().selection;
  if (ids.length === 0) toast(SELECT_PART_FIRST);
  else ui().openDialog({ kind: "packing-settings", ids: [...ids] });
}

function renameSelection() {
  const sel = store().selection;
  if (sel.length > 0) ui().openDialog({ kind: "rename", partId: sel[sel.length - 1] });
}

function deleteSelection() {
  const locked = store().deleteParts(store().selection);
  if (locked > 0) toast(`${locked} locked part(s) were not deleted.`, "warn");
}

function centreViewHere() {
  // The context menu stores the world point under the cursor; from elsewhere, use the selection's centre.
  const point = ui().contextMenu?.point ?? selectionCentre(store());
  if (point) viewport.centreOn(point);
}

function toggleSelectedVisible() {
  const s = store();
  const sel = selectedParts(s);
  if (sel.length > 0) s.setPartsVisible(s.selection, !sel.every((p) => p.visible));
}

function toggleSelectedTransparent() {
  const s = store();
  const sel = selectedParts(s);
  if (sel.length > 0) s.setPartsTransparent(s.selection, !sel.every((p) => p.transparent));
}

function hideUnselected() {
  const s = store();
  const keep = new Set(s.selection);
  s.setPartsVisible(s.parts.filter((p) => !keep.has(p.id)).map((p) => p.id), false);
}

function showAll() {
  const s = store();
  s.patchParts(
    s.parts.map((p) => p.id),
    { visible: true, transparent: false },
    "Show all",
  );
}

// ---------------------------------------------------------------------------------------------
// Repair mode. src/repair/session.ts does the work; the Repair tab and the repair context menu share these.

const REPAIR_HINT = "Repair the selected part: find and fix holes, flipped triangles and other problems.";

/** Home › Repair, the quick strip and the part context menu. A running Cut or Split would replace the part under it. */
function repairPart() {
  if (meshOperationRunning()) toast(WAIT_MESSAGE);
  else startRepair();
}

const repair = () => useRepair.getState();

const pickTool = (tool: SelectTool) => () => repair().setSelectTool(tool);
const toolIs = (tool: SelectTool) => (r: RepairState) => r.selectTool === tool && r.editTool === null;
/** Move nodes and Cut surfaces are pressed while they hold the viewport's clicks. */
const editIs = (tool: EditTool) => (r: RepairState) => r.session !== null && r.editTool === tool;
const repairAction = (id: ActionId) => () => void runAction(id);

const removeTriangles = repairAction("remove_selected_triangles");
const flipTriangles = repairAction("flip_selected_triangles");
const extractCopy = () => void extractSelection(false);
const extractSeparate = () => void extractSelection(true);

function pickScript() {
  if (isRepairing()) ui().openDialog({ kind: "repair-script-picker" });
}

function zoomToBox(box: Bounds | null) {
  if (box) viewport.zoomToBounds(box);
}

/** Zoom to › Part (A): the working copy. */
const zoomToPart = () => zoomToBox(workingCopyBounds());
/** Zoom to › Selected (F): the selected triangles, or the working copy when none is selected. */
const zoomToTriangles = () => zoomToBox(selectedBounds() ?? workingCopyBounds());

/** "Add this triangle / surface / shell to selection": that region around the triangle the context menu opened on. */
const addUnderCursor = (tool: SelectTool) => () => {
  const triangle = ui().contextMenu?.triangle;
  if (typeof triangle === "number") selectAt(triangle, "add", tool);
};

const ORBIT_HINT = navigationHint(IS_MAC);
/** Box selection in repair mode (src/viewport/picking.ts). */
const BOX_HINT = `A box dragged from empty space selects the triangles facing you (Shift adds, ${toggleKey(IS_MAC)} toggles).`;

/** The status bar's tool in repair mode: the select tool picked under Repair › Select. */
export const REPAIR_TOOLS: Record<SelectTool, Tool> = {
  triangle: { name: "Repair: Triangle", hint: `Click a triangle to select it, Shift+click to add or remove it. ${BOX_HINT} ${ORBIT_HINT}` },
  surface: {
    name: "Repair: Surface",
    hint: `Click a triangle to select its surface up to the View tab's tolerance angle, Shift+click to add or remove it. ${BOX_HINT} ${ORBIT_HINT}`,
  },
  shell: { name: "Repair: Shell", hint: `Click a triangle to select its shell, Shift+click to add or remove it. ${BOX_HINT} ${ORBIT_HINT}` },
  area: {
    name: "Repair: Area",
    hint: `Drag a rectangle anywhere to select the triangles facing you (Shift adds, ${toggleKey(IS_MAC)} toggles). ${ORBIT_HINT}`,
  },
  brush: {
    name: "Repair: Brush",
    hint: `Drag across the working copy to select the triangles under the pointer (Shift adds, ${toggleKey(IS_MAC)} toggles). ${ORBIT_HINT}`,
  },
};

/** The status bar's tool while Move nodes or Cut surfaces holds the viewport's clicks. */
export const EDIT_TOOLS: Record<EditTool, Tool> = {
  "move-nodes": {
    name: "Repair: Move nodes",
    hint: `Point at the surface to find the nearest point; drag it to move it in the view's plane, Esc to stop. ${ORBIT_HINT}`,
  },
  cut: {
    name: "Repair: Cut surfaces",
    hint: `Click points on the surface; the line between them follows it. Execute cut in the panel cuts the surface open along it. ${ORBIT_HINT}`,
  },
};

// ---------------------------------------------------------------------------------------------
// Boolean operations. src/boolean/session.ts does the work.

const pickBooleanTool = (tool: BooleanTool) => () => useBoolean.getState().setTool(tool);
const booleanToolIs = (tool: BooleanTool) => (b: BooleanState) => b.tool === tool;

/** The status bar's tool in Boolean operations. */
export const BOOLEAN_TOOLS: Record<BooleanTool, Tool> = {
  select: { name: "Boolean: Select", hint: `Click a part to select it in its list. ${ORBIT_HINT}` },
  "zoom-area": { name: "Boolean: Zoom to area", hint: `Drag a rectangle around the area to zoom into. ${ORBIT_HINT}` },
};

/** Why a "this triangle" entry cannot run: the repair context menu did not open on a triangle. */
export const PICK_TRIANGLE_FIRST = "Right-click a triangle to use this.";

// ---------------------------------------------------------------------------------------------
// Label generation. src/label/session.ts does the work.

const LABEL_HINTS: Record<LabelKind, string> = {
  text: "Raise text from the selected part's surface or sink it in. Click on the part where it goes.",
  image: "Raise a picture's dark parts from the selected part's surface or sink them in.",
  tag: "Add a tag with text that stands out from the selected part on a thin bridge, to break off after printing.",
  list: "Make one copy of the selected part per list entry, each labelled with its entry: running numbers or a CSV column.",
};

/** Generate label › a kind: label generation for the selected part, placing that kind first. */
const label = (kind: LabelKind) => () => openLabelMode(kind);
const kindIs = (kind: LabelKind) => (l: LabelState) => l.session?.kind === kind;

// ---------------------------------------------------------------------------------------------
// Measure mode. src/measure/session.ts does the work; the tools come from src/measure/tools.ts.

const MEASURE_HINT = "Measure distances, angles, radii and wall thickness on the parts.";

/** Home › New analysis ▸ a kind, and the Analyze tab's buttons: the same command. */
const analysis = (kind: AnalysisKind) => () => newAnalysis(kind);

const ANALYSIS_HINTS: Record<AnalysisKind, string> = {
  standard: "Position, size, volume, mesh counts, damage and mesh quality of the selected part.",
  gravity: "Where the centre of gravity of the selected part is, marked in the view.",
  shadow: "The area the selected part covers seen from straight above. Needs a closed, sound part.",
  support: "Estimate the support the selected part needs below its overhangs.",
  skin: "Colour the nearly flat surfaces of the selected part: facing up green, facing down red, with their areas.",
  wall: "Colour the selected part by how thick its walls are, and test whether too much of it is too thin.",
  compare: "Colour two parts by how far each surface is from the other's, with the largest and average distances.",
  removability: "Check that every body of the selected part can be lifted straight up without hitting another, and show the trapped ones.",
};

const COLLISIONS_HINT = "Check which parts cross or come too close, and keep checking as they move.";

/** Part analysis › Zoom to part. */
function zoomToAnalysisPart() {
  const found = shownAnalysis();
  if (found) zoomToBox(boundsOf(found.part.mesh, found.part.matrix));
}

const ORIENT_HINT = "Find the best way for the selected part to stand: least support area, lowest height or smallest footprint.";

const DUPLICATE_HINTS: Record<DuplicateMode, string> = {
  "in-place": "Make copies of the selected parts right on top of them.",
  fill: "Fill the platform with copies of the selected parts, as many as fit.",
  grid: "Make copies of the selected parts in rows along X, Y and Z.",
};

const duplicate = (mode: DuplicateMode) => () => openDuplicate(mode);

const PACK_HINT = "Arrange the parts on the platform in as little room as possible, by one of four methods.";

const PACK_HINTS: Record<PackMethod, string> = {
  "bounding-box": "Pack the parts' boxes, unturned: the fastest, least tight.",
  scanline2d: "Lay the parts out in one layer on the platform by their real outline, turned about Z.",
  scanline3d: "Stack the parts by their real shape, each where its top ends lowest.",
  "monte-carlo": "Shake the parts into the gaps, seeded, keeping the packing height low; follows each part's priority and rotation.",
};

/** Home › Pack's entries: [id, method, icon], in the reference's order. */
const PACK_ENTRIES: [string, string, LucideIcon][] = [
  ["home.pack-box", "bounding-box", Box],
  ["home.pack-2d", "scanline2d", LayoutDashboard],
  ["home.pack-3d", "scanline3d", Boxes],
  ["home.pack-monte-carlo", "monte-carlo", Dices],
  ["home.pack-gravity", "gravity", ArrowDownToLine],
  ["home.pack-true-shape-2d", "true-shape-2d", Shapes],
  ["home.pack-true-shape-3d", "true-shape-3d", Cuboid],
];
const align = (how: AlignHow) => () => void alignParts(how);

/** Align › Align parts to each other: [how, label, icon, hint]. Front is the platform's near edge. */
const ALIGN_TO_EACH_OTHER: [AlignHow, string, LucideIcon, string][] = [
  ["left", "Left", AlignStartVertical, "Line up the parts' left sides (smallest X)."],
  ["right", "Right", AlignEndVertical, "Line up the parts' right sides (largest X)."],
  ["front", "Front", AlignStartHorizontal, "Line up the parts' front sides (smallest Y)."],
  ["back", "Back", AlignEndHorizontal, "Line up the parts' back sides (largest Y)."],
  ["bottom", "Bottom", ArrowDownToLine, "Line up the parts' bottoms (smallest Z)."],
  ["top", "Top", ArrowUpToLine, "Line up the parts' tops (largest Z)."],
  ["centre-x", "Centre X", AlignCenterVertical, "Line up the parts' centres in X."],
  ["centre-y", "Centre Y", AlignCenterHorizontal, "Line up the parts' centres in Y."],
  ["centre-z", "Centre Z", AlignVerticalJustifyCenter, "Line up the parts' centres in Z."],
];

/** Align › Align to the platform. */
const ALIGN_TO_PLATFORM: [AlignHow, string, LucideIcon, string][] = [
  ["platform-centre", "Centre on platform", Target, "Move the selection as a group to the middle of the platform in X and Y."],
  ["origin", "Move to origin", ArrowDownLeft, "Move the selection as a group so its box starts at the platform's origin, on the platform."],
  ["drop", "Drop onto platform", ArrowDownToLine, "Move each selected part down or up so its lowest point is on the platform."],
];

/** The tab's six dropdowns; Wall thickness and 3-point angle are buttons of their own. */
const MENU_GROUPS: ToolGroup[] = ["point", "edge", "circle", "circle3", "sphere4", "surface"];

const measureToolIs = (id: string) => (m: MeasureState) => m.session?.tool === id;
const groupIs = (group: ToolGroup) => (m: MeasureState) => TOOLS.some((t) => t.group === group && t.id === m.session?.tool);

/** The tools of a group, in a dropdown (`parent`) or as buttons of their own. */
function toolEntries(group: ToolGroup, parent?: string): Entry[] {
  return TOOLS.filter((t) => t.group === group).map((t) => ({
    id: `measure.${t.id}`,
    label: t.label,
    icon: toolIcon(t),
    status: "ready",
    run: () => pickMeasureTool(t.id),
    ...(parent ? { parent } : { size: "large" as const }),
    measureActive: measureToolIs(t.id),
    hint: describe(t).join(" "),
  }));
}

/** The status bar's tool in Measure mode: the tool picked, and what to click next. */
export function measureStatus(m: MeasureState): Tool | null {
  if (!m.session) return null;
  const tool = currentTool();
  return { name: `Measure: ${tool.label}`, hint: `${nextClickText(tool, m.session.picks.length, m.session.points.length)} Esc starts it again. ${ORBIT_HINT}` };
}

// ---------------------------------------------------------------------------------------------
// Export. Every entry point (ribbon, File menu, right-click, Ctrl+E) runs the same command: the options step, then
// the system's save dialog listing every format tricky3D writes (src/io/files.ts exportParts). The formats still to
// come are listed under File › More export formats.

const EXPORT_HINT = "Save the selected parts, or every visible part, as STL, 3MF, OBJ, PLY, AMF or OFF.";
const SAVE_HINT = "Save the whole project as a tricky3D project file (.3mf). For an STL or another mesh file, use Export.";

const SOON_EXPORT_FORMATS: [key: string, label: string][] = [
  ["color-stl", "Colour STL"],
  ["gts", "GTS"],
  ["x3d", "X3D"],
  ["3ds", "3DS"],
  ["vrml", "VRML"],
  ["binvox", "BINVOX"],
  ["compressed", "Compressed mesh"],
  ["formlabs", "Formlabs"],
  ["slice", "Slice"],
];

function soonExportFormats(parent: string): Feature[] {
  return SOON_EXPORT_FORMATS.map(([key, label]) => ({
    id: `${parent}.${key}`,
    tab: "file",
    group: "More formats",
    label,
    icon: key === "slice" ? Layers : FileAxis3d,
    status: "soon",
    parent,
  }));
}

// ---------------------------------------------------------------------------------------------
// The registry. Array order is display order; a tab shows its groups in order of first appearance.
// Inside a menu, `group` names the section, and the menu draws a separator where it changes.

type Entry = Omit<Feature, "tab" | "group"> & { group?: string };

function group(tab: TabId, name: string, entries: Entry[]): Feature[] {
  return entries.map((e) => ({ ...e, tab, group: e.group ?? name }));
}

export const FEATURES: Feature[] = [
  // File menu ---------------------------------------------------------------------------------
  ...group("file", "Project", [
    { id: "file.new", label: "New project", icon: FilePlus, status: "ready", run: newProject, hint: "Start over with an empty platform." },
    { id: "file.open", label: "Open…", icon: FolderOpen, status: "ready", run: openProject, shortcut: mod("O"), hint: "Open a project, or a mesh file as a new project." },
    { id: "file.save", label: "Save project (.3mf)", icon: Save, status: "ready", run: saveProject, shortcut: mod("S"), hint: SAVE_HINT },
    { id: "file.save-as", label: "Save project as (.3mf)…", icon: SaveAll, status: "ready", run: saveProjectAs, hint: "Save the project as a tricky3D project file (.3mf) under a new name." },
    { id: "file.recent", label: "Recent files", icon: FileClock, status: "soon" },
    { id: "file.autosave", label: "Autosave", icon: Timer, status: "soon" },
  ]),
  ...group("file", "Part", [
    { id: "file.import", label: "Import part…", icon: Import, status: "ready", run: importPart, hint: "Add parts from mesh files." },
    { id: "file.import-split", label: "Import file as separate parts", icon: Split, status: "soon" },
    { id: "file.import-slices", label: "Open slice file", icon: FileStack, status: "soon" },
    { id: "file.preview", label: "Preview a file", icon: FileSearch, status: "soon" },
    { id: "file.import-options", label: "Import options", icon: FileSliders, status: "soon" },
    { id: "file.formats", label: "Supported formats…", icon: FileAxis3d, status: "ready", run: () => ui().openDialog({ kind: "formats" }), hint: "File formats tricky3D can read and write." },
    { id: "file.export", label: "Export part…", icon: FileOutput, status: "ready", run: exportSelection, shortcut: mod("E"), hint: EXPORT_HINT },
    { id: "file.export-more", label: "More export formats", icon: FileAxis3d, status: "soon" },
    { id: "file.batch-export", label: "Export many parts at once", icon: FolderOutput, status: "soon" },
    { id: "file.part-library", label: "Add to part library", icon: LibraryBig, status: "soon" },
    { id: "file.3mf-colours", label: "3MF colours and materials", icon: SwatchBook, status: "soon" },
  ]),
  ...soonExportFormats("file.export-more"),
  ...group("file", "App", [
    { id: "file.settings", label: "Settings…", icon: Settings, status: "ready", run: () => ui().openDialog({ kind: "settings" }), hint: "Change how tricky3D looks and behaves." },
    { id: "file.about", label: "About tricky3D", icon: Info, status: "ready", run: () => ui().openDialog({ kind: "about" }), hint: "Version, licence and credits." },
  ]),

  // Home: the reference tool's layout, left to right. The repair tools that used to live here are in the Repair
  // tab and the repair right-click menu, where the reference has them.
  ...group("home", "Edit", [
    { id: "home.save", label: "Save project (.3mf)", icon: Save, status: "ready", run: saveProject, shortcut: mod("S"), size: "large", hint: SAVE_HINT },
    { id: "home.export", label: "Export", icon: FileOutput, status: "ready", run: exportSelection, shortcut: mod("E"), size: "large", hint: EXPORT_HINT },
    { id: "home.undo", label: "Undo", icon: Undo2, status: "ready", run: undo, dropdown: () => historyMenu("undo"), shortcut: mod("Z"), hint: "Undo the last change. The arrow lists the recent changes, to undo several at once." },
    { id: "home.redo", label: "Redo", icon: Redo2, status: "ready", run: redo, dropdown: () => historyMenu("redo"), shortcut: mod("Shift+Z"), hint: "Redo the change you undid. The arrow lists the changes you can redo." },
  ]),
  ...group("home", "Add parts", [
    { id: "home.my-machines", label: "My machines", icon: Printer, status: "soon", size: "large" },
    { id: "home.add-part", label: "Add part", icon: PackagePlus, status: "ready", size: "large", hint: "Import a mesh file or add a test cube." },
    { id: "home.import", label: "Import from file…", icon: Import, status: "ready", run: importPart, parent: "home.add-part", hint: "Add parts from mesh files." },
    { id: "home.add-test-cube", label: "Add test cube", icon: Box, status: "ready", run: () => store().addTestCube(), parent: "home.add-part", hint: "Add a 20 mm cube to the platform." },
    { id: "home.part-library", label: "Part library", icon: Library, status: "ready", run: () => void openPartLibrary(), size: "large", hint: "Add a shape, grid, screw, nut, gear, QR code, mathematical solid or test part, made to the values you set." },
    { id: "home.platform-statistics", label: "Platform statistics", icon: Sheet, status: "ready", run: openPlatformStats, size: "large", outsideRepair: true, hint: "List every part with its size, volume and mesh counts, and the platform's packing density." },
  ]),
  ...group("home", "Prepare", [
    { id: "home.repair", label: "Repair part", icon: Wrench, status: "ready", run: repairPart, size: "large", outsideRepair: true, hint: REPAIR_HINT },
    { id: "home.run-script", label: "Run script", icon: ScrollText, status: "ready", size: "large", hint: "Run a repair script on the selected part." },
    { id: "home.run-repair-script", label: "Run repair script", icon: Play, status: "ready", run: () => void runRepairScript(), parent: "home.run-script", hint: "Pick a repair script and run it on the selected part." },
    { id: "home.run-support-script", label: "Run support script", icon: Construction, status: "soon", parent: "home.run-script" },
    { id: "home.run-lua-script", label: "Run Lua script", icon: FileCode, status: "soon", parent: "home.run-script" },
    { id: "home.quick-access", label: "Quick access", icon: Zap, status: "soon", parent: "home.run-script", group: "Quick access" },
    { id: "home.quick-access-recent", label: "Recently run scripts", icon: History, status: "soon", parent: "home.quick-access" },
  ]),
  ...group("home", "Analyze", [
    { id: "home.new-analysis", label: "New analysis", icon: ChartArea, status: "ready", size: "large", hint: "Analyze the selected part. Its results go under it in the parts tree." },
    { id: "home.analysis-skin", label: "Up- and downskins", icon: ArrowDownToDot, status: "ready", run: analysis("skin"), parent: "home.new-analysis", outsideRepair: true, hint: ANALYSIS_HINTS.skin },
    { id: "home.analysis-gravity", label: "Centre of gravity", icon: Crosshair, status: "ready", run: analysis("gravity"), parent: "home.new-analysis", outsideRepair: true, hint: ANALYSIS_HINTS.gravity },
    { id: "home.analysis-standard", label: "Standard analysis", icon: ClipboardList, status: "ready", run: analysis("standard"), parent: "home.new-analysis", outsideRepair: true, hint: ANALYSIS_HINTS.standard },
    { id: "home.analysis-shadow", label: "Shadow area", icon: SunDim, status: "ready", run: analysis("shadow"), parent: "home.new-analysis", outsideRepair: true, hint: ANALYSIS_HINTS.shadow },
    { id: "home.analysis-support-volume", label: "Support volume", icon: Construction, status: "ready", run: analysis("support"), parent: "home.new-analysis", outsideRepair: true, hint: ANALYSIS_HINTS.support },
    { id: "home.analysis-wall-thickness", label: "Wall thickness", icon: RulerDimensionLine, status: "ready", run: analysis("wall"), parent: "home.new-analysis", outsideRepair: true, hint: ANALYSIS_HINTS.wall },
    { id: "home.measure", label: "Measure", icon: Ruler, status: "ready", run: () => openMeasure(), size: "large", hint: MEASURE_HINT },
  ]),
  ...group("home", "Modify", [
    { id: "home.boolean", label: "Boolean", icon: SquaresUnite, status: "ready", run: boolean, size: "large", outsideRepair: true, hint: "Unite the selected parts, subtract some from the others, or keep what they share." },
    { id: "home.generate-shell", label: "Generate shell", icon: Cylinder, status: "ready", run: generateShell, outsideRepair: true, hint: SHELL_HINT },
    { id: "home.generate-label", label: "Generate label", icon: Tag, status: "ready" },
    { id: "home.label-text", label: "Text label", icon: Type, status: "ready", run: label("text"), parent: "home.generate-label", outsideRepair: true, hint: LABEL_HINTS.text },
    { id: "home.label-image", label: "Image label", icon: Stamp, status: "ready", run: label("image"), parent: "home.generate-label", outsideRepair: true, hint: LABEL_HINTS.image },
    { id: "home.label-tag", label: "Tag", icon: Tags, status: "ready", run: label("tag"), parent: "home.generate-label", outsideRepair: true, hint: LABEL_HINTS.tag },
    { id: "home.label-list", label: "Numbered labels from a list", icon: ListOrdered, status: "ready", run: label("list"), parent: "home.generate-label", outsideRepair: true, hint: LABEL_HINTS.list },
  ]),
  ...group("home", "Arrange", [
    { id: "home.scale", label: "Scale", icon: Scale3d, status: "ready", run: scale, size: "large", outsideRepair: true, hint: "Resize the selected parts by a factor or to a size." },
    { id: "home.rotate", label: "Rotate", icon: Rotate3d, status: "ready", run: rotate, size: "large", outsideRepair: true, hint: "Rotate the selected parts." },
    { id: "home.move", label: "Move", icon: Move, status: "ready", run: move, size: "large", outsideRepair: true, hint: "Move the selected parts." },
    { id: "home.orient", label: "Orient", icon: Compass, status: "ready", run: openOrient, size: "large", outsideRepair: true, hint: ORIENT_HINT },
    { id: "home.duplicate", label: "Duplicate", icon: CopyPlus, status: "ready", size: "large", hint: "Copy the selected parts: in place, filling the platform, or in a grid." },
    { id: "home.duplicate-count", label: "Duplicate in place", icon: Copy, status: "ready", run: duplicate("in-place"), parent: "home.duplicate", outsideRepair: true, hint: DUPLICATE_HINTS["in-place"] },
    { id: "home.duplicate-fill", label: "Fill the platform", icon: Grid3x2, status: "ready", run: duplicate("fill"), parent: "home.duplicate", outsideRepair: true, hint: DUPLICATE_HINTS.fill },
    { id: "home.duplicate-grid", label: "Duplicate in a grid", icon: Rows3, status: "ready", run: duplicate("grid"), parent: "home.duplicate", outsideRepair: true, hint: DUPLICATE_HINTS.grid },
    { id: "home.pack", label: "Pack", icon: Blocks, status: "ready", size: "large", hint: PACK_HINT },
    ...PACK_ENTRIES.map(([id, method, icon]): Entry => {
      const m = PACK_METHODS.find((p) => p.value === method);
      return m?.soon
        ? { id, label: m.label, icon, status: "soon", parent: "home.pack" }
        : { id, label: m?.label ?? id, icon, status: "ready", run: () => openPack(method as PackMethod), parent: "home.pack", outsideRepair: true, hint: PACK_HINTS[method as PackMethod] };
    }),
  ]),
  ...group("home", "Support", [
    { id: "home.generate-supports", label: "Generate supports", icon: Construction, status: "soon", size: "large" },
    { id: "home.supports-script", label: "Run support script", icon: Play, status: "soon", parent: "home.generate-supports" },
    { id: "home.supports-manual", label: "Edit supports by hand", icon: PenTool, status: "soon", parent: "home.generate-supports" },
    { id: "home.manage-supports", label: "Manage supports", icon: Settings2, status: "soon", size: "large" },
    { id: "home.supports-remove", label: "Remove supports", icon: Trash, status: "soon", parent: "home.manage-supports" },
    { id: "home.supports-export", label: "Export supports", icon: FileOutput, status: "soon", parent: "home.manage-supports" },
  ]),
  ...group("home", "Slice parts", [
    { id: "home.slice-parts", label: "Slice parts", icon: Layers, status: "soon", size: "large" },
  ]),
  ...group("home", "Zoom to", [
    { id: "home.zoom-everything", label: "Everything", icon: Fullscreen, status: "ready", run: zoomTo("everything"), hint: "Fit the platform and all parts in the view." },
    { id: "home.zoom-selected", label: "Selected", icon: Focus, status: "ready", run: zoomTo("selected"), shortcut: "F", hint: "Fit the selected parts in the view." },
    { id: "home.zoom-platform", label: "Platform", icon: Scan, status: "ready", run: zoomTo("platform"), hint: "Fit the build platform in the view." },
    { id: "home.zoom-area", label: "Area", icon: ScanLine, status: "ready", run: viewTool("zoom-area"), viewTool: "zoom-area", hint: "Drag a rectangle in the view to zoom into it." },
    { id: "home.zoom-parts", label: "All parts", icon: ScanSearch, status: "ready", run: zoomTo("parts"), shortcut: "A", hint: "Fit all parts in the view." },
    { id: "home.zoom-point", label: "Point", icon: LocateFixed, status: "ready", run: viewTool("zoom-point"), viewTool: "zoom-point", hint: "Click a point on a part: the view centres on it and turns around it from now on." },
  ]),
  ...group("home", "Utilities", [
    { id: "home.open-utility", label: "Open Utility", icon: AppWindow, status: "soon", size: "large" },
    { id: "home.utility-simulation", label: "Simulation", icon: Atom, status: "soon", parent: "home.open-utility" },
    { id: "home.utility-lattice", label: "Lattices", icon: Network, status: "soon", parent: "home.open-utility" },
  ]),

  // Analyze -----------------------------------------------------------------------------------
  ...group("analyze", "Edit", [
    { id: "analyze.save", label: "Save project (.3mf)", icon: Save, status: "ready", run: saveProject, shortcut: mod("S"), hint: SAVE_HINT },
    { id: "analyze.undo", label: "Undo", icon: Undo2, status: "ready", run: undo, dropdown: () => historyMenu("undo"), shortcut: mod("Z"), hint: "Undo the last change. The arrow lists the recent changes, to undo several at once." },
    { id: "analyze.redo", label: "Redo", icon: Redo2, status: "ready", run: redo, dropdown: () => historyMenu("redo"), shortcut: mod("Shift+Z"), hint: "Redo the change you undid. The arrow lists the changes you can redo." },
  ]),
  ...group("analyze", "Overview", [
    { id: "analyze.platform-statistics", label: "Platform statistics", icon: Sheet, status: "ready", run: openPlatformStats, size: "large", outsideRepair: true, hint: "List every part with its size, volume and mesh counts, and the platform's packing density." },
    { id: "analyze.information", label: "Information", icon: Info, status: "ready", run: () => ui().focusSection("information"), outsideRepair: true, hint: "Show size, volume and mesh counts in the right panel." },
  ]),
  ...group("analyze", "Analysis", [
    { id: "analyze.overhangs", label: "Up- and downskins", icon: ArrowDownToDot, status: "ready", run: analysis("skin"), size: "large", outsideRepair: true, hint: ANALYSIS_HINTS.skin },
    { id: "analyze.centre-of-mass", label: "Centre of gravity", icon: Crosshair, status: "ready", run: analysis("gravity"), size: "large", outsideRepair: true, hint: ANALYSIS_HINTS.gravity },
    { id: "analyze.standard", label: "Standard analysis", icon: ClipboardList, status: "ready", run: analysis("standard"), size: "large", outsideRepair: true, hint: ANALYSIS_HINTS.standard },
    { id: "analyze.footprint", label: "Shadow area", icon: SunDim, status: "ready", run: analysis("shadow"), size: "large", outsideRepair: true, hint: ANALYSIS_HINTS.shadow },
    { id: "analyze.support-volume", label: "Support volume", icon: Construction, status: "ready", run: analysis("support"), size: "large", outsideRepair: true, hint: ANALYSIS_HINTS.support },
    { id: "analyze.wall-thickness-map", label: "Wall thickness", icon: RulerDimensionLine, status: "ready", run: analysis("wall"), size: "large", outsideRepair: true, hint: ANALYSIS_HINTS.wall },
    { id: "analyze.measure", label: "Measure", icon: Ruler, status: "ready", run: () => openMeasure(), size: "large", hint: MEASURE_HINT },
    { id: "analyze.compare", label: "Compare", icon: Diff, status: "ready", run: analysis("compare"), size: "large", outsideRepair: true, hint: ANALYSIS_HINTS.compare },
    { id: "analyze.layer-area", label: "Area per layer", icon: ChartArea, status: "soon", size: "large" },
    { id: "analyze.wall-thickness", label: "Wall at a point", icon: RulerDimensionLine, status: "ready", run: () => openMeasure("wall"), hint: "Click a point on a part to measure the wall straight through to the other side." },
  ]),
  ...group("analyze", "Collision and interlocking", [
    { id: "analyze.collisions", label: "Collision detection", icon: OctagonAlert, status: "ready", run: openCollisions, size: "large", outsideRepair: true, hint: COLLISIONS_HINT },
    { id: "analyze.interlock", label: "Interlocking test", icon: Link2, status: "ready", run: () => void interlockingTest(), size: "large", outsideRepair: true, hint: "Test whether the selected parts (or all shown parts) can be lifted out upwards one after another without hitting each other." },
    { id: "analyze.lift-out", label: "Z-removability", icon: ArrowUpFromLine, status: "ready", run: analysis("removability"), size: "large", outsideRepair: true, hint: ANALYSIS_HINTS.removability },
  ]),
  ...group("analyze", "Documents", [
    { id: "analyze.report", label: "Report", icon: FileText, status: "soon", size: "large" },
    { id: "analyze.drawing", label: "Dimensioned drawing", icon: DraftingCompass, status: "soon" },
  ]),
  ...group("analyze", "Zoom to", [
    { id: "analyze.zoom-everything", label: "Everything", icon: Fullscreen, status: "ready", run: zoomTo("everything"), hint: "Fit the platform and all parts in the view." },
    { id: "analyze.zoom-platform", label: "Platform", icon: Scan, status: "ready", run: zoomTo("platform"), hint: "Fit the build platform in the view." },
    { id: "analyze.zoom-parts", label: "All parts", icon: ScanSearch, status: "ready", run: zoomTo("parts"), shortcut: "A", hint: "Fit all parts in the view." },
    { id: "analyze.zoom-selected", label: "Selected", icon: Focus, status: "ready", run: zoomTo("selected"), shortcut: "F", hint: "Fit the selected parts in the view." },
    { id: "analyze.zoom-area", label: "Area", icon: ScanLine, status: "ready", run: viewTool("zoom-area"), viewTool: "zoom-area", hint: "Drag a rectangle in the view to zoom into it." },
  ]),

  // Modify ------------------------------------------------------------------------------------
  ...group("modify", "Edit", [
    { id: "modify.save", label: "Save project (.3mf)", icon: Save, status: "ready", run: saveProject, shortcut: mod("S"), hint: SAVE_HINT },
    { id: "modify.export", label: "Export", icon: FileOutput, status: "ready", run: exportSelection, shortcut: mod("E"), hint: EXPORT_HINT },
    { id: "modify.undo", label: "Undo", icon: Undo2, status: "ready", run: undo, shortcut: mod("Z"), hint: "Undo the last change." },
    { id: "modify.redo", label: "Redo", icon: Redo2, status: "ready", run: redo, shortcut: mod("Shift+Z"), hint: "Redo the change you undid." },
  ]),
  ...group("modify", "Split and merge", [
    { id: "modify.cut", label: "Cut", icon: Scissors, status: "ready", run: cut, size: "large", outsideRepair: true, hint: "Cut the selected parts in two along a plane." },
    { id: "modify.boolean", label: "Boolean", icon: SquaresUnite, status: "ready", run: boolean, size: "large", outsideRepair: true, hint: "Unite the selected parts, subtract some from the others, or keep what they share." },
    { id: "modify.edit-shells", label: "Edit shells", icon: Boxes, status: "ready", size: "large", hint: "Split a part into its shells, or merge parts into one." },
    { id: "modify.split-shells", label: "Split shells into parts", icon: Split, status: "ready", run: splitShells, parent: "modify.edit-shells", outsideRepair: true, hint: "Make each separate shell its own part." },
    { id: "modify.merge-parts", label: "Merge parts", icon: Combine, status: "ready", run: mergeParts, parent: "modify.edit-shells", outsideRepair: true, hint: "Put the selected parts into one part without changing the geometry." },
    { id: "modify.stencil-cut", label: "Stencil cut", icon: ScissorsLineDashed, status: "soon" },
  ]),
  ...group("modify", "Mirror and scale", [
    { id: "modify.scale", label: "Scale", icon: Scale3d, status: "ready", run: scale, size: "large", outsideRepair: true, hint: "Resize the selected parts by a factor or to a size." },
    { id: "modify.mirror", label: "Mirror", icon: ArrowLeftRight, status: "soon" },
    { id: "modify.invert-normals", label: "Invert normals", icon: ArrowDownUp, status: "soon" },
    { id: "modify.convert-units", label: "Convert units", icon: Ruler, status: "soon" },
    { id: "modify.shear", label: "Shear", icon: MoveDiagonal2, status: "soon" },
  ]),
  ...group("modify", "Enhance", [
    { id: "modify.tessellation", label: "Tessellation", icon: Hexagon, status: "soon" },
    { id: "modify.z-compensation", label: "Z-compensation", icon: ChevronsDownUp, status: "soon" },
    { id: "modify.manipulate-mesh", label: "Manipulate mesh", icon: SplinePointer, status: "soon" },
    { id: "modify.reduce-triangles", label: "Simplify mesh", icon: Minimize2, status: "soon", parent: "modify.manipulate-mesh" },
    { id: "modify.smooth", label: "Smooth", icon: WavesHorizontal, status: "soon", parent: "modify.manipulate-mesh" },
    { id: "modify.remesh", label: "Remesh", icon: LayoutGrid, status: "soon", parent: "modify.manipulate-mesh" },
    { id: "modify.offset-surface", label: "Offset selected surface", icon: Expand, status: "soon", parent: "modify.manipulate-mesh" },
    { id: "modify.extrude", label: "Extrude", icon: ArrowBigUp, status: "soon", parent: "modify.manipulate-mesh" },
    { id: "modify.distort", label: "Drag points freely", icon: HandGrab, status: "soon", parent: "modify.manipulate-mesh" },
    { id: "modify.surface-pattern", label: "Surface pattern", icon: Grid2x2, status: "soon", parent: "modify.manipulate-mesh" },
    { id: "modify.texture-colour", label: "Texture and colour", icon: Palette, status: "soon" },
    { id: "modify.paint", label: "Paint colour", icon: Paintbrush, status: "soon", parent: "modify.texture-colour" },
    { id: "modify.image-texture", label: "Put an image on the surface", icon: Image, status: "soon", parent: "modify.texture-colour" },
    { id: "modify.image-relief", label: "Raise the surface from an image", icon: ImageUp, status: "soon", parent: "modify.texture-colour" },
    { id: "modify.face-groups", label: "Triangle groups", icon: Group, status: "soon", parent: "modify.texture-colour" },
    { id: "modify.label", label: "Generate label", icon: Tag, status: "ready" },
    { id: "modify.label-text", label: "Text label", icon: Type, status: "ready", run: label("text"), parent: "modify.label", outsideRepair: true, hint: LABEL_HINTS.text },
    { id: "modify.label-image", label: "Image label", icon: Stamp, status: "ready", run: label("image"), parent: "modify.label", outsideRepair: true, hint: LABEL_HINTS.image },
    { id: "modify.label-list", label: "Numbered labels from a list", icon: ListOrdered, status: "ready", run: label("list"), parent: "modify.label", outsideRepair: true, hint: LABEL_HINTS.list },
    { id: "modify.label-tags", label: "Labels on breakaway tags", icon: Tags, status: "ready", run: label("tag"), parent: "modify.label", outsideRepair: true, hint: LABEL_HINTS.tag },
    { id: "modify.qr-code", label: "QR code", icon: QrCode, status: "soon", parent: "modify.label" },
    { id: "modify.base-plate", label: "Generate base plate", icon: RectangleHorizontal, status: "soon" },
    { id: "modify.remove-add-ons", label: "Remove parametric add-ons", icon: PackageMinus, status: "soon" },
  ]),
  ...group("modify", "Hollowing", [
    { id: "modify.hollow", label: "Generate shell", icon: Cylinder, status: "ready", run: generateShell, outsideRepair: true, hint: SHELL_HINT },
    { id: "modify.perforation", label: "Add perforation", icon: Drill, status: "soon" },
  ]),
  ...group("modify", "Lattices", [
    { id: "modify.lattice-assistant", label: "Lattice assistant", icon: WandSparkles, status: "soon" },
    { id: "modify.lattice-commander", label: "Lattice commander", icon: Network, status: "soon" },
    { id: "modify.structure-library", label: "Structure library", icon: Library, status: "soon" },
  ]),
  ...group("modify", "Zoom to", [
    { id: "modify.zoom-everything", label: "Everything", icon: Fullscreen, status: "ready", run: zoomTo("everything"), hint: "Fit the platform and all parts in the view." },
    { id: "modify.zoom-platform", label: "Platform", icon: Scan, status: "ready", run: zoomTo("platform"), hint: "Fit the build platform in the view." },
    { id: "modify.zoom-parts", label: "All parts", icon: ScanSearch, status: "ready", run: zoomTo("parts"), shortcut: "A", hint: "Fit all parts in the view." },
    { id: "modify.zoom-selected", label: "Selected", icon: Focus, status: "ready", run: zoomTo("selected"), shortcut: "F", hint: "Fit the selected parts in the view." },
    { id: "modify.zoom-area", label: "Area", icon: ScanLine, status: "ready", run: viewTool("zoom-area"), viewTool: "zoom-area", hint: "Drag a rectangle in the view to zoom into it." },
    { id: "modify.zoom-point", label: "Point", icon: LocateFixed, status: "ready", run: viewTool("zoom-point"), viewTool: "zoom-point", hint: "Click a point on a part: the view centres on it and turns around it from now on." },
  ]),

  // Arrange -----------------------------------------------------------------------------------
  ...group("arrange", "Transform", [
    { id: "arrange.move", label: "Move", icon: Move, status: "ready", run: move, size: "large", outsideRepair: true, hint: "Move the selected parts." },
    { id: "arrange.rotate", label: "Rotate", icon: Rotate3d, status: "ready", run: rotate, size: "large", outsideRepair: true, hint: "Rotate the selected parts." },
  ]),
  ...group("arrange", "Platform", [
    { id: "arrange.platform-size", label: "Platform size", icon: Proportions, status: "ready", run: () => ui().openDialog({ kind: "platform" }), size: "large", hint: "Set the build platform's width, depth and height in mm." },
    { id: "arrange.place-on-platform", label: "Place on platform", icon: ArrowDownToLine, status: "ready", run: placeOnPlatform, outsideRepair: true, hint: "Drop the selection so it rests on the platform." },
    { id: "arrange.centre-on-platform", label: "Centre on platform", icon: Target, status: "ready", run: centreOnPlatform, outsideRepair: true, hint: "Move the selection to the middle of the platform." },
    { id: "arrange.front-left", label: "Move to front-left corner", icon: ArrowDownLeft, status: "soon" },
  ]),
  ...group("arrange", "Orientation", [
    { id: "arrange.align", label: "Align", icon: AlignStartVertical, status: "ready", size: "large", hint: "Line the selected parts up with each other, put them on the platform, or lay one flat." },
    { id: "arrange.align-parts", label: "Align parts to each other", icon: AlignStartHorizontal, status: "ready", parent: "arrange.align", group: "Align", hint: "Line the selected parts up by the box round all of them." },
    ...ALIGN_TO_EACH_OTHER.map(([how, label, icon, hint]): Entry => ({ id: `arrange.align-${how}`, label, icon, status: "ready", run: align(how), parent: "arrange.align-parts", outsideRepair: true, hint })),
    { id: "arrange.align-platform", label: "Align to the platform", icon: ArrowDownToLine, status: "ready", parent: "arrange.align", group: "Align", hint: "Centre the selection on the platform, move it to the origin, or drop each part onto it." },
    ...ALIGN_TO_PLATFORM.map(([how, label, icon, hint]): Entry => ({ id: `arrange.align-${how}`, label, icon, status: "ready", run: align(how), parent: "arrange.align-platform", outsideRepair: true, hint })),
    { id: "arrange.align-to-part", label: "Match a reference part", icon: CopyCheck, status: "soon", parent: "arrange.align", group: "Align" },
    { id: "arrange.face-down", label: "Lay a face on the platform", icon: SquareArrowDown, status: "ready", run: startLayFlat, viewTool: "lay-flat", parent: "arrange.align", group: "Lay flat", outsideRepair: true, hint: "Click a face of a part: the part turns so that face lies on the platform." },
    { id: "arrange.lay-flat-auto", label: "Lay flat automatically", icon: ArrowDownToDot, status: "ready", run: () => void layFlatAuto(), parent: "arrange.align", group: "Lay flat", outsideRepair: true, hint: "Turn each selected part onto its largest flat face." },
    { id: "arrange.min-box", label: "Rotate to smallest box", icon: Minimize, status: "ready", run: openMinBox, outsideRepair: true, hint: "Turn each selected part so its box is as small as it gets, about Z only or about all axes." },
  ]),
  ...group("arrange", "Layout", [
    { id: "arrange.auto-arrange", label: "Auto-arrange", icon: LayoutDashboard, status: "soon", size: "large" },
    { id: "arrange.packing", label: "Packing", icon: Blocks, status: "ready", run: () => openPack(), size: "large", outsideRepair: true, hint: PACK_HINT },
    { id: "arrange.copies", label: "Copies", icon: CopyPlus, status: "ready", hint: "Copy the selected parts: in place, filling the platform, or in a grid." },
    { id: "arrange.copy-count", label: "Duplicate in place", icon: Copy, status: "ready", run: duplicate("in-place"), parent: "arrange.copies", outsideRepair: true, hint: DUPLICATE_HINTS["in-place"] },
    { id: "arrange.fill-platform", label: "Fill the platform", icon: Grid3x2, status: "ready", run: duplicate("fill"), parent: "arrange.copies", outsideRepair: true, hint: DUPLICATE_HINTS.fill },
    { id: "arrange.copies-grid", label: "Duplicate in a grid", icon: Rows3, status: "ready", run: duplicate("grid"), parent: "arrange.copies", outsideRepair: true, hint: DUPLICATE_HINTS.grid },
  ]),
  ...group("arrange", "Organize", [
    { id: "arrange.group", label: "Group parts", icon: Group, status: "soon" },
    { id: "arrange.rename-template", label: "Rename by pattern", icon: TextCursorInput, status: "soon" },
    { id: "arrange.select-by-property", label: "Select parts by rule", icon: ListFilter, status: "soon" },
  ]),
  ...group("arrange", "Build room", [
    { id: "arrange.no-build-zones", label: "Keep-out zones", icon: Ban, status: "soon" },
    { id: "arrange.collisions", label: "Collision check", icon: OctagonAlert, status: "ready", run: openCollisions, outsideRepair: true, hint: COLLISIONS_HINT },
    { id: "arrange.platform-summary", label: "Platform summary", icon: ClipboardList, status: "soon" },
    { id: "arrange.platforms", label: "Platforms", icon: SquareStack, status: "soon" },
    { id: "arrange.add-platform", label: "Add a platform", icon: SquarePlus, status: "soon", parent: "arrange.platforms" },
    { id: "arrange.clone-platform", label: "Duplicate platform", icon: Copy, status: "soon", parent: "arrange.platforms" },
  ]),

  // View --------------------------------------------------------------------------------------
  ...group("view", "Views", [
    { id: "view.iso", label: "Isometric", icon: Box, status: "ready", run: viewFrom("iso"), size: "large", hint: "Look at the platform from a front corner." },
    { id: "view.top", label: "Top", icon: PanelTop, status: "ready", run: viewFrom("top"), hint: "Look straight down." },
    { id: "view.bottom", label: "Bottom", icon: PanelBottom, status: "ready", run: viewFrom("bottom"), hint: "Look straight up from below." },
    { id: "view.front", label: "Front", icon: Square, status: "ready", run: viewFrom("front"), hint: "Look from the front." },
    { id: "view.back", label: "Back", icon: SquareDashed, status: "ready", run: viewFrom("back"), hint: "Look from the back." },
    { id: "view.left", label: "Left", icon: PanelLeft, status: "ready", run: viewFrom("left"), hint: "Look from the left." },
    { id: "view.right", label: "Right", icon: PanelRight, status: "ready", run: viewFrom("right"), hint: "Look from the right." },
  ]),
  ...group("view", "Projection", [
    { id: "view.perspective", label: "Perspective", icon: Pyramid, status: "ready", run: setView({ projection: "perspective" }), isActive: (s) => s.view.projection === "perspective", hint: "Far things look smaller." },
    { id: "view.orthographic", label: "Orthographic", icon: SquareSquare, status: "ready", run: setView({ projection: "orthographic" }), isActive: (s) => s.view.projection === "orthographic", hint: "Parallel lines stay parallel, good for checking sizes." },
  ]),
  ...group("view", "Shading", [
    { id: "view.shaded", label: "Shaded", icon: Contrast, status: "ready", run: setView({ shading: "shaded" }), isActive: (s) => s.view.shading === "shaded", hint: "Draw solid surfaces." },
    { id: "view.wireframe", label: "Wireframe", icon: Globe, status: "ready", run: setView({ shading: "wireframe" }), isActive: (s) => s.view.shading === "wireframe", hint: "Draw triangle edges only." },
    { id: "view.shaded-edges", label: "Shaded + edges", icon: Gem, status: "ready", run: setView({ shading: "shaded-edges" }), isActive: (s) => s.view.shading === "shaded-edges", hint: "Draw solid surfaces with triangle edges on top." },
  ]),
  ...group("view", "Display", [
    { id: "view.show-platform", label: "Show platform", icon: Grid2x2, status: "ready", run: flipPlatform, isActive: (s) => s.view.showPlatform, hint: "Show or hide the build platform and grid." },
    { id: "view.show-axes", label: "Show axes", icon: Axis3d, status: "ready", run: flipAxes, isActive: (s) => s.view.showAxes, hint: "Show or hide the X, Y and Z axes." },
    { id: "view.show-rulers", label: "Show rulers", icon: Ruler, status: "ready", run: flipRulers, isActive: (s) => s.view.showRulers, hint: "Show or hide the millimetre rulers along the left and bottom edges of the view." },
    { id: "view.always-show-gizmo", label: "Always show gizmo", icon: Move3d, status: "ready", run: flipGizmo, shortcut: mod("Shift+G"), isActive: (s) => s.view.alwaysShowGizmo, hint: "Show the move and rotate gizmo whenever parts are selected. When off, it shows after Move, Rotate, W, E or a scale." },
    { id: "view.show-textures", label: "Show textures", icon: Images, status: "soon" },
    { id: "view.display-colours", label: "Viewport colours", icon: PaintBucket, status: "soon" },
    { id: "view.random-colours", label: "Random part colours", icon: Dices, status: "soon" },
  ]),
  ...group("view", "Slices", [
    { id: "view.slice-viewer", label: "Slice viewer", icon: Layers, status: "soon" },
    { id: "view.toolpath-playback", label: "Play back toolpaths", icon: CirclePlay, status: "soon" },
  ]),
  ...group("view", "Capture", [
    { id: "view.screenshot", label: "Screenshot", icon: Camera, status: "soon", size: "large" },
  ]),
  ...group("view", "Interface", [
    { id: "view.find-command", label: "Find a command", icon: Search, status: "soon" },
    { id: "view.search-settings", label: "Search settings", icon: Settings2, status: "soon" },
    { id: "view.shortcuts", label: "Keyboard shortcuts", icon: Keyboard, status: "soon" },
    { id: "view.panel-layouts", label: "Panel layouts", icon: PanelsTopLeft, status: "soon" },
    { id: "view.custom-tab", label: "Custom ribbon tab", icon: LayoutTemplate, status: "soon" },
    { id: "view.message-log", label: "Message log", icon: MessageSquareWarning, status: "soon" },
    { id: "view.language", label: "Language", icon: Languages, status: "soon" },
  ]),

  // Part context menu only --------------------------------------------------------------------
  ...group("context", "Part", [
    { id: "context.export", label: "Export part…", icon: FileOutput, status: "ready", run: exportSelection, shortcut: mod("E"), hint: EXPORT_HINT },
    { id: "context.duplicate", label: "Duplicate", icon: Copy, status: "ready", run: duplicateSelection, hint: "Copy the selected parts beside the originals." },
    { id: "context.rename", label: "Rename", icon: Pencil, status: "ready", run: renameSelection, hint: "Give the part a new name." },
    { id: "context.packing-settings", label: "Packing settings…", icon: Blocks, status: "ready", run: packingSettings, outsideRepair: true, hint: "Set the selected parts' packing priority, permitted rotation and position lock." },
    { id: "context.repair", label: "Repair part", icon: Wrench, status: "ready", run: repairPart, outsideRepair: true, hint: REPAIR_HINT },
    { id: "context.batch-repair", label: "Repair many parts at once", icon: ListChecks, status: "ready", run: () => void batchRepair(), outsideRepair: true, hint: "Run Default repair on the selected parts, or on every unlocked part." },
    { id: "context.copy", label: "Copy", icon: Copy, status: "ready", run: copyParts, shortcut: mod("C"), outsideRepair: true, hint: "Copy the selected parts to the clipboard." },
    { id: "context.paste", label: "Paste", icon: ClipboardPaste, status: "ready", run: () => void pasteParts(), shortcut: mod("V"), outsideRepair: true, hint: "Paste the copied parts beside the originals." },
    { id: "context.delete", label: "Delete", icon: Trash, status: "ready", run: deleteSelection, shortcut: "Del", outsideRepair: true, hint: "Remove the selected parts. Locked parts stay." },
    { id: "context.centre-view", label: "Centre view here", icon: Crosshair, status: "ready", run: centreViewHere, hint: "Orbit around this point from now on." },
    { id: "context.toggle-visible", label: "Show/Hide selected", icon: Eye, status: "ready", run: toggleSelectedVisible, hint: "Hide the selected parts, or show them again." },
    { id: "context.toggle-transparent", label: "Make selected transparent", icon: Blend, status: "ready", run: toggleSelectedTransparent, hint: "See through the selected parts, or make them solid again." },
    { id: "context.hide-unselected", label: "Hide unselected", icon: EyeOff, status: "ready", run: hideUnselected, hint: "Hide every part that is not selected." },
    { id: "context.show-all", label: "Show all", icon: Eye, status: "ready", run: showAll, hint: "Show every part, none transparent." },
  ]),

  // Quick-action strip at the bottom of the right panel ---------------------------------------
  ...group("quick", "Quick", [
    { id: "quick.repair", label: "Repair", icon: Wrench, status: "ready", run: repairPart, outsideRepair: true, hint: REPAIR_HINT },
    { id: "quick.model-package", label: "Create model package", icon: Package, status: "soon" },
    { id: "quick.slice", label: "Slice selected parts", icon: Layers, status: "soon" },
    { id: "quick.file-preview", label: "File preview browser", icon: FileSearch, status: "soon" },
    { id: "quick.collisions", label: "Collision detection", icon: OctagonAlert, status: "ready", run: openCollisions, outsideRepair: true, hint: COLLISIONS_HINT },
  ]),

  // Repair (contextual tab, shown in repair mode) ---------------------------------------------
  ...group("repair", "Repair", [
    { id: "repair.script", label: "Repair script", icon: ScrollText, status: "ready", run: pickScript, size: "large", hint: "Pick a repair script and run it on the part." },
    { id: "repair.edit-scripts", label: "Edit repair scripts", icon: FilePen, status: "ready", run: () => void editRepairScripts(), hint: "Open the repair scripts list." },
  ]),
  ...group("repair", "Info", [
    { id: "repair.selection-info", label: "Show selection info", icon: BadgeInfo, status: "ready", run: showSelectionInfo, size: "large", hint: "Show how many triangles are selected, their area and the shells they touch." },
    { id: "repair.status", label: "Status", icon: Activity, status: "ready", hint: "What is wrong with the part, and its shells." },
    { id: "repair.damage-warnings", label: "Damage warnings", icon: TriangleAlert, status: "ready", run: () => void damageWarnings(), parent: "repair.status", hint: "Show what is wrong with the part." },
    { id: "repair.problem-counts", label: "Problem counts", icon: ClipboardList, status: "ready", run: () => void problemCounts(), parent: "repair.status", hint: "Count the holes, borders and other problems." },
    { id: "repair.shell-list", label: "Shell list", icon: List, status: "ready", run: () => void shellList(), parent: "repair.status", hint: "Open the shell list for the part being repaired." },
  ]),
  ...group("repair", "Select", [
    { id: "repair.select-primitive", label: "Primitive", icon: Shapes, status: "ready", run: pickTool("area"), repairActive: toolIs("area"), hint: "Drag a rectangle anywhere to select the triangles facing you." },
    { id: "repair.select-triangle", label: "Triangle", icon: Triangle, status: "ready", run: pickTool("triangle"), repairActive: toolIs("triangle"), hint: "Click to select single triangles." },
    { id: "repair.select-surface", label: "Surface", icon: VectorSquare, status: "ready", run: pickTool("surface"), repairActive: toolIs("surface"), hint: "Click to select a surface: the triangles around it up to the View tab's tolerance angle." },
    { id: "repair.select-brush", label: "Brush", icon: Brush, status: "ready", run: pickTool("brush"), repairActive: toolIs("brush"), hint: "Drag across the part to select the triangles under the pointer." },
    { id: "repair.select-shell", label: "Shell", icon: Cuboid, status: "ready", run: pickTool("shell"), repairActive: toolIs("shell"), hint: "Click to select a whole shell." },
    { id: "repair.select-all", label: "All", icon: SquareCheckBig, status: "ready", run: selectAll, hint: "Select every triangle." },
    { id: "repair.select-none", label: "None", icon: SquareX, status: "ready", run: selectNone, shortcut: "Esc", hint: "Clear the triangle selection." },
    { id: "repair.select-overlap", label: "Overlap", icon: Layers2, status: "ready", run: () => void findOverlaps(), hint: "Select the triangles that cross or repeat others, over the whole part." },
    { id: "repair.select-toggle", label: "Toggle", icon: ToggleLeft, status: "ready", run: invertSelection, hint: "Select the unselected triangles and deselect the selected ones." },
    { id: "repair.select-advanced", label: "Advanced", icon: SlidersHorizontal, status: "ready" },
    { id: "repair.select-criteria", label: "Select damaged triangles", icon: ListFilter, status: "ready", run: () => void selectByCriteria(), parent: "repair.select-advanced", hint: "Select every damaged triangle." },
    { id: "repair.hide-triangles", label: "Hide selected triangles", icon: EyeOff, status: "ready", run: () => void hideTriangles(), parent: "repair.select-advanced", hint: "Hide the selected triangles, or show them all again." },
  ]),
  ...group("repair", "Refine mesh", [
    { id: "repair.z-comp", label: "Z-comp", icon: ChevronsDownUp, status: "soon" },
    { id: "repair.remesh", label: "Remesh", icon: LayoutGrid, status: "soon" },
    { id: "repair.offset", label: "Offset", icon: Expand, status: "soon" },
    { id: "repair.join-contours", label: "Join contours", icon: Waypoints, status: "soon" },
    { id: "repair.close-freeform", label: "Close freeform holes", icon: CircleDashed, status: "soon" },
    { id: "repair.split-non-manifold", label: "Split non-manifold edges", icon: Unlink2, status: "ready", run: repairAction("split_non_manifold_edges"), hint: "Separate edges shared by more than two triangles." },
  ]),
  ...group("repair", "Add", [
    { id: "repair.add-triangle", label: "Add triangle", icon: TriangleRight, status: "soon" },
    { id: "repair.add-nodes", label: "Add nodes", icon: CirclePlus, status: "soon" },
    { id: "repair.add-free-triangle", label: "Add free triangle", icon: TriangleDashed, status: "soon" },
  ]),
  ...group("repair", "Remove", [
    { id: "repair.remove-triangles", label: "Remove triangle", icon: Trash, status: "ready", run: removeTriangles, shortcut: "Del", needsTriangles: true, hint: "Delete the selected triangles." },
    { id: "repair.remove-edge", label: "Remove edge", icon: Eraser, status: "soon" },
  ]),
  ...group("repair", "Edit mesh", [
    { id: "repair.flip-selected", label: "Flip selected", icon: RefreshCcw, status: "ready", run: flipTriangles, needsTriangles: true, hint: "Turn the selected triangles to face the other way." },
    { id: "repair.move-nodes", label: "Move nodes", icon: Move, status: "ready", run: () => void moveNodes(), repairActive: editIs("move-nodes"), hint: "Drag a point of the surface; the triangles round it follow." },
    { id: "repair.cut-surfaces", label: "Cut surfaces", icon: PenLine, status: "ready", run: () => void cutSurfaces(), repairActive: editIs("cut"), hint: "Draw a line on the surface through clicked points and cut the surface open along it." },
    { id: "repair.extrude", label: "Extrude surfaces", icon: ArrowBigUp, status: "soon" },
    { id: "repair.extract", label: "Extract parts", icon: SquareArrowOutUpRight, status: "ready", hint: "Make a new part from the selected triangles." },
    { id: "repair.extract-copy", label: "Extract selected triangles as part", icon: SquareArrowOutUpRight, status: "ready", run: extractCopy, parent: "repair.extract", needsTriangles: true, hint: "Add the selected triangles as a new part and keep them here too." },
    { id: "repair.extract-separate", label: "Separate selection as part", icon: PackageOpen, status: "ready", run: extractSeparate, parent: "repair.extract", needsTriangles: true, hint: "Move the selected triangles out into a new part." },
    { id: "repair.extract-shells", label: "Selected shells to new part", icon: PackageOpen, status: "ready", run: () => void extractSelectedShells(), parent: "repair.extract", hint: "Copy the shells the selected triangles belong to into a new part." },
  ]),
  ...group("repair", "Zoom to", [
    { id: "repair.zoom-part", label: "Part", icon: ScanSearch, status: "ready", run: zoomToPart, shortcut: "A", hint: "Fit the part in the view." },
    { id: "repair.zoom-selected", label: "Selected", icon: Focus, status: "ready", run: zoomToTriangles, shortcut: "F", hint: "Fit the selected triangles in the view, or the part when none is selected." },
    { id: "repair.zoom-area", label: "Area", icon: ScanLine, status: "ready", run: viewTool("zoom-area"), viewTool: "zoom-area", hint: "Drag a rectangle in the view to zoom into it." },
  ]),
  ...group("repair", "Close", [
    { id: "repair.apply", label: "Apply repair", icon: CheckCheck, status: "ready", run: requestApply, size: "large", hint: "Keep the repaired part and leave repair mode." },
    { id: "repair.close", label: "Close repair", icon: X, status: "ready", run: () => void closeRepair(), size: "large", hint: "Leave repair mode without applying the changes." },
  ]),

  // Boolean operations (contextual tab, shown while the mode is open) -------------------------
  ...group("boolean", "Select", [
    { id: "boolean.select", label: "Select", icon: MousePointer2, status: "ready", run: pickBooleanTool("select"), booleanActive: booleanToolIs("select"), size: "large", hint: "Click a part to select it in its list." },
    { id: "boolean.zoom-area", label: "Zoom to area", icon: ScanLine, status: "ready", run: pickBooleanTool("zoom-area"), booleanActive: booleanToolIs("zoom-area"), size: "large", hint: "Drag a rectangle in the view to zoom into it." },
  ]),
  ...group("boolean", "Zoom to", [
    { id: "boolean.zoom-part", label: "Zoom to part", icon: ScanSearch, status: "ready", run: zoomToBooleanPart, size: "large", hint: "Fit the part selected in the lists in the view, or all of their parts when none is selected." },
  ]),
  ...group("boolean", "Close", [
    { id: "boolean.ok", label: "OK", icon: CheckCheck, status: "ready", run: acceptBoolean, size: "large", hint: "Add the result as a new part and leave Boolean operations." },
    { id: "boolean.close", label: "Close Boolean operations", icon: X, status: "ready", run: () => void closeBooleanMode(), size: "large", hint: "Leave Boolean operations without adding a result." },
  ]),

  // Measurement (contextual tab, shown in Measure mode) ---------------------------------------
  ...group("measure", "New measurement", [
    ...MENU_GROUPS.flatMap((g): Entry[] => [
      { id: `measure.${g}`, label: GROUP_LABELS[g], icon: GROUP_ICONS[g], status: "ready", size: "large", measureActive: groupIs(g), hint: `Measurements that start from a ${GROUP_LABELS[g].toLowerCase()}.` },
      ...toolEntries(g, `measure.${g}`),
    ]),
    ...toolEntries("wall"),
    ...toolEntries("angle3"),
  ]),
  ...group("measure", "Exit measurement", [
    { id: "measure.cancel", label: "Cancel", icon: X, status: "ready", run: closeMeasure, size: "large", hint: "Leave Measure. The measurements stay with their parts and show again next time." },
  ]),

  // Part analysis (contextual tab, shown while an analysis is on show) ------------------------
  ...group("analysis", "Edit", [
    { id: "analysis.save", label: "Save project (.3mf)", icon: Save, status: "ready", run: saveProject, shortcut: mod("S"), hint: SAVE_HINT },
    { id: "analysis.undo", label: "Undo", icon: Undo2, status: "ready", run: undo, dropdown: () => historyMenu("undo"), shortcut: mod("Z"), hint: "Undo the last change. The arrow lists the recent changes, to undo several at once." },
    { id: "analysis.redo", label: "Redo", icon: Redo2, status: "ready", run: redo, dropdown: () => historyMenu("redo"), shortcut: mod("Shift+Z"), hint: "Redo the change you undid. The arrow lists the changes you can redo." },
  ]),
  ...group("analysis", "Zoom to", [
    { id: "analysis.zoom-part", label: "Zoom to part", icon: ScanSearch, status: "ready", run: zoomToAnalysisPart, size: "large", hint: "Fit the analysed part in the view." },
    { id: "analysis.zoom-area", label: "Zoom to area", icon: ScanLine, status: "ready", run: viewTool("zoom-area"), viewTool: "zoom-area", size: "large", hint: "Drag a rectangle in the view to zoom into it." },
  ]),
  ...group("analysis", "Analyze", [
    { id: "analysis.measure", label: "Measure", icon: Ruler, status: "ready", run: () => openMeasure(), size: "large", hint: MEASURE_HINT },
  ]),
  ...group("analysis", "Exit part analysis", [
    { id: "analysis.ok", label: "OK", icon: Check, status: "ready", run: closeAnalysisView, size: "large", hint: "Leave the analysis. It stays in the parts tree under its part." },
    { id: "analysis.close", label: "Close part analysis", icon: X, status: "ready", run: closeShownAnalysis, size: "large", hint: "Leave the analysis and take it off the part (one undo step)." },
  ]),

  // Label generation (contextual tab, shown while the mode is open) ---------------------------
  ...group("label", "Edit", [
    { id: "label.save", label: "Save project (.3mf)", icon: Save, status: "ready", run: saveProject, hint: `${SAVE_HINT} Labels not yet applied are not saved.` },
    { id: "label.undo", label: "Undo", icon: Undo2, status: "ready", run: undo, dropdown: () => historyMenu("undo"), hint: "Undo the last change to the labels. The arrow lists the recent changes." },
    { id: "label.redo", label: "Redo", icon: Redo2, status: "ready", run: redo, dropdown: () => historyMenu("redo"), hint: "Redo the change you undid." },
  ]),
  ...group("label", "Generate label", [
    { id: "label.select", label: "Select", icon: MousePointer2, status: "ready", run: () => pickLabelTool("select"), labelActive: (l) => l.session?.tool === "select", size: "large", hint: "Click a label to change it; drag its handles to move, size and turn it." },
    { id: "label.place", label: "Place label", icon: MapPin, status: "ready", run: () => placeKind(), labelActive: (l) => l.session?.tool === "place", size: "large", hint: "Click on the part to place another label of the kind picked last." },
    { id: "label.generate", label: "Generate label", icon: Tag, status: "ready", size: "large" },
    { id: "label.add-text", label: "Text", icon: Type, status: "ready", run: () => placeKind("text"), parent: "label.generate", labelActive: kindIs("text"), hint: LABEL_HINTS.text },
    { id: "label.add-image", label: "Image", icon: Stamp, status: "ready", run: () => placeKind("image"), parent: "label.generate", labelActive: kindIs("image"), hint: LABEL_HINTS.image },
    { id: "label.add-tag", label: "Tag", icon: Tags, status: "ready", run: () => placeKind("tag"), parent: "label.generate", labelActive: kindIs("tag"), hint: LABEL_HINTS.tag },
    { id: "label.add-list", label: "Text from list", icon: ListOrdered, status: "ready", run: () => placeKind("list"), parent: "label.generate", labelActive: kindIs("list"), hint: LABEL_HINTS.list },
  ]),
  ...group("label", "Zoom to", [
    { id: "label.zoom-part", label: "Zoom to part", icon: ScanSearch, status: "ready", run: zoomToLabelPart, size: "large", hint: "Fit the part being labelled in the view." },
    { id: "label.zoom-label", label: "Zoom to label", icon: Focus, status: "ready", run: zoomToLabel, size: "large", hint: "Fit the selected label in the view, or all of them when none is selected." },
    { id: "label.zoom-area", label: "Zoom to area", icon: ScanLine, status: "ready", run: viewTool("zoom-area"), viewTool: "zoom-area", size: "large", hint: "Drag a rectangle in the view to zoom into it." },
  ]),
  ...group("label", "Exit label generation", [
    { id: "label.apply", label: "Apply to part", icon: Check, status: "ready", run: () => void applyLabelMode(), size: "large", hint: "Put the labels into the part, then choose whether to keep the original." },
    { id: "label.close", label: "Close label generation", icon: X, status: "ready", run: () => void closeLabelMode(), size: "large", hint: "Leave without applying the labels." },
  ]),

  // Part library (contextual tab, shown while the library is open) -----------------------------
  ...group("library", "Create", [
    { id: "library.create", label: "Create part", icon: PackagePlus, status: "ready", run: () => void createPart(), size: "large", hint: "Add the part on the middle of the platform (or, made from the selection, where it is), and close the library." },
  ]),
  ...group("library", "Close", [
    { id: "library.cancel", label: "Cancel", icon: X, status: "ready", run: closePartLibrary, size: "large", hint: "Close the part library without adding a part." },
  ]),

  // Repair context menu (right-click on the working copy), top to bottom ----------------------
  ...group("repair-context", "This triangle", [
    { id: "repair-ctx.add-triangle", label: "Add this triangle to selection", icon: Triangle, status: "ready", run: addUnderCursor("triangle"), underCursor: true, hint: "Add the triangle under the cursor to the selection." },
    { id: "repair-ctx.add-surface", label: "Add this surface to selection", icon: VectorSquare, status: "ready", run: addUnderCursor("surface"), underCursor: true, hint: "Add the surface under the cursor to the selection, up to the View tab's tolerance angle." },
    { id: "repair-ctx.add-shell", label: "Add this shell to selection", icon: Cuboid, status: "ready", run: addUnderCursor("shell"), underCursor: true, hint: "Add the shell under the cursor to the selection." },
  ]),
  ...group("repair-context", "Selection", [
    { id: "repair-ctx.expand", label: "Expand selection", icon: SquarePlus, status: "ready", run: expandSelection, shortcut: "=", needsTriangles: true, hint: "Add every triangle that shares a corner with the selection." },
    { id: "repair-ctx.shrink", label: "Shrink selection", icon: SquareMinus, status: "ready", run: shrinkSelection, shortcut: "-", needsTriangles: true, hint: "Drop the selected triangles that share a corner with an unselected one." },
    { id: "repair-ctx.select-criteria", label: "Select damaged triangles", icon: ListFilter, status: "ready", run: () => void selectByCriteria(), hint: "Select every damaged triangle." },
    { id: "repair-ctx.hide-triangles", label: "Hide selected triangles", icon: EyeOff, status: "ready", run: () => void hideTriangles(), hint: "Hide the selected triangles, or show them all again." },
  ]),
  ...group("repair-context", "Mesh", [
    { id: "repair-ctx.reduce", label: "Reduce triangles", icon: Minimize2, status: "soon" },
    { id: "repair-ctx.refine", label: "Refine triangle mesh", icon: Grid3x3, status: "ready", run: () => void refineTriangles(), hint: "Split each selected triangle, or every triangle, into four." },
    { id: "repair-ctx.insert-point", label: "Add point", icon: Plus, status: "ready", run: () => void addPoint(), needsTriangles: true, hint: "Add a point in the middle of each selected triangle." },
    { id: "repair-ctx.smooth", label: "Smooth triangles", icon: WavesHorizontal, status: "ready", run: repairAction("smooth_triangles"), hint: "Move each point inside the selection, or of the whole part, to the average of its neighbours." },
    { id: "repair-ctx.z-compensation", label: "Z-compensation", icon: ChevronsDownUp, status: "soon" },
    { id: "repair-ctx.remesh", label: "Remesh", icon: LayoutGrid, status: "soon" },
    { id: "repair-ctx.offset", label: "Offset selection", icon: Expand, status: "soon" },
  ]),
  ...group("repair-context", "Holes", [
    { id: "repair-ctx.close-trivial", label: "Close trivial holes", icon: CircleSmall, status: "ready", run: repairAction("close_trivial_holes"), hint: "Fill holes bordered by 3 edges, or by 4 edges that are nearly flat." },
    { id: "repair-ctx.close-defined", label: "Close defined holes", icon: CircleDashed, status: "ready", run: repairAction("close_defined_holes"), needsTriangles: true, hint: "Fill the holes that border the selected triangles." },
    { id: "repair-ctx.close-all", label: "Close all holes", icon: CircleDot, status: "ready", run: repairAction("close_all_holes"), hint: "Fill every hole." },
    { id: "repair-ctx.close-curved", label: "Close holes along the curvature", icon: Spline, status: "ready", run: () => void closeHolesCurved(), hint: "Fill the holes with patches that carry on the curve of the surface round them, flat holes too." },
    { id: "repair-ctx.fill-three-edge", label: "Fill three-edge holes", icon: TriangleRight, status: "ready", run: () => void addTriangle(), hint: "Fill each hole bordered by three edges with one triangle; with none, close the trivial holes." },
    { id: "repair-ctx.grow-band", label: "Grow a band from a hole edge", icon: CircleFadingPlus, status: "ready", run: () => void offsetHoleEdge(), hint: "Add a band of triangles along an open edge." },
    { id: "repair-ctx.bridge-gap", label: "Bridge a gap", icon: Bridge, status: "soon" },
    { id: "repair-ctx.stitch", label: "Stitch triangles", icon: Link, status: "ready", run: repairAction("stitch_triangles"), hint: "Join open edges that lie within 0.010 mm, closing gaps and cracks." },
  ]),
  ...group("repair-context", "Clean up", [
    { id: "repair-ctx.remove-duplicates", label: "Remove duplicate triangles", icon: CopyX, status: "ready", run: () => void removeDuplicateFaces(), hint: "Remove triangles that repeat the same three points." },
    { id: "repair-ctx.remove-degenerate", label: "Collapse degenerate triangles", icon: TriangleDashed, status: "ready", run: () => void removeDegenerate(), hint: "Collapse triangles that have no area." },
    { id: "repair-ctx.merge-points", label: "Merge duplicate points", icon: Dot, status: "ready", run: () => void mergeDuplicatePoints(), hint: "Merge points that sit on top of each other." },
    { id: "repair-ctx.remove-tiny-shells", label: "Delete small shells", icon: Shrink, status: "ready", run: () => void removeTinyShells(), hint: "Delete shells that are too small to keep." },
    { id: "repair-ctx.self-intersections", label: "Fix self-intersections", icon: SquaresIntersect, status: "ready", run: () => void fixSelfIntersections(), hint: "Remove triangles that cut through the part." },
    { id: "repair-ctx.remove-problem-areas", label: "Cut out damaged areas", icon: Eraser, status: "ready", run: () => void removeProblemAreas(), hint: "Cut out the damaged parts of the surface." },
    { id: "repair-ctx.wrap-surface", label: "Keep only the outer surface", icon: Package, status: "ready", run: () => void wrapSurface(), hint: "Replace the part with its outer surface." },
  ]),
  ...group("repair-context", "Triangles", [
    { id: "repair-ctx.remove-selected", label: "Remove selected triangles", icon: Trash, status: "ready", run: removeTriangles, shortcut: "Del", needsTriangles: true, hint: "Delete the selected triangles." },
    { id: "repair-ctx.flip-selected", label: "Flip selected triangles", icon: RefreshCcw, status: "ready", run: flipTriangles, needsTriangles: true, hint: "Turn the selected triangles to face the other way." },
    { id: "repair-ctx.fix-flipped", label: "Fix flipped triangles", icon: ArrowDownUp, status: "ready", run: repairAction("fix_flipped_triangles"), hint: "Turn triangles that face the wrong way, then make each closed shell face outward." },
    { id: "repair-ctx.invert-shells", label: "Turn inside-out shells", icon: IterationCcw, status: "ready", run: () => void invertShells(), hint: "Turn shells whose volume is negative." },
    { id: "repair-ctx.split-non-oriented", label: "Split non-oriented edges", icon: Unlink, status: "ready", run: repairAction("split_non_oriented_edges"), hint: "Separate edges whose two triangles face opposite ways." },
  ]),
  ...group("repair-context", "Extract", [
    { id: "repair-ctx.extract-copy", label: "Extract selected triangles as part", icon: SquareArrowOutUpRight, status: "ready", run: extractCopy, needsTriangles: true, hint: "Add the selected triangles as a new part and keep them here too." },
    { id: "repair-ctx.extract-separate", label: "Separate selection as part", icon: PackageOpen, status: "ready", run: extractSeparate, needsTriangles: true, hint: "Move the selected triangles out into a new part." },
    { id: "repair-ctx.extract-shells", label: "Selected shells to new part", icon: PackageOpen, status: "ready", run: () => void extractSelectedShells(), hint: "Copy the shells the selected triangles belong to into a new part." },
  ]),
];

/** Part context menu, top to bottom. "-" is a separator. Ids refer to FEATURES. */
export const CONTEXT_MENU: string[] = [
  "context.export",
  "-",
  "context.copy",
  "context.paste",
  "context.duplicate",
  "context.rename",
  "context.repair",
  "context.batch-repair",
  "context.packing-settings",
  "context.delete",
  "-",
  "context.centre-view",
  "-",
  "arrange.move",
  "arrange.rotate",
  "modify.scale",
  "-",
  "context.toggle-visible",
  "context.toggle-transparent",
  "context.hide-unselected",
  "context.show-all",
  "-",
  "modify.split-shells",
  "modify.merge-parts",
  "modify.boolean",
];

const byId = new Map(FEATURES.map((f) => [f.id, f]));

const childMap = new Map<string, Feature[]>();
for (const f of FEATURES) {
  if (f.parent) childMap.set(f.parent, [...(childMap.get(f.parent) ?? []), f]);
}

export function featureById(id: string): Feature | undefined {
  return byId.get(id);
}

export function childrenOf(id: string): Feature[] {
  return childMap.get(id) ?? [];
}

export function isMenu(f: Feature): boolean {
  return childMap.has(f.id);
}

/** A menu is ready when at least one child is ready; anything else uses its own status. */
export function effectiveStatus(f: Feature): FeatureStatus {
  const kids = childrenOf(f.id);
  if (kids.length === 0) return f.status;
  return kids.some((k) => effectiveStatus(k) === "ready") ? "ready" : "soon";
}

export function soonMessage(f: Feature): string {
  return `${f.label} is coming in a future version.`;
}

/**
 * Why a ready entry cannot run right now, or null when it can: a main-document command while repair mode or Boolean
 * operations is open (`booleanOpen`), an entry that needs selected triangles while none is selected, or a "this triangle" entry when the repair context
 * menu did not open on a triangle (`triangle`, from the menu's state). Such entries show greyed out with the reason
 * in their tooltip, and a click only shows the reason. Soon entries are never blocked: they have their own message.
 */
export function blockedReason(
  f: Feature,
  r: Pick<RepairState, "session"> = useRepair.getState(),
  triangle: number | null | undefined = useUi.getState().contextMenu?.triangle,
  booleanOpen: boolean = isBooleanOpen(),
): string | null {
  if (effectiveStatus(f) === "soon") return null;
  if (f.outsideRepair && r.session) return CLOSE_REPAIR_FIRST;
  if (f.outsideRepair && booleanOpen) return CLOSE_BOOLEAN_FIRST;
  if (f.outsideRepair && isLibraryOpen()) return CLOSE_LIBRARY_FIRST;
  if (f.outsideRepair && isMeasureOpen()) return CLOSE_MEASURE_FIRST;
  if (f.outsideRepair && isLabelOpen()) return CLOSE_LABEL_FIRST;
  if (f.needsTriangles && (r.session?.selectedCount ?? 0) === 0) return SELECT_TRIANGLES_FIRST;
  if (f.underCursor && typeof triangle !== "number") return PICK_TRIANGLE_FIRST;
  return null;
}

/** Tooltip text: the soon message, or the label with its shortcut and hint (the reason it is blocked, if it is). */
export function tooltipFor(f: Feature, blocked: string | null = null): string {
  if (effectiveStatus(f) === "soon") return soonMessage(f);
  const name = f.shortcut ? `${f.label} (${f.shortcut})` : f.label;
  const detail = blocked ?? f.hint;
  return detail ? `${name} — ${detail}` : name;
}

/** Status-bar text while the pointer is over an entry: the soon message, the reason it is blocked, or its hint. */
export function hoverHintFor(f: Feature, blocked: string | null = null): string {
  return effectiveStatus(f) === "soon" ? soonMessage(f) : (blocked ?? f.hint ?? f.label);
}

/** True when Settings → "Hide unavailable features" should hide this entry. */
export function isHidden(f: Feature, hideSoon: boolean): boolean {
  return hideSoon && effectiveStatus(f) === "soon";
}

/**
 * What a click does: soon entries (menus too) toast, blocked entries toast why (blockedReason), ready entries run.
 * Menus are opened by the UI, not here.
 */
export function activate(f: Feature) {
  if (effectiveStatus(f) === "soon") {
    toast(soonMessage(f));
    return;
  }
  const blocked = blockedReason(f);
  if (blocked) {
    toast(blocked);
    return;
  }
  f.run?.();
}

export function runFeature(id: string) {
  const f = featureById(id);
  if (f) activate(f);
}

/** The tabs whose entries only show in repair mode, and those of either mode. */
const REPAIR_TABS: readonly TabId[] = ["repair", "repair-context"];
const MODE_TABS: readonly TabId[] = [...REPAIR_TABS, "boolean", "library", "measure", "label"];

/** Registry mistakes that would otherwise only show up by clicking around. Logged in development. */
export function registryProblems(): string[] {
  const problems: string[] = [];
  const seen = new Set<string>();
  for (const f of FEATURES) {
    if (seen.has(f.id)) problems.push(`duplicate id ${f.id}`);
    seen.add(f.id);
    if (f.parent && !byId.has(f.parent)) problems.push(`${f.id}: unknown parent ${f.parent}`);
    if (f.parent && byId.get(f.parent)?.tab !== f.tab) problems.push(`${f.id}: tab differs from its menu`);
    if (!REPAIR_TABS.includes(f.tab) && (f.repairActive || f.needsTriangles)) problems.push(`${f.id}: repair mode state outside the repair tabs`);
    if (f.dropdown && (isMenu(f) || !f.run)) problems.push(`${f.id}: a split button needs a run and no children`);
    if (f.underCursor && f.tab !== "repair-context") problems.push(`${f.id}: only the repair context menu has a triangle under the cursor`);
    if (f.booleanActive && f.tab !== "boolean") problems.push(`${f.id}: Boolean operations state outside its tab`);
    if (f.measureActive && f.tab !== "measure") problems.push(`${f.id}: Measure state outside its tab`);
    if (f.labelActive && f.tab !== "label") problems.push(`${f.id}: label generation state outside its tab`);
    if (MODE_TABS.includes(f.tab) && f.outsideRepair) problems.push(`${f.id}: a mode's entry cannot be blocked in its mode`);
    if (isMenu(f)) {
      if (f.run) problems.push(`${f.id}: a menu cannot have run`);
      continue;
    }
    if (f.status === "ready" && !f.run) problems.push(`${f.id}: ready but has no run`);
    if (f.status === "soon" && f.run) problems.push(`${f.id}: soon but has a run`);
  }
  for (const id of CONTEXT_MENU) {
    if (id !== "-" && !byId.has(id)) problems.push(`context menu: unknown id ${id}`);
  }
  return problems;
}

if (import.meta.env.DEV) {
  for (const p of registryProblems()) console.error(`[features] ${p}`);
}
