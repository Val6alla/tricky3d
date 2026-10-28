import { useRef, useState, type KeyboardEvent, type PointerEvent } from "react";

/** Size rules for a resizable side panel. Widths are in CSS px. */
export interface PanelSize {
  storageKey: string;
  min: number;
  max: number;
  initial: number;
}

const KEY_STEP = 16;

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

/** The width stored for this panel, kept within its limits; the initial width when none is stored or storage fails. */
export function loadWidth(size: PanelSize): number {
  try {
    const raw = localStorage.getItem(size.storageKey);
    const n = raw === null ? NaN : Number(raw);
    return Number.isFinite(n) ? clamp(n, size.min, size.max) : size.initial;
  } catch {
    return size.initial;
  }
}

function saveWidth(key: string, width: number) {
  try {
    localStorage.setItem(key, String(Math.round(width)));
  } catch {
    // Storage can be unavailable; the width then lasts for this session only.
  }
}

/** The width after dragging the handle `dx` px (rightwards positive) from a width of `start`, within the limits. */
export function dragWidth(edge: SplitterProps["edge"], size: PanelSize, start: number, dx: number): number {
  return clamp(edge === "right" ? start + dx : start - dx, size.min, size.max);
}

/** The width after an arrow key on the handle: a step wider towards the outside of the panel, narrower inwards. */
export function keyWidth(edge: SplitterProps["edge"], size: PanelSize, width: number, key: "ArrowLeft" | "ArrowRight"): number {
  return dragWidth(edge, size, width, key === "ArrowRight" ? KEY_STEP : -KEY_STEP);
}

/** Panel width state, restored from localStorage. The Splitter saves it. */
export function usePanelWidth(size: PanelSize) {
  return useState(() => loadWidth(size));
}

interface SplitterProps {
  /** Edge of the panel the handle sits on. Dragging away from the panel widens it. */
  edge: "left" | "right";
  size: PanelSize;
  width: number;
  onResize(width: number): void;
  label: string;
}

/**
 * Drag handle on a panel edge. Its parent element must be the panel.
 * The viewport's CSS min-width, not this handle, keeps it usable: panels shrink towards their min-width when space runs out.
 */
export function Splitter({ edge, size, width, onResize, label }: SplitterProps) {
  const drag = useRef<{ x: number; start: number; last: number } | null>(null);
  const [dragging, setDragging] = useState(false);

  // Live updates during a drag skip storage; it is written once the width settles.
  function commit(w: number) {
    onResize(w);
    saveWidth(size.storageKey, w);
  }

  function onPointerDown(e: PointerEvent<HTMLDivElement>) {
    if (e.button !== 0) return;
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    drag.current = { x: e.clientX, start: width, last: width };
    setDragging(true);
  }

  function onPointerMove(e: PointerEvent<HTMLDivElement>) {
    const d = drag.current;
    if (!d) return;
    const dx = e.clientX - d.x;
    d.last = dragWidth(edge, size, d.start, dx);
    onResize(d.last);
  }

  function endDrag() {
    const d = drag.current;
    if (!d) return;
    drag.current = null;
    setDragging(false);
    if (d.last !== d.start) saveWidth(size.storageKey, d.last);
  }

  function onKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
    e.preventDefault();
    commit(keyWidth(edge, size, width, e.key));
  }

  return (
    <div
      className={`splitter splitter-${edge}${dragging ? " is-dragging" : ""}`}
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      aria-valuemin={size.min}
      aria-valuemax={size.max}
      aria-valuenow={Math.round(width)}
      tabIndex={0}
      title="Drag to resize. Double-click to reset."
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      onLostPointerCapture={endDrag}
      onDoubleClick={() => commit(size.initial)}
      onKeyDown={onKeyDown}
    />
  );
}
