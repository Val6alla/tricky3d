import type { Bounds } from "../mesh";
import type { Vec3 } from "../store";

export type ZoomTarget = "everything" | "platform" | "parts" | "selected";
export type ViewPreset = "top" | "bottom" | "front" | "back" | "left" | "right" | "iso";

/** Camera commands the rest of the app can send to the viewport. */
export interface ViewportApi {
  zoomTo(target: ZoomTarget): void;
  /**
   * Fits a world box in the view, keeping the view direction: repair mode's Zoom to › Part and Selected, with the
   * working copy's or the selected triangles' box.
   */
  zoomToBounds(bounds: Bounds): void;
  setViewPreset(preset: ViewPreset): void;
  /** Re-targets the orbit centre on a world point, keeping the view direction. */
  centreOn(point: Vec3): void;
  /** Unit vector the camera looks along, in world space. */
  viewDirection(): Vec3;
  /** The part under a page point (clip planes respected), the world point hit and the face's normal there. */
  surfaceAt(clientX: number, clientY: number): { partId: string; point: Vec3; normal: Vec3 } | null;
  /** Mesh compare on show: the compared surface under the pointer and the distance there. */
  probeAt(clientX: number, clientY: number): { point: Vec3; value: number } | null;
  /** The world ray through a page point. */
  rayAt(clientX: number, clientY: number): { origin: Vec3; direction: Vec3 } | null;
}

let current: ViewportApi | null = null;

/** Called by the Viewport component on mount (with its api) and unmount (with null). */
export function registerViewport(api: ViewportApi | null) {
  current = api;
}

export const viewport: ViewportApi = {
  zoomTo: (target) => current?.zoomTo(target),
  zoomToBounds: (bounds) => current?.zoomToBounds(bounds),
  setViewPreset: (preset) => current?.setViewPreset(preset),
  centreOn: (point) => current?.centreOn(point),
  viewDirection: () => current?.viewDirection() ?? [0, 1, 0],
  surfaceAt: (x, y) => current?.surfaceAt(x, y) ?? null,
  probeAt: (x, y) => current?.probeAt(x, y) ?? null,
  rayAt: (x, y) => current?.rayAt(x, y) ?? null,
};
