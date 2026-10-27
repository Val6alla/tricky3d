// The plane the open Cut dialog would cut with, drawn by the viewport as a translucent quad
// (viewport/planePreview.ts). View state only: not undoable, not saved.
import { create } from "zustand";
import type { Bounds } from "./mesh";
import type { CutPlane } from "./ops/types";

export interface CutPreviewState {
  /** Null while no Cut dialog is open or its fields do not give a plane yet. */
  plane: CutPlane | null;
  /**
   * World box the quad is centred on and sized to: the parts the cut acts on, else the scene. Kept here
   * rather than worked out by the viewport so the quad covers exactly the box the dialog's default
   * position came from.
   */
  box: Bounds | null;
  /** Shows the quad for `plane` across `box`; null for either hides it. */
  setPlane(plane: CutPlane | null, box?: Bounds | null): void;
}

export const useCutPreview = create<CutPreviewState>()((set) => ({
  plane: null,
  box: null,
  setPlane(plane, box = null) {
    set(plane && box ? { plane, box } : { plane: null, box: null });
  },
}));
