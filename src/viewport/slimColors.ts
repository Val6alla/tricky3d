// Manipulate mesh › Remesh › Show slim faces: a colour per triangle, the part's own colour except for triangles with an
// angle under the limit, yellow (near the limit) to red (no angle at all). The mask is the repair View tab's
// (slimMask); only the masked triangles get their smallest angle worked out for the tint.
import type { MeshData } from "../store";
import { slimMask } from "./repairLayer";

function hexRgb(hex: string): [number, number, number] {
  const m = /^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(hex);
  return m ? [parseInt(m[1], 16), parseInt(m[2], 16), parseInt(m[3], 16)] : [169, 177, 188];
}

/** The smallest angle of triangle `t`, degrees. */
function smallestAngle(positions: Float32Array, indices: Uint32Array, t: number): number {
  let least = 180;
  for (let k = 0; k < 3; k++) {
    const o = indices[t * 3 + k] * 3;
    const a = indices[t * 3 + ((k + 1) % 3)] * 3;
    const b = indices[t * 3 + ((k + 2) % 3)] * 3;
    const ux = positions[a] - positions[o], uy = positions[a + 1] - positions[o + 1], uz = positions[a + 2] - positions[o + 2];
    const vx = positions[b] - positions[o], vy = positions[b + 1] - positions[o + 1], vz = positions[b + 2] - positions[o + 2];
    const lu = Math.hypot(ux, uy, uz);
    const lv = Math.hypot(vx, vy, vz);
    if (lu === 0 || lv === 0) return 0;
    const cos = Math.max(-1, Math.min(1, (ux * vx + uy * vy + uz * vz) / (lu * lv)));
    least = Math.min(least, (Math.acos(cos) * 180) / Math.PI);
  }
  return least;
}

const cache = new WeakMap<MeshData, { key: string; colors: Uint8Array }>();

/** r, g, b bytes per triangle of `mesh` for Show slim faces at `degrees`. */
export function slimColors(mesh: MeshData, degrees: number, base: string): Uint8Array {
  const key = `${degrees}|${base}`;
  const hit = cache.get(mesh);
  if (hit?.key === key) return hit.colors;
  const mask = slimMask(mesh.positions, mesh.indices, degrees);
  const [r, g, b] = hexRgb(base);
  const colors = new Uint8Array(mask.length * 3);
  for (let t = 0; t < mask.length; t++) {
    if (mask[t]) {
      const share = degrees > 0 ? Math.min(1, smallestAngle(mesh.positions, mesh.indices, t) / degrees) : 0;
      colors.set([255, Math.round(220 * share), 0], t * 3);
    } else {
      colors.set([r, g, b], t * 3);
    }
  }
  cache.set(mesh, { key, colors });
  return colors;
}
