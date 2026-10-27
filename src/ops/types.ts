// Shapes shared by the mesh operations (Cut, Split shells, Merge parts), their dialogs and the worker.
import type { CutScope } from "../clip";
import type { Vec3 } from "../store";

/** A cutting plane in world mm: the points p with dot(normal, p) = offset. `normal` is unit length. */
export interface CutPlane {
  normal: Vec3;
  offset: number;
}

/** What the Cut dialog's Execute runs. */
export interface CutOptions {
  plane: CutPlane;
  scope: CutScope;
  /** Close cut surfaces: cap each piece so it is a closed solid. */
  cap: boolean;
  /** mm the "(cut 2)" piece is moved along the normal; 0 leaves the pieces in place. */
  gap: number;
}
