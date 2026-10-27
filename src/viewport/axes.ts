import { ConeGeometry, CylinderGeometry, Group, Mesh, MeshBasicMaterial } from "three";
import type { Vec3 } from "../store";
import { disposeLabel, labelSprite, makeTextLabel } from "./labels";
import { axisColor, type VpTheme } from "./theme";
import { orientAlongAxis, type SceneLayer } from "./util";

/** X/Y/Z arrows from the origin, about 15% of the platform's largest side, with letters at the tips. */
export function buildAxes(size: Vec3, theme: VpTheme): SceneLayer {
  const length = Math.max(...size) * 0.15;
  const radius = length * 0.012;
  const headLength = length * 0.14;
  const shaftLength = length - headLength;
  const shaft = new CylinderGeometry(radius, radius, shaftLength, 8).translate(0, shaftLength / 2, 0);
  const head = new ConeGeometry(radius * 3.2, headLength, 16).translate(0, shaftLength + headLength / 2, 0);
  const materials = [0, 1, 2].map(() => new MeshBasicMaterial());
  const letters = ["X", "Y", "Z"].map((text) => makeTextLabel(text, theme.font));

  const group = new Group();
  for (let axis = 0; axis < 3; axis++) {
    const arrow = new Group();
    arrow.add(new Mesh(shaft, materials[axis]), new Mesh(head, materials[axis]));
    orientAlongAxis(arrow, axis);
    const letter = labelSprite(letters[axis], length * 0.16);
    letter.position.setComponent(axis, length * 1.12);
    group.add(arrow, letter);
  }

  function applyTheme(t: VpTheme) {
    for (let axis = 0; axis < 3; axis++) {
      materials[axis].color.set(axisColor(t, axis));
      letters[axis].material.color.set(axisColor(t, axis));
    }
  }
  applyTheme(theme);

  return {
    group,
    applyTheme,
    dispose() {
      shaft.dispose();
      head.dispose();
      for (const material of materials) material.dispose();
      for (const letter of letters) disposeLabel(letter);
    },
  };
}
