// Where a cut can come from: the enabled clip planes of the right panel, or an axis and position typed in
// the Cut dialog. Pure, so the dialog and the cut command agree on the plane without a viewport.
import { CLIP_AXES, formatMm, rowDirection, rowOffset, rowRange, type ClipAxis, type ClipSettings } from "../clip";
import type { Bounds } from "../mesh";
import type { CutPlane } from "./types";

export type AxisName = "x" | "y" | "z";

const AXIS_INDEX: Record<AxisName, number> = { x: 0, y: 1, z: 2 };

export interface PlaneChoice {
  axis: ClipAxis;
  /** "X plane at 12.50 mm", for the dialog's list. */
  label: string;
  plane: CutPlane;
}

const AXIS_LABEL: Record<ClipAxis, string> = { x: "X plane", y: "Y plane", z: "Z plane", custom: "Custom plane" };

/**
 * The enabled clip planes as cut planes, in panel order. Each uses the row's direction as its normal (the
 * Flip toggle only changes which side the viewport hides) and the offset the panel shows, so the cut lands
 * exactly where the slider says. `bounds` is the panel's range box (clip.ts sceneBounds).
 */
export function enabledCutPlanes(clip: Pick<ClipSettings, "rows" | "customNormal">, bounds: Bounds): PlaneChoice[] {
  const out: PlaneChoice[] = [];
  for (const axis of CLIP_AXES) {
    const row = clip.rows[axis];
    if (!row.enabled) continue;
    const offset = rowOffset(row, rowRange(axis, clip, bounds));
    const [x, y, z] = rowDirection(axis, clip.customNormal);
    out.push({ axis, label: `${AXIS_LABEL[axis]} at ${formatMm(offset)} mm`, plane: { normal: [x, y, z], offset } });
  }
  return out;
}

/** The plane across `axis` at `position` mm, normal along +axis. */
export function axisCutPlane(axis: AxisName, position: number): CutPlane {
  const normal: [number, number, number] = [0, 0, 0];
  normal[AXIS_INDEX[axis]] = 1;
  return { normal, offset: position };
}

/** The middle of `bounds` along `axis`, the Cut dialog's default position. */
export function centreAlong(axis: AxisName, bounds: Bounds): number {
  const i = AXIS_INDEX[axis];
  return (bounds.min[i] + bounds.max[i]) / 2;
}
