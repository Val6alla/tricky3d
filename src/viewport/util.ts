import { BufferAttribute, BufferGeometry, Group, LineBasicMaterial, LineSegments, type Object3D } from "three";
import type { VpTheme } from "./theme";

/** A group of scene objects that follows the theme and owns its GPU resources. */
export interface SceneLayer {
  group: Group;
  applyTheme(theme: VpTheme): void;
  dispose(): void;
}

/** LineSegments over pairs of points (x, y, z, x, y, z, ...). */
export function lineSegments(positions: Float32Array, material: LineBasicMaterial): LineSegments<BufferGeometry, LineBasicMaterial> {
  const geometry = new BufferGeometry();
  geometry.setAttribute("position", new BufferAttribute(positions, 3));
  return new LineSegments(geometry, material);
}

/** Rotates an object modelled along +Y so it points along world axis 0 (X), 1 (Y) or 2 (Z). */
export function orientAlongAxis(object: Object3D, axis: number) {
  if (axis === 0) object.rotation.set(0, 0, -Math.PI / 2);
  else if (axis === 2) object.rotation.set(Math.PI / 2, 0, 0);
  else object.rotation.set(0, 0, 0);
}
