import type { Bounds } from "../mesh";

/** Arm length as a share of the box's size along the arm. */
const ARM_SHARE = 0.22;
/** Arms are at least this share of the box's largest side, so flat or thin boxes keep visible corners. */
const MIN_ARM_SHARE = 0.05;
/**
 * Gap between the box and the brackets, as a share of the box's largest side, on every side. Box
 * corners usually lie on the part's own (selected, orange) edges, where an orange line would vanish.
 */
export const MARGIN_SHARE = 0.02;

/** 8 corners × 3 arms × 2 endpoints × xyz. */
export const BRACKET_FLOATS = 8 * 3 * 2 * 3;

/**
 * Corner brackets just outside `box`: at each of the 8 corners of the box grown by MARGIN_SHARE,
 * one segment along each of the 3 edges meeting there, pointing inward. An arm never passes the
 * middle of its edge, so arms from opposite corners can meet but never cross. Writes pairs of
 * points (x, y, z, x, y, z, ...) into `out`.
 */
export function bracketSegments(box: Bounds, out: Float32Array = new Float32Array(BRACKET_FLOATS)): Float32Array {
  const margin = Math.max(box.max[0] - box.min[0], box.max[1] - box.min[1], box.max[2] - box.min[2]) * MARGIN_SHARE;
  const min = box.min.map((v) => v - margin);
  const max = box.max.map((v) => v + margin);
  const size = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
  const floor = Math.max(...size) * MIN_ARM_SHARE;
  const arm = size.map((s) => Math.min(s / 2, Math.max(s * ARM_SHARE, floor)));
  let o = 0;
  for (let corner = 0; corner < 8; corner++) {
    const x = corner & 1 ? max[0] : min[0];
    const y = corner & 2 ? max[1] : min[1];
    const z = corner & 4 ? max[2] : min[2];
    for (let axis = 0; axis < 3; axis++) {
      // From a max corner the arm runs towards min, and the other way round.
      const step = corner & (1 << axis) ? -arm[axis] : arm[axis];
      out[o] = x;
      out[o + 1] = y;
      out[o + 2] = z;
      out[o + 3] = axis === 0 ? x + step : x;
      out[o + 4] = axis === 1 ? y + step : y;
      out[o + 5] = axis === 2 ? z + step : z;
      o += 6;
    }
  }
  return out;
}
