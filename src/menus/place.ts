export interface Rect {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

export interface Size {
  width: number;
  height: number;
}

/** Gap kept between a menu and the window edge. */
const MARGIN = 4;

/**
 * Top-left corner for a menu of `size` beside `anchor`, kept inside a window of `view` size.
 * "below" drops down under the anchor (ribbon buttons); "right" opens to its right (submenus, and a
 * context menu, whose anchor is the clicked point). Either side flips when there is no room.
 */
export function placeMenu(anchor: Rect, size: Size, view: Size, side: "below" | "right"): { x: number; y: number } {
  if (side === "below") {
    return {
      x: slide(anchor.left, size.width, view.width),
      y: flip(anchor.bottom, anchor.top, size.height, view.height),
    };
  }
  return {
    x: flip(anchor.right, anchor.left, size.width, view.width),
    y: slide(anchor.top, size.height, view.height),
  };
}

/** Start at `start`, sliding back just enough for the far edge to fit. */
function slide(start: number, length: number, limit: number): number {
  return Math.max(MARGIN, Math.min(start, limit - MARGIN - length));
}

/** Open after `after` when it fits, else end at `before`, else pin to the window edge. */
function flip(after: number, before: number, length: number, limit: number): number {
  if (after + length <= limit - MARGIN) return after;
  if (before - length >= MARGIN) return before - length;
  return Math.max(MARGIN, limit - MARGIN - length);
}

export function rectOf(el: Element): Rect {
  const r = el.getBoundingClientRect();
  return { left: r.left, top: r.top, right: r.right, bottom: r.bottom };
}

export function pointRect(x: number, y: number): Rect {
  return { left: x, top: y, right: x, bottom: y };
}
