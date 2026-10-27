// The platform the window runs on, and the keys and gestures the status-bar hints name for it. macOS has its own:
// Control-click is the right click there, so Cmd toggles, and a trackpad has no middle button to pan with.

export const IS_MAC = typeof navigator !== "undefined" && /Mac/i.test(navigator.userAgent);

/** The key that toggles a click into the selection: Cmd on macOS, Ctrl elsewhere. */
export function toggleKey(isMac: boolean): string {
  return isMac ? "Cmd" : "Ctrl";
}

/** How to orbit, pan and zoom the view (src/viewport/controls.ts). */
export function navigationHint(isMac: boolean): string {
  return isMac
    ? "Two-finger drag or right-drag orbits, Option+drag or Shift+two-finger drag pans, pinch or scroll zooms."
    : "Right-drag orbits, middle-drag pans, scroll zooms.";
}

/** The Select tool's hint. */
export function selectHint(isMac: boolean): string {
  return `Click a part or drag a box from empty space to select, Shift adds, ${toggleKey(isMac)} toggles. ${navigationHint(isMac)}`;
}
