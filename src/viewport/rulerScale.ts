import type { Bounds } from "../mesh";

/**
 * Pure maths for the screen-edge rulers (rulers.ts draws them). A ruler maps a screen position to a
 * label in mm: the reference point's coordinate on the measured world axis, plus the real distance
 * along the screen axis at the reference point's depth. In axis-aligned orthographic views the
 * screen axes are world axes, so the labels are exact world coordinates at every depth.
 */

/** A 4×4 matrix in column-major order, as three's Matrix4.elements. */
export type Mat4 = ArrayLike<number>;
export type Point = readonly [number, number, number];
export type Axis = 0 | 1 | 2;

/** What a ruler measures. */
export interface RulerAxis {
  /** World axis the labels show: 0 X, 1 Y, 2 Z. The one most aligned with the screen direction. */
  axis: Axis;
  /** +1 when the screen direction points towards +axis, -1 when towards -axis. */
  sign: 1 | -1;
  /** The screen direction in world space (unit): camera right for the bottom ruler, camera up for the left one. */
  dir: [number, number, number];
}

/** A ruler's screen-to-label mapping for one frame. */
export interface RulerMapping extends RulerAxis {
  /** Where the reference point projects, in CSS px along the ruler: x for the bottom ruler, y (down) for the left one. */
  origin: number;
  /** Label value at `origin`: the reference point's coordinate on `axis`. */
  value0: number;
  /** Screen px per real mm at the reference point's depth. */
  pxPerMm: number;
  /** Change in label value (mm) per px of screen position along the ruler; negative when labels fall that way. */
  slope: number;
}

export interface RulerMappings {
  horizontal: RulerMapping;
  vertical: RulerMapping;
}

export interface ScreenAxes {
  horizontal: RulerAxis;
  vertical: RulerAxis;
}

/** The axes the rulers measured in the previous frame. */
export interface HeldAxes {
  horizontal: Axis;
  vertical: Axis;
}

/** Each ruler's preferred axis first: X along the bottom, Z up the left. */
const HORIZONTAL_ORDER: readonly Axis[] = [0, 1, 2];
const VERTICAL_ORDER: readonly Axis[] = [2, 1, 0];
const TIE = 1e-9;
/**
 * How much more aligned with the screen direction (difference of unit-vector components) a less
 * preferred axis must be to take a ruler over: about 4° of orbit past the 45° line.
 */
const SWITCH_MARGIN = 0.1;

/**
 * The world axes the bottom (screen right) and left (screen up) rulers measure, from the camera's
 * view matrix. `held` is what they measured last frame (see rulerAxis); without it each ruler
 * starts from its preferred axis.
 */
export function screenAxes(view: Mat4, held?: HeldAxes): ScreenAxes {
  // Rows of the view matrix's rotation are the camera's right, up and back vectors in world space.
  const horizontal = rulerAxis(view[0], view[4], view[8], HORIZONTAL_ORDER, held?.horizontal);
  // The two rulers never measure the same axis: their switch margins differ when the view is tilted,
  // so on its own the left ruler could still hold the axis the bottom one has just taken.
  const order = VERTICAL_ORDER.filter((a) => a !== horizontal.axis);
  const vertical = rulerAxis(view[1], view[5], view[9], order, held?.vertical === horizontal.axis ? undefined : held?.vertical);
  return { horizontal, vertical };
}

/**
 * The axis a ruler measures, with hysteresis so a near-tie never flips it (a flip moves every label
 * by the difference of the reference's coordinates). From the held axis, a more preferred axis takes
 * over once it is at least as aligned (ties included); a less preferred one only once it is aligned
 * by SWITCH_MARGIN more. So the iso view, where screen right is exactly X + Y, always measures X, a
 * small orbit from it keeps X, and an orbit across 45° switches only when clearly past.
 */
function rulerAxis(x: number, y: number, z: number, order: readonly Axis[], held: Axis | undefined): RulerAxis {
  const length = Math.hypot(x, y, z) || 1;
  const dir: [number, number, number] = [x / length, y / length, z / length];
  const current = held ?? order[0];
  const rank = order.indexOf(current);
  let axis = current;
  for (let i = 0; i < order.length; i++) {
    const a = order[i];
    if (a === current) continue;
    const bar = i < rank ? -TIE : SWITCH_MARGIN;
    if (!(Math.abs(dir[a]) > Math.abs(dir[current]) + bar)) continue;
    // Of several axes that qualify, the most aligned wins; ties go to the earlier one in `order`.
    if (axis === current || Math.abs(dir[a]) > Math.abs(dir[axis]) + TIE) axis = a;
  }
  return { axis, sign: dir[axis] < 0 ? -1 : 1, dir };
}

/**
 * Both rulers' mappings for a camera (view = matrixWorldInverse, projection = projectionMatrix) and a
 * viewport of width × height CSS px. The scale is measured by projecting the reference point and
 * the points 1 mm from it along camera right and camera up, so it holds for perspective (at the
 * reference depth) and orthographic cameras alike. Null when the reference is not in front of the
 * camera, or (perspective) is nearer than `minDepth`, which the caller sets to the near plane.
 */
export function rulerMappings(
  view: Mat4,
  projection: Mat4,
  width: number,
  height: number,
  reference: Point,
  axes: ScreenAxes = screenAxes(view),
  minDepth = 0,
): RulerMappings | null {
  if (!(width > 0) || !(height > 0)) return null;
  const { horizontal, vertical } = axes;
  const [rx, ry, rz] = reference;
  // Clip w is the depth in front of a perspective camera (and 1 for an orthographic one).
  const minW = Math.max(minDepth, 1e-9);
  const centre = toScreen(view, projection, rx, ry, rz, width, height, minW);
  if (!centre) return null;
  // Camera right and up keep the depth, so these pass the same depth test.
  const right = toScreen(view, projection, rx + horizontal.dir[0], ry + horizontal.dir[1], rz + horizontal.dir[2], width, height, minW);
  const up = toScreen(view, projection, rx + vertical.dir[0], ry + vertical.dir[1], rz + vertical.dir[2], width, height, minW);
  if (!right || !up) return null;
  const pxH = Math.hypot(right[0] - centre[0], right[1] - centre[1]);
  const pxV = Math.hypot(up[0] - centre[0], up[1] - centre[1]);
  if (!(pxH > 0 && pxV > 0 && Number.isFinite(pxH) && Number.isFinite(pxV))) return null;
  return {
    horizontal: { ...horizontal, origin: centre[0], value0: reference[horizontal.axis], pxPerMm: pxH, slope: horizontal.sign / pxH },
    // Screen y grows downwards while `dir` is camera up, hence the minus.
    vertical: { ...vertical, origin: centre[1], value0: reference[vertical.axis], pxPerMm: pxV, slope: -vertical.sign / pxV },
  };
}

/** Screen position (CSS px, y down) of a world point, or null when its clip w is not above `minW`. */
function toScreen(view: Mat4, p: Mat4, x: number, y: number, z: number, width: number, height: number, minW: number): [number, number] | null {
  const cx = view[0] * x + view[4] * y + view[8] * z + view[12];
  const cy = view[1] * x + view[5] * y + view[9] * z + view[13];
  const cz = view[2] * x + view[6] * y + view[10] * z + view[14];
  const cw = view[3] * x + view[7] * y + view[11] * z + view[15];
  const clipX = p[0] * cx + p[4] * cy + p[8] * cz + p[12] * cw;
  const clipY = p[1] * cx + p[5] * cy + p[9] * cz + p[13] * cw;
  const clipW = p[3] * cx + p[7] * cy + p[11] * cz + p[15] * cw;
  if (!(clipW > minW)) return null;
  return [((clipX / clipW + 1) / 2) * width, ((1 - clipY / clipW) / 2) * height];
}

/**
 * The point straight ahead of the camera at `reference`'s distance from it, but at least
 * `minDistance`: a stand-in reference when the real one is behind the camera.
 */
export function straightAhead(view: Mat4, reference: Point, minDistance: number): [number, number, number] {
  // The view matrix is [Rᵀ | −Rᵀp]: the camera sits at p = −R·t, and looks along minus its third row.
  const t0 = view[12];
  const t1 = view[13];
  const t2 = view[14];
  const px = -(view[0] * t0 + view[1] * t1 + view[2] * t2);
  const py = -(view[4] * t0 + view[5] * t1 + view[6] * t2);
  const pz = -(view[8] * t0 + view[9] * t1 + view[10] * t2);
  const distance = Math.max(Math.hypot(reference[0] - px, reference[1] - py, reference[2] - pz), minDistance);
  const back = Math.hypot(view[2], view[6], view[10]) || 1;
  const k = distance / back;
  return [px - view[2] * k, py - view[6] * k, pz - view[10] * k];
}

/** Label value (mm) at screen position `s` along the ruler. */
export function valueAt(m: RulerMapping, s: number): number {
  return m.value0 + m.slope * (s - m.origin);
}

/** Screen position along the ruler of label value `v`. */
export function screenAt(m: RulerMapping, v: number): number {
  return m.origin + (v - m.value0) / m.slope;
}

/**
 * Label values spanned by `box` on this ruler: the box's extent along the screen direction,
 * measured from the reference point and so at the reference depth. [min, max].
 */
export function boxRange(m: RulerMapping, reference: Point, box: Bounds): [number, number] {
  let lo = 0;
  let hi = 0;
  for (let a = 0; a < 3; a++) {
    const p = (box.min[a] - reference[a]) * m.dir[a];
    const q = (box.max[a] - reference[a]) * m.dir[a];
    lo += Math.min(p, q);
    hi += Math.max(p, q);
  }
  return m.sign > 0 ? [m.value0 + lo, m.value0 + hi] : [m.value0 - hi, m.value0 - lo];
}

export interface TickLayout {
  /** Distance between labelled (major) ticks, mm: 1, 2 or 5 × a power of ten. */
  step: number;
  /** Decimals the labels need at this step (trailing zeros are still dropped). */
  decimals: number;
  /** Minor intervals per major step; 1 means no minor ticks. */
  parts: number;
}

const MANTISSAS = [1, 2, 5] as const;
/** Minor subdivisions to try per mantissa, finest first. */
const SUBDIVISIONS: Record<(typeof MANTISSAS)[number], readonly number[]> = { 1: [10, 5, 2], 2: [4, 2], 5: [5] };
const MAX_CANDIDATES = 60;

/**
 * The smallest step in the 1-2-5 sequence whose labels, at `pxPerMm`, are at least the widest label
 * in [from, to] plus `gap` px apart, so centred labels never touch. `measure` returns a label's
 * width in px along the ruler. Minor ticks subdivide the step only while they stay `minMinorPx` apart.
 */
export function chooseTicks(
  pxPerMm: number,
  from: number,
  to: number,
  measure: (label: string) => number,
  gap: number,
  minMinorPx: number,
): TickLayout {
  const lo = Math.min(from, to);
  const hi = Math.max(from, to);
  // No step below gap / pxPerMm can fit, so start at the decade holding it.
  const firstExp = Math.floor(Math.log10(gap / pxPerMm));
  let step = 1;
  let decimals = 0;
  let mantissa: (typeof MANTISSAS)[number] = 1;
  for (let i = 0; i < MAX_CANDIDATES; i++) {
    mantissa = MANTISSAS[i % 3];
    const exp = firstExp + Math.floor(i / 3);
    // Dividing by an exact power of ten gives the closest double to 0.1, 0.2, 0.05...
    step = exp >= 0 ? mantissa * 10 ** exp : mantissa / 10 ** -exp;
    decimals = Math.min(20, Math.max(0, -exp));
    if (step * pxPerMm >= widestLabel(lo, hi, step, decimals, measure) + gap) break;
  }
  const spacing = step * pxPerMm;
  const parts = SUBDIVISIONS[mantissa].find((n) => spacing / n >= minMinorPx) ?? 1;
  return { step, decimals, parts };
}

/**
 * Widest label among the ticks in [lo, hi]. Digit count grows with magnitude, so it is at an end;
 * with trailing zeros dropped the tick next to an end can be wider ("99.5" beside "100").
 */
function widestLabel(lo: number, hi: number, step: number, decimals: number, measure: (label: string) => number): number {
  const first = Math.ceil(lo / step);
  const last = Math.floor(hi / step);
  if (first > last) return 0;
  let widest = 0;
  for (const k of [first, Math.min(first + 1, last), Math.max(last - 1, first), last]) {
    widest = Math.max(widest, measure(formatValue(k * step, decimals)));
  }
  return widest;
}

/** Tick at minor index `j` (value j × step / parts): 0 major, 1 mid (half a major step), 2 minor. */
export function tickKind(j: number, parts: number): 0 | 1 | 2 {
  const r = ((j % parts) + parts) % parts;
  if (r === 0) return 0;
  return parts % 2 === 0 && r === parts / 2 ? 1 : 2;
}

/** A ruler label: `decimals` places with trailing zeros dropped, and a true minus sign. Never "-0". */
export function formatValue(value: number, decimals: number): string {
  let text = value.toFixed(decimals);
  if (text.includes(".")) text = text.replace(/\.?0+$/, "");
  if (/^-0$/.test(text)) return "0";
  return text.startsWith("-") ? `−${text.slice(1)}` : text;
}
