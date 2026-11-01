// Modify › Mirror and Shear: transforms of whole parts, like Scale, so they go into each part's matrix exactly (a
// mirror twice is the part as it was). Exports and the mesh tools bake the matrix in; a mirroring matrix turns the
// triangles round as it bakes (src/ops/bake.ts), so the surface keeps facing outwards.
import { centreOf, partBounds } from "../mesh";
import { multiply, scaleAbout, shearAbout, type Axis } from "../matrix";
import { useStore, type Part, type Vec3 } from "../store";
import { transformTargets } from "../transformTools";
import { toast, useUi } from "../ui";

/** About which point a part is mirrored or sheared: its own box centre, or the platform's origin. */
export type Pivot = "centre" | "origin";

export interface MirrorSettings {
  axis: Axis;
  pivot: Pivot;
}

export interface ShearSettings {
  /** The axis points move along. */
  along: Axis;
  /** The axis whose distance decides how far. */
  by: Axis;
  /** Degrees the lines along `by` lean towards `along`, −80 to 80. */
  angle: number;
  pivot: Pivot;
}

export const MAX_SHEAR_ANGLE = 80;

function pivotOf(part: Part, pivot: Pivot): Vec3 {
  const box = partBounds(part);
  return pivot === "origin" || !box ? [0, 0, 0] : centreOf(box);
}

/** Each part's new matrix mirrored across the plane through its pivot at right angles to `axis`. */
export function mirrorMatrices(parts: readonly Part[], { axis, pivot }: MirrorSettings): Map<string, number[]> {
  const scale: Vec3 = [1, 1, 1];
  scale[axis] = -1;
  return new Map(parts.map((p) => [p.id, multiply(scaleAbout(scale, pivotOf(p, pivot)), p.matrix)]));
}

/** Each part's new matrix sheared about its pivot. */
export function shearMatrices(parts: readonly Part[], { along, by, angle, pivot }: ShearSettings): Map<string, number[]> {
  const factor = Math.tan((Math.max(-MAX_SHEAR_ANGLE, Math.min(MAX_SHEAR_ANGLE, angle)) * Math.PI) / 180);
  return new Map(parts.map((p) => [p.id, multiply(shearAbout(along, by, factor, pivotOf(p, pivot)), p.matrix)]));
}

const AXES = ["X", "Y", "Z"];

/** The selection's shown, unlocked parts, or null after saying why there are none. */
function targets(verb: string): Part[] | null {
  const t = transformTargets(useStore.getState());
  if (t.parts.length) return t.parts;
  toast(t.locked || t.hidden ? `Hidden or locked parts cannot be ${verb}.` : `Select the parts to ${verb === "mirrored" ? "mirror" : "shear"} first.`);
  return null;
}

export function openMirror(): void {
  if (targets("mirrored")) useUi.getState().openDialog({ kind: "mirror" });
}

export function openShear(): void {
  if (targets("sheared")) useUi.getState().openDialog({ kind: "shear" });
}

/** Mirrors the selected parts as one undo step. Returns how many changed. */
export function mirrorParts(settings: MirrorSettings): number {
  const parts = targets("mirrored");
  if (!parts) return 0;
  useStore.getState().setPartMatrices(mirrorMatrices(parts, settings), `Mirror in ${AXES[settings.axis]}`);
  return parts.length;
}

/** Shears the selected parts as one undo step. Returns how many changed. */
export function shearParts(settings: ShearSettings): number {
  const parts = targets("sheared");
  if (!parts) return 0;
  if (settings.along === settings.by) {
    toast("Pick two different axes.");
    return 0;
  }
  if (settings.angle === 0) return 0;
  useStore.getState().setPartMatrices(shearMatrices(parts, settings), "Shear");
  return parts.length;
}
