import { OrthographicCamera, type Vector3 } from "three";
import type { Bounds } from "../mesh";
import type { ViewCamera } from "./cameras";
import {
  boxRange,
  chooseTicks,
  formatValue,
  rulerMappings,
  screenAt,
  screenAxes,
  straightAhead,
  tickKind,
  valueAt,
  type HeldAxes,
  type RulerMapping,
} from "./rulerScale";
import { axisColor, type VpTheme } from "./theme";
import "./rulers.css";

export interface Rulers {
  /**
   * Redraws the rulers for the frame just rendered. `reference` is the world point (mm) whose depth
   * sets the scale; `selection` is the selected parts' world box, or null. `fallback` (the orbit
   * target) sets the depth instead while a perspective camera has moved past `reference`.
   */
  draw(camera: ViewCamera, width: number, height: number, reference: Vector3, selection: Bounds | null, fallback?: Vector3): void;
  setVisible(visible: boolean): void;
  applyTheme(theme: VpTheme): void;
  dispose(): void;
}

/** Width of the left ruler and height of the bottom one; the corner square is this size too. CSS px. */
export const RULER_THICKNESS = 24;
const FONT_PX = 10;
/** Tick lengths from the ruler's inner edge, CSS px. */
const MAJOR_TICK = 8;
const MID_TICK = 5;
const MINOR_TICK = 3;
/** Clear space between neighbouring labels, CSS px. */
const LABEL_GAP = 10;
const MIN_MINOR_PX = 5;
/** Space a label keeps from the ruler's far end and from the corner square, CSS px. */
const END_GAP = 3;
/** Labels are centred across the ruler in the space beyond the major ticks, this far from the inner edge. CSS px. */
const LABEL_CENTRE = (MAJOR_TICK + RULER_THICKNESS) / 2;
/** Corner letters, centred at these offsets from the corner's top-left, CSS px: the left ruler's above the bottom ruler's. */
const VERTICAL_LETTER = [9, 8.5] as const;
const HORIZONTAL_LETTER = [16, 16.5] as const;
const BACKGROUND_ALPHA = 0.86;
const BAND_ALPHA = 0.24;
/** More ticks than this per ruler means a degenerate scale; draw none rather than stall. */
const MAX_TICKS = 4000;
const LETTERS = ["X", "Y", "Z"];

/** Text metrics for one font, device px. */
interface FontMetrics {
  /** Height of a digit above the baseline, to centre text across the ruler. */
  ascent: number;
  /** The widest digit's advance. Every digit gets it, so figures are tabular whatever the font's own digits are. */
  digit: number;
  /** Advances of the other characters met so far ('.', '−'), by char code. */
  others: Map<number, number>;
}

/**
 * Millimetre rulers along the left (screen up) and bottom (screen right) edges of the viewport,
 * on one 2D canvas inside `host`. The scale is real mm at the reference point's depth (see
 * rulerScale.ts); the selection's extent shows as a band. Drawn in device pixels so ticks stay crisp.
 */
export function createRulers(host: HTMLElement, theme: VpTheme): Rulers {
  const canvas = document.createElement("canvas");
  canvas.className = "viewport-rulers";
  canvas.setAttribute("aria-hidden", "true");
  // Directly above the WebGL canvas, so the view cube's overlay and the drop hint stay on top.
  const glCanvas = host.querySelector(":scope > canvas");
  if (glCanvas) glCanvas.after(canvas);
  else host.prepend(canvas);
  const context = canvas.getContext("2d");
  if (!context) {
    // Not expected in a webview; without a 2D context there are simply no rulers.
    canvas.remove();
    return { draw() {}, setVisible() {}, applyTheme() {}, dispose() {} };
  }
  const ctx: CanvasRenderingContext2D = context;

  let current = theme;
  let visible = true;
  const reference: [number, number, number] = [0, 0, 0];
  /** The axes measured last frame, so a near-tie keeps its axis (see rulerScale's rulerAxis). */
  const held: HeldAxes = { horizontal: 0, vertical: 2 };

  // Per-frame layout, in device px: canvas size, ruler thickness, line width and the CSS-to-device scale.
  let bw = 0;
  let bh = 0;
  let t = 0;
  let lw = 1;
  let scale = 1;
  const fonts = new Map<string, FontMetrics>();
  let metrics: FontMetrics = { ascent: 0, digit: 0, others: new Map() };

  function setFont(font: string) {
    ctx.font = font;
    let m = fonts.get(font);
    if (!m) {
      let digit = 0;
      for (let code = 48; code <= 57; code++) digit = Math.max(digit, ctx.measureText(String.fromCharCode(code)).width);
      m = { ascent: ctx.measureText("0").actualBoundingBoxAscent, digit, others: new Map() };
      fonts.set(font, m);
    }
    metrics = m;
  }

  /** Advance of one character in the current font, device px; digits all get the widest digit's. */
  function advance(code: number): number {
    if (code >= 48 && code <= 57) return metrics.digit;
    let width = metrics.others.get(code);
    if (width === undefined) {
      width = ctx.measureText(String.fromCharCode(code)).width;
      metrics.others.set(code, width);
    }
    return width;
  }

  /** A label's width along the ruler in device px, as drawText lays it out. */
  function textWidth(label: string): number {
    let width = 0;
    for (let i = 0; i < label.length; i++) width += advance(label.charCodeAt(i));
    return width;
  }

  /** A label's width along the ruler in CSS px, with the label font set. */
  const measure = (label: string) => textWidth(label) / scale;

  /**
   * Draws `label` centred on x (device px, in the current transform) one character at a time,
   * so digits sit in equal cells: canvas text has no font-variant-numeric. textAlign is "center".
   */
  function drawText(label: string, x: number, y: number) {
    let left = x - textWidth(label) / 2;
    for (let i = 0; i < label.length; i++) {
      const a = advance(label.charCodeAt(i));
      ctx.fillText(label[i], left + a / 2, y);
      left += a;
    }
  }

  /** Fills a rectangle given along the ruler (device px) and across it, measured from the inner edge (device px). */
  function fill(vertical: boolean, along: number, length: number, from: number, to: number) {
    if (vertical) ctx.fillRect(t - to, along, to - from, length);
    else ctx.fillRect(along, bh - t + from, length, to - from);
  }

  /** A line across the ruler at CSS position `s`, snapped to device pixels. */
  function line(vertical: boolean, s: number, across: number) {
    fill(vertical, Math.round(s * scale - lw / 2), lw, 0, across);
  }

  /** One ruler. `start`..`end` is its extent along the screen in CSS px. */
  function drawRuler(m: RulerMapping, vertical: boolean, start: number, end: number, selection: Bounds | null) {
    const a = valueAt(m, start);
    const b = valueAt(m, end);
    const lo = Math.min(a, b);
    const hi = Math.max(a, b);

    // The selection's extent: a tinted band under the ticks, and its two ends as ticks of their own.
    let s0 = NaN;
    let s1 = NaN;
    if (selection) {
      const [v0, v1] = boxRange(m, reference, selection);
      s0 = screenAt(m, v0);
      s1 = screenAt(m, v1);
      const from = Math.max(Math.min(s0, s1), start);
      const to = Math.min(Math.max(s0, s1), end);
      if (to > from) {
        ctx.fillStyle = current.selected;
        ctx.globalAlpha = BAND_ALPHA;
        fill(vertical, from * scale, (to - from) * scale, 0, t);
        ctx.globalAlpha = 1;
      }
    }

    const { step, decimals, parts } = chooseTicks(m.pxPerMm, lo, hi, measure, LABEL_GAP, MIN_MINOR_PX);
    const minor = step / parts;
    const firstTick = Math.ceil(lo / minor);
    const lastTick = Math.floor(hi / minor);
    if (lastTick - firstTick > MAX_TICKS) return;
    ctx.fillStyle = current.textMuted;
    for (let j = firstTick; j <= lastTick; j++) {
      const kind = tickKind(j, parts);
      line(vertical, screenAt(m, j * minor), (kind === 0 ? MAJOR_TICK : kind === 1 ? MID_TICK : MINOR_TICK) * scale);
    }
    // Over the ticks, so an end that sits on its coordinate's tick shows as that tick turning orange.
    // Major-tick length keeps the marker clear of the label centred on the same spot.
    ctx.fillStyle = current.selected;
    if (s0 >= start && s0 <= end) line(vertical, s0, MAJOR_TICK * scale);
    if (s1 >= start && s1 <= end) line(vertical, s1, MAJOR_TICK * scale);

    // Centred on the tick; a label that would run past either end of the ruler is left out.
    ctx.fillStyle = current.text;
    const across = LABEL_CENTRE * scale - metrics.ascent / 2;
    for (let k = Math.ceil(lo / step), last = Math.floor(hi / step); k <= last; k++) {
      const value = k * step;
      const label = formatValue(value, decimals);
      const half = measure(label) / 2;
      const s = screenAt(m, value);
      if (s - half < start + END_GAP || s + half > end - END_GAP) continue;
      const p = Math.round(s * scale);
      if (vertical) {
        // Rotated to read bottom to top, glyph tops towards the outer edge.
        ctx.setTransform(0, -1, 1, 0, t - across, p);
        drawText(label, 0, 0);
      } else {
        drawText(label, p, bh - t + across + metrics.ascent);
      }
    }
    ctx.setTransform(1, 0, 0, 1, 0, 0);
  }

  function drawLetter(axis: number, [x, y]: readonly [number, number]) {
    ctx.fillStyle = axisColor(current, axis);
    ctx.fillText(LETTERS[axis], Math.round(x * scale), Math.round(bh - t + y * scale + metrics.ascent / 2));
  }

  return {
    draw(camera, width, height, ref, selection, fallback) {
      if (!visible) return;
      scale = window.devicePixelRatio || 1;
      const w = Math.round(width * scale);
      const h = Math.round(height * scale);
      if (canvas.width !== w || canvas.height !== h) {
        // Resizing clears the canvas (and resets the context; every draw sets what it uses).
        canvas.width = w;
        canvas.height = h;
      } else {
        ctx.clearRect(0, 0, w, h);
      }
      bw = w;
      bh = h;
      t = Math.round(RULER_THICKNESS * scale);
      lw = Math.max(1, Math.round(scale));
      if (bw <= t * 2 || bh <= t * 2) return;

      ctx.globalAlpha = BACKGROUND_ALPHA;
      ctx.fillStyle = current.surface;
      ctx.fillRect(0, 0, t, bh);
      ctx.fillRect(t, bh - t, bw - t, t);
      ctx.globalAlpha = 1;

      const view = camera.matrixWorldInverse.elements;
      const projection = camera.projectionMatrix.elements;
      const axes = screenAxes(view, held);
      held.horizontal = axes.horizontal.axis;
      held.vertical = axes.vertical.axis;
      // A perspective camera draws nothing nearer than its near plane, so a reference there has no useful depth.
      const near = camera instanceof OrthographicCamera ? 0 : camera.near;
      reference[0] = ref.x;
      reference[1] = ref.y;
      reference[2] = ref.z;
      let mappings = rulerMappings(view, projection, width, height, reference, axes, near);
      let band = selection;
      if (!mappings) {
        // Zoomed past the reference: measure at the orbit target (always ahead of the camera), else straight
        // ahead at the reference's distance. The selection is out of view then, so it gets no band.
        band = null;
        if (fallback) {
          reference[0] = fallback.x;
          reference[1] = fallback.y;
          reference[2] = fallback.z;
          mappings = rulerMappings(view, projection, width, height, reference, axes);
        }
        if (!mappings) mappings = rulerMappings(view, projection, width, height, straightAhead(view, reference, near), axes);
      }
      ctx.textAlign = "center";
      ctx.textBaseline = "alphabetic";
      setFont(`${FONT_PX * scale}px ${current.font}`);
      if (mappings) {
        drawRuler(mappings.horizontal, false, t / scale, width, band);
        drawRuler(mappings.vertical, true, 0, (bh - t) / scale, band);
      }

      // Inner edges, meeting at the corner square.
      ctx.fillStyle = current.borderStrong;
      ctx.fillRect(t - lw, 0, lw, bh - t + lw);
      ctx.fillRect(t - lw, bh - t, bw - t + lw, lw);

      // Which world axis each ruler measures; known even when there is no scale.
      setFont(`600 ${FONT_PX * scale}px ${current.font}`);
      drawLetter(axes.vertical.axis, VERTICAL_LETTER);
      drawLetter(axes.horizontal.axis, HORIZONTAL_LETTER);
    },

    setVisible(v) {
      visible = v;
      canvas.style.display = v ? "" : "none";
      // Cleared so showing it again never flashes the frame from before it was hidden.
      if (!v) ctx.clearRect(0, 0, canvas.width, canvas.height);
    },

    applyTheme(next) {
      // Picked up by the next draw; the engine requests one after a theme change.
      current = next;
      fonts.clear();
    },

    dispose() {
      canvas.width = 0;
      canvas.height = 0;
      canvas.remove();
    },
  };
}
