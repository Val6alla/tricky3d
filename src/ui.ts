import { create } from "zustand";
import type { TabId } from "./features";
import type { PackMethod } from "./arrange/pack";
import type { DuplicateMode } from "./arrange/types";
import type { ExportOptions, TransformMode, Vec3 } from "./store";

export type ToastKind = "info" | "warn" | "error";

export interface ToastItem {
  id: number;
  message: string;
  kind: ToastKind;
}

/** Answer to "Place on platform?" when importing a file. */
export interface PlaceAnswer {
  place: boolean;
  /** Use this answer for later imports without asking. */
  remember: boolean;
}

/** Answer to "one part / one part per object" for a file with several objects. */
export type ObjectsAnswer = "single" | "split";

/** Answer to "Split into many parts?": split as asked, or repair the part first. */
export type SplitAnswer = "split" | "repair";

export type Dialog =
  | { kind: "platform" }
  | { kind: "rename"; partId: string }
  | { kind: "settings" }
  | { kind: "about" }
  | { kind: "formats" }
  | { kind: "transform"; mode: TransformMode }
  | { kind: "cut" }
  /** Home › Platform statistics. */
  | { kind: "platform-stats" }
  /** Home › Generate shell and Modify › Generate shell; runs and shows its progress itself. */
  | { kind: "generate-shell" }
  // Arrange (src/arrange). Their runs show progress and Cancel in the busy box (src/arrange/ArrangeBusy.tsx).
  /** Home › Duplicate and Arrange › Copies: in place, filling the platform, or in a grid. */
  | { kind: "duplicate"; mode: DuplicateMode }
  /** Arrange › Rotate to smallest box: about Z only or all axes. */
  | { kind: "min-box" }
  /** Home › Orient for the part `partId`. */
  | { kind: "orient"; partId: string }
  /** Home › Pack ▸ a method, Arrange › Packing: the method's settings, its presets, and Pack platform. */
  | { kind: "pack"; method: PackMethod }
  /** Right-click › Packing settings for the parts `ids`: priority, permitted rotation, position lock. */
  | { kind: "packing-settings"; ids: string[] }
  // Prompts answer through resolve; null means the dialog was cancelled.
  | { kind: "place-on-import"; fileName: string; resolve(answer: PlaceAnswer | null): void }
  | { kind: "import-objects"; fileName: string; count: number; resolve(answer: ObjectsAnswer | null): void }
  /** A unit-less file `size` mm across, small enough to have been modelled in metres: true scales it by 1000. */
  | { kind: "scale-on-import"; fileName: string; size: number; resolve(answer: boolean | null): void }
  /** Split shells into parts would make `count` parts; `partName` when they all come from one part. */
  | { kind: "split-many"; count: number; partName: string | null; resolve(answer: SplitAnswer | null): void }
  /** Export's options for `count` parts, opened on `options` (the last ones used). */
  | { kind: "export-options"; count: number; options: ExportOptions; resolve(answer: ExportOptions | null): void }
  // Repair mode (src/repair/dialogs).
  /** Several parts selected: pick the one to repair. */
  | { kind: "repair-choose-part"; ids: string[] }
  | { kind: "repair-apply" }
  | { kind: "repair-script-picker" }
  /** A repair run's progress bar and Cancel; closes itself when the run ends. */
  | { kind: "repair-progress" }
  /** The gear button: rename a script (`scriptId`), or name a new one (null). */
  | { kind: "repair-script-settings"; scriptId: string | null }
  | null;

export interface ContextMenuState {
  x: number; // client px
  y: number;
  /** World point under the cursor when the menu opened on a part, used by "Centre view here". */
  point: Vec3 | null;
  /** In repair mode: the working copy's triangle under the cursor (the repair context menu's "this triangle"). */
  triangle?: number | null;
}

export type PanelSection = "clip" | "cuts" | "information";

/**
 * A one-shot view tool: from Zoom to, the next left drag zooms into its rectangle or the next click centres the view;
 * with Arrange › Lay a face on the platform, the next click on a part lays that face on the platform.
 */
export type ViewTool = "zoom-area" | "zoom-point" | "lay-flat";

/** Long-running work shown in the status bar (imports, saves). */
export interface Task {
  id: number;
  label: string;
  /** 0..1, or null when the amount of work is unknown. */
  progress: number | null;
}

interface UiState {
  toasts: ToastItem[];
  dialog: Dialog;
  contextMenu: ContextMenuState | null;
  /** Bumped each time a right-panel section is asked to take focus. */
  focus: { section: PanelSection; nonce: number } | null;
  /** Hint for the hovered ribbon button; the status bar shows it instead of the tool hint. */
  hoverHint: string | null;
  tasks: Task[];
  /** Files are being dragged over the window; the viewport shows a drop hint. */
  dropActive: boolean;
  /** The ribbon's selected tab. Repair mode and Boolean operations select their contextual tabs. */
  ribbonTab: TabId;
  /** Zoom to › Area or Point waiting for its drag or click; null otherwise. Esc drops it. */
  viewTool: ViewTool | null;

  dismissToast(id: number): void;
  openDialog(dialog: Exclude<Dialog, null>): void;
  closeDialog(): void;
  openContextMenu(menu: ContextMenuState): void;
  closeContextMenu(): void;
  focusSection(section: PanelSection): void;
  setHoverHint(hint: string | null): void;
  setDropActive(active: boolean): void;
  setRibbonTab(tab: TabId): void;
  setViewTool(tool: ViewTool | null): void;
}

/** A prompt closed or replaced by anything but its own answer counts as cancelled; a second resolve is a no-op. */
function cancelPrompt(dialog: Dialog) {
  if (dialog && "resolve" in dialog) dialog.resolve(null);
}

export const useUi = create<UiState>()((set, get) => ({
  toasts: [],
  dialog: null,
  contextMenu: null,
  focus: null,
  hoverHint: null,
  tasks: [],
  dropActive: false,
  ribbonTab: "modify",
  viewTool: null,

  dismissToast(id) {
    set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) }));
  },
  openDialog(dialog) {
    const current = get().dialog;
    set({ dialog, contextMenu: null });
    if (current !== dialog) cancelPrompt(current);
  },
  closeDialog() {
    const current = get().dialog;
    set({ dialog: null });
    cancelPrompt(current);
  },
  openContextMenu(menu) {
    set({ contextMenu: menu });
  },
  closeContextMenu() {
    set({ contextMenu: null });
  },
  focusSection(section) {
    set((s) => ({ focus: { section, nonce: (s.focus?.nonce ?? 0) + 1 } }));
  },
  setHoverHint(hint) {
    set({ hoverHint: hint });
  },
  setDropActive(active) {
    set({ dropActive: active });
  },
  setRibbonTab(tab) {
    set({ ribbonTab: tab, hoverHint: null });
  },
  setViewTool(tool) {
    set({ viewTool: tool });
  },
}));

let toastSeq = 0;
const TOAST_MS = 3500;
const MAX_TOASTS = 4;

/** Shows a short message at the bottom of the window. Errors stay up longer so they can be read. */
export function toast(message: string, kind: ToastKind = "info") {
  const id = ++toastSeq;
  useUi.setState((s) => ({ toasts: [...s.toasts, { id, message, kind }].slice(-MAX_TOASTS) }));
  window.setTimeout(() => useUi.getState().dismissToast(id), kind === "error" ? TOAST_MS * 2 : TOAST_MS);
}

let taskSeq = 0;

export interface TaskHandle {
  /** 0..1, or null when unknown. */
  progress(fraction: number | null): void;
  end(): void;
}

/** Shows `label` with a busy indicator in the status bar until end() is called. */
export function startTask(label: string): TaskHandle {
  const id = ++taskSeq;
  useUi.setState((s) => ({ tasks: [...s.tasks, { id, label, progress: null }] }));
  return {
    progress(fraction) {
      useUi.setState((s) => ({ tasks: s.tasks.map((t) => (t.id === id ? { ...t, progress: fraction } : t)) }));
    },
    end() {
      useUi.setState((s) => ({ tasks: s.tasks.filter((t) => t.id !== id) }));
    },
  };
}

type PromptDialog<T> = (resolve: (answer: T | null) => void) => Exclude<Dialog, null>;

let promptQueue: Promise<unknown> = Promise.resolve();

/**
 * Resolves once no dialog is open. Another waiter may open one before the caller runs again, so a caller that then
 * opens a dialog checks the slot once more first, and waits again while it is taken.
 */
export function dialogSlotFree(): Promise<void> {
  if (useUi.getState().dialog === null) return Promise.resolve();
  return new Promise((resolve) => {
    const unsubscribe = useUi.subscribe((s) => {
      if (s.dialog !== null) return;
      unsubscribe();
      resolve();
    });
  });
}

/**
 * Opens a dialog that answers through its `resolve` and waits for the answer (null if cancelled).
 * Prompts queue: each waits for the previous one, and for any other open dialog, to close first.
 * The dialog component must call resolve exactly once and then closeDialog().
 */
export function prompt<T>(make: PromptDialog<T>): Promise<T | null> {
  const run = promptQueue.then(async () => {
    while (useUi.getState().dialog !== null) await dialogSlotFree();
    return new Promise<T | null>((resolve) => useUi.getState().openDialog(make(resolve)));
  });
  promptQueue = run.catch(() => undefined);
  return run;
}
