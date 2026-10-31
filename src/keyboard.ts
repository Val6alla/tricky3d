import { closeAnalysisView, isAnalysisOpen } from "./analysis/session";
import { useEffect } from "react";
import { IS_MAC, blockedReason, featureById, runFeature } from "./features";
import { isMenuOpen } from "./menus/Menu";
import { dropPending } from "./measure/session";
import { isRepairing } from "./repair/session";
import { useStore, type GizmoMode } from "./store";
import { finishScale } from "./transformTools";
import { useUi } from "./ui";
import { isLabelOpen, labelKey } from "./label/session";

type KeyPress = Pick<KeyboardEvent, "key" | "code" | "ctrlKey" | "metaKey" | "shiftKey" | "altKey">;

/**
 * The pressed key in lower case. On a non-Latin layout (Cyrillic, Greek...) a letter key reports
 * its own script, so Ctrl+Z arrives as "я"; the physical key from `code` is used instead.
 */
export function keyOf(e: Pick<KeyboardEvent, "key" | "code">): string {
  const key = e.key.toLowerCase();
  const physical = /^Key([A-Z])$/.exec(e.code);
  if (physical && /^\p{L}$/u.test(key) && !/^[a-z]$/.test(key)) return physical[1].toLowerCase();
  return key;
}

/** Gizmo tools for W / E / R. Move and Rotate share the combined gizmo, so W and E both pick it. */
const GIZMO_KEYS: Record<string, GizmoMode> = { w: "combined", e: "combined", r: "scale" };

/**
 * What a key press asks for: a feature id, "escape", "gizmo.<mode>", or null when it is not a shortcut. The Ctrl/Cmd
 * shortcuts are the same in repair mode (Undo and Redo then step through the repair history); the plain keys act on
 * the working copy instead, see repairShortcut.
 */
export function shortcutFor(e: KeyPress, isMac: boolean, repairing = false): string | null {
  if (e.altKey) return null;
  const key = keyOf(e);
  // Cmd on macOS, Ctrl elsewhere; either is accepted everywhere.
  if (e.ctrlKey || e.metaKey) {
    if (key === "z") return e.shiftKey ? "modify.redo" : "modify.undo";
    if (e.shiftKey) return key === "g" ? "view.always-show-gizmo" : null;
    if (key === "y") return "modify.redo";
    if (key === "s") return "file.save";
    if (key === "e") return "file.export";
    if (key === "o") return "file.open";
    if (key === "c") return "context.copy";
    if (key === "v") return "context.paste";
    return null;
  }
  if (repairing) return repairShortcut(key, e.shiftKey, isMac);
  if (e.shiftKey) return null;
  if (key === "delete" || (isMac && key === "backspace")) return "context.delete";
  if (key === "f") return "modify.zoom-selected";
  if (key === "a") return "modify.zoom-parts";
  if (key === "escape") return "escape";
  if (key in GIZMO_KEYS) return `gizmo.${GIZMO_KEYS[key]}`;
  return null;
}

/**
 * Repair mode's plain keys: = (or +) and - grow and shrink the triangle selection, Del removes the selected triangles,
 * Esc clears the selection, F zooms to it (to the part when none is selected) and A to the part. W, E and R pick no
 * gizmo: the part stays where it is while it is repaired. "+" and "-" are characters that some layouts type with
 * Shift, so Shift is allowed for them and for "=".
 */
function repairShortcut(key: string, shift: boolean, isMac: boolean): string | null {
  if (key === "=" || key === "+") return "repair-ctx.expand";
  if (key === "-") return "repair-ctx.shrink";
  if (shift) return null;
  if (key === "delete" || (isMac && key === "backspace")) return "repair.remove-triangles";
  if (key === "escape") return "repair.select-none";
  if (key === "f") return "repair.zoom-selected";
  if (key === "a") return "repair.zoom-part";
  return null;
}

/**
 * Webview shortcuts that would reload the page (losing the scene), open find, or print. Ctrl+Shift+G
 * (find previous) is among them although it is also ours; see onKeyDown.
 * F12 and Ctrl+Shift+I are left alone so devtools still open in debug builds.
 */
export function isBrowserShortcut(e: KeyPress): boolean {
  const key = keyOf(e);
  if (key === "f5" || key === "f3") return true;
  // Not with Alt: Ctrl+Alt is AltGr on Windows, which types characters.
  return (e.ctrlKey || e.metaKey) && !e.altKey && ["r", "f", "g", "p"].includes(key);
}

const TEXT_INPUTS = ["text", "search", "url", "tel", "email", "password", "number"];

/** Text fields keep the native Cut/Copy/Paste menu; nothing else gets the webview's page menu. */
function isTextEntry(target: EventTarget | null): boolean {
  if (target instanceof HTMLInputElement) return TEXT_INPUTS.includes(target.type);
  return target instanceof HTMLTextAreaElement || (target instanceof HTMLElement && target.isContentEditable);
}

// Both run in the capture phase, so dialogs and inputs that stop events from bubbling cannot let these through.
function blockBrowserShortcut(e: KeyboardEvent) {
  if (isBrowserShortcut(e)) e.preventDefault();
}

function blockPageMenu(e: MouseEvent) {
  if (!isTextEntry(e.target)) e.preventDefault();
}

/**
 * True when the focused control uses this key itself: a text field takes typing, Delete, Esc and its
 * own Ctrl+Z (but not Save or Open, which mean nothing there), and a select takes plain letters for
 * type-ahead. Checkboxes and sliders use no shortcut key, so shortcuts keep working after a click on one.
 */
function controlKeepsKey(e: KeyboardEvent, action: string): boolean {
  if (isTextEntry(e.target)) return action !== "file.save" && action !== "file.open";
  if (e.target instanceof HTMLSelectElement) return e.key.length === 1 && !e.ctrlKey && !e.metaKey;
  return false;
}

function onKeyDown(e: KeyboardEvent) {
  // A control that used the key marks it handled. Browser shortcuts arrive marked too, by blockBrowserShortcut
  // in the capture phase before any control sees them, so for those the mark says nothing and ours still run.
  if ((e.defaultPrevented && !isBrowserShortcut(e)) || e.isComposing) return;
  // Dialogs and open menus (the context menu included) handle their own keys (Esc closes them).
  if (useUi.getState().dialog !== null || isMenuOpen()) return;

  // Esc first drops a Zoom to tool still waiting for its drag or click, then a measurement half made.
  if (e.key === "Escape" && useUi.getState().viewTool !== null) {
    e.preventDefault();
    useUi.getState().setViewTool(null);
    return;
  }
  if (e.key === "Escape" && dropPending()) {
    e.preventDefault();
    return;
  }
  // Label generation: Esc leaves Place label, then drops the selection; Delete removes the selected label.
  if (isLabelOpen() && !isTextEntry(e.target) && labelKey(e.key)) {
    e.preventDefault();
    return;
  }
  const action = shortcutFor(e, IS_MAC, isRepairing());
  if (!action || controlKeepsKey(e, action)) return;
  // A held toggle would flip on every auto-repeat. The other shortcuts set the same state again, stop at their
  // dialog, or are meant to repeat (undo, redo).
  if (e.repeat && action === "view.always-show-gizmo") return;
  const store = useStore.getState();
  if (action === "context.delete" && store.selection.length === 0) return;
  // Likewise Del, = and - in repair mode do nothing while no triangle is selected.
  const feature = featureById(action);
  if (feature && blockedReason(feature) !== null) return;
  e.preventDefault();
  if (action === "escape") {
    // Esc steps back one tool at a time: Scale ends as one scale does (finishScale), the move and rotate tool goes
    // to Select, and Select clears the selection.
    if (store.gizmoMode === "scale") finishScale();
    else if (store.gizmoMode) store.setGizmoMode(null);
    else if (isAnalysisOpen()) closeAnalysisView();
    else store.clearSelection();
  } else if (action.startsWith("gizmo.")) {
    store.setGizmoMode(action.slice("gizmo.".length) as GizmoMode);
  } else {
    runFeature(action);
  }
}

/** Global keyboard shortcuts, and the webview's own shortcuts and page menu switched off. Used once, in App. */
export function useKeyboardShortcuts() {
  useEffect(() => {
    window.addEventListener("keydown", blockBrowserShortcut, true);
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("contextmenu", blockPageMenu, true);
    return () => {
      window.removeEventListener("keydown", blockBrowserShortcut, true);
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("contextmenu", blockPageMenu, true);
    };
  }, []);
}
