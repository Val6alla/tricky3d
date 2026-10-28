import {
  BoxGeometry,
  CanvasTexture,
  CylinderGeometry,
  Group,
  LinearMipmapNearestFilter,
  Mesh,
  MeshBasicMaterial,
  OrthographicCamera,
  Raycaster,
  SRGBColorSpace,
  Scene,
  Vector2,
  type Camera,
  type WebGLRenderer,
} from "three";
import type { ViewPreset } from "./api";
import { disposeLabel, labelSprite, makeTextLabel, type TextLabel } from "./labels";
import { axisColor, type VpTheme } from "./theme";
import { orientAlongAxis } from "./util";

const SIZE = 110; // px, square
const MARGIN = 12; // px from the viewport's top-right corner
const HALF_EXTENT = 1.2; // cube camera half-size; the cube is 1 unit wide
const TEXTURE_PX = 256;

/**
 * BoxGeometry orders its faces +X, -X, +Y, -Y, +Z, -Z in its own Y-up frame. After rotateX(90°)
 * they become right, left, top, bottom, front, back in our Z-up world, and each label reads
 * upright in the matching preset view (top with +Y up, bottom with -Y up).
 */
export const FACES: { label: string; preset: ViewPreset }[] = [
  { label: "RIGHT", preset: "right" },
  { label: "LEFT", preset: "left" },
  { label: "TOP", preset: "top" },
  { label: "BOTTOM", preset: "bottom" },
  { label: "FRONT", preset: "front" },
  { label: "BACK", preset: "back" },
];

/** The cube, one unit wide, its faces in FACES order (BoxGeometry's groups, one material each). */
export const cubeGeometry = () => new BoxGeometry(1, 1, 1).rotateX(Math.PI / 2);

interface Face {
  label: string;
  preset: ViewPreset;
  canvas: HTMLCanvasElement;
  map: CanvasTexture;
  material: MeshBasicMaterial;
}

export interface ViewCube {
  /** Draws the cube over the top-right corner of the frame just rendered. */
  render(renderer: WebGLRenderer, main: Camera, width: number, height: number): void;
  applyTheme(theme: VpTheme): void;
  dispose(): void;
}

/**
 * Orientation cube drawn by the main renderer in its own scissored viewport (no second GL context).
 * `overlay` is an element over that corner that takes the cube's pointer events.
 */
export function createViewCube(
  overlay: HTMLElement,
  theme: VpTheme,
  getCamera: () => Camera,
  onPick: (preset: ViewPreset) => void,
  requestRender: () => void,
): ViewCube {
  Object.assign(overlay.style, { top: `${MARGIN}px`, right: `${MARGIN}px`, width: `${SIZE}px`, height: `${SIZE}px` });
  let current = theme;
  let hovered: number | null = null;

  const scene = new Scene();
  const camera = new OrthographicCamera(-HALF_EXTENT, HALF_EXTENT, HALF_EXTENT, -HALF_EXTENT, 0.1, 10);
  const faces = FACES.map(makeFace);
  const geometry = cubeGeometry();
  const cube = new Mesh(
    geometry,
    faces.map((f) => f.material),
  );
  const tripod = buildTripod(theme.font);
  scene.add(cube, tripod.group);

  const raycaster = new Raycaster();
  const pointer = new Vector2();

  function syncCamera(main: Camera) {
    camera.quaternion.copy(main.quaternion);
    camera.position.set(0, 0, 4).applyQuaternion(main.quaternion);
    camera.updateMatrixWorld();
  }

  function faceAt(e: MouseEvent): number | null {
    const rect = overlay.getBoundingClientRect();
    if (!rect.width || !rect.height) return null;
    pointer.set(((e.clientX - rect.left) / rect.width) * 2 - 1, -((e.clientY - rect.top) / rect.height) * 2 + 1);
    syncCamera(getCamera());
    raycaster.setFromCamera(pointer, camera);
    const hit = raycaster.intersectObject(cube, false)[0];
    return hit?.face ? hit.face.materialIndex : null;
  }

  function setHovered(index: number | null) {
    if (index === hovered) return;
    const previous = hovered;
    hovered = index;
    if (previous !== null) drawFace(faces[previous], false, current);
    if (index !== null) drawFace(faces[index], true, current);
    overlay.style.cursor = index === null ? "" : "pointer";
    requestRender();
  }

  function applyTheme(t: VpTheme) {
    current = t;
    faces.forEach((face, i) => drawFace(face, i === hovered, t));
    for (let axis = 0; axis < 3; axis++) {
      tripod.materials[axis].color.set(axisColor(t, axis));
      tripod.letters[axis].material.color.set(axisColor(t, axis));
    }
  }
  applyTheme(theme);

  const onMove = (e: PointerEvent) => setHovered(faceAt(e));
  const onLeave = () => setHovered(null);
  const onClick = (e: MouseEvent) => {
    const index = faceAt(e);
    if (index !== null) onPick(faces[index].preset);
  };
  const prevent = (e: Event) => e.preventDefault();
  overlay.addEventListener("pointermove", onMove);
  overlay.addEventListener("pointerleave", onLeave);
  overlay.addEventListener("click", onClick);
  overlay.addEventListener("contextmenu", prevent);
  // Stops the webview treating Ctrl + wheel or a pinch over the cube as page zoom.
  overlay.addEventListener("wheel", prevent, { passive: false });

  return {
    render(renderer, main, width, height) {
      syncCamera(main);
      const x = width - MARGIN - SIZE;
      const y = height - MARGIN - SIZE; // viewport y counts from the bottom
      renderer.setScissorTest(true);
      renderer.setScissor(x, y, SIZE, SIZE);
      renderer.setViewport(x, y, SIZE, SIZE);
      renderer.clearDepth();
      renderer.render(scene, camera);
      renderer.setScissorTest(false);
      renderer.setViewport(0, 0, width, height);
    },

    applyTheme,

    dispose() {
      overlay.removeEventListener("pointermove", onMove);
      overlay.removeEventListener("pointerleave", onLeave);
      overlay.removeEventListener("click", onClick);
      overlay.removeEventListener("contextmenu", prevent);
      overlay.removeEventListener("wheel", prevent);
      overlay.style.cursor = "";
      geometry.dispose();
      for (const face of faces) {
        face.map.dispose();
        face.material.dispose();
      }
      tripod.dispose();
    },
  };
}

function makeFace(face: { label: string; preset: ViewPreset }): Face {
  const canvas = document.createElement("canvas");
  canvas.width = TEXTURE_PX;
  canvas.height = TEXTURE_PX;
  const map = new CanvasTexture(canvas);
  map.colorSpace = SRGBColorSpace;
  // A face is drawn at 1/3 to 1/6 of the texture size. One mip level (not a blend of two) and
  // anisotropy for faces seen at an angle keep the label sharp; three caps anisotropy at the GPU's limit.
  map.minFilter = LinearMipmapNearestFilter;
  map.anisotropy = 8;
  return { ...face, canvas, map, material: new MeshBasicMaterial({ map }) };
}

/** A face is only about 45 px on screen, so the label is bold and as wide as the face allows. */
function drawFace(face: Face, hovered: boolean, theme: VpTheme) {
  const ctx = face.canvas.getContext("2d");
  if (!ctx) return;
  const s = TEXTURE_PX;
  const line = s * 0.045;
  ctx.fillStyle = theme.surface;
  ctx.fillRect(0, 0, s, s);
  if (hovered) {
    ctx.globalAlpha = 0.28;
    ctx.fillStyle = theme.accent;
    ctx.fillRect(0, 0, s, s);
    ctx.globalAlpha = 1;
  }
  ctx.strokeStyle = hovered ? theme.accent : theme.borderStrong;
  ctx.lineWidth = line;
  ctx.strokeRect(line / 2, line / 2, s - line, s - line);

  // Fit the label to most of the face width; the cap keeps short words like TOP from looming.
  ctx.font = `700 100px ${theme.font}`;
  const size = Math.min(s * 0.3, (100 * s * 0.8) / ctx.measureText(face.label).width);
  ctx.font = `700 ${size}px ${theme.font}`;
  const m = ctx.measureText(face.label);
  ctx.fillStyle = theme.text;
  ctx.textAlign = "center";
  ctx.fillText(face.label, s / 2, (s + m.actualBoundingBoxAscent - m.actualBoundingBoxDescent) / 2);
  face.map.needsUpdate = true;
}

/** Small X/Y/Z tripod just outside the cube's left-front-bottom corner. */
function buildTripod(font: string) {
  const corner = -0.5 - 0.08;
  const length = 0.62;
  const shaft = new CylinderGeometry(0.028, 0.028, length, 8).translate(0, length / 2, 0);
  const materials = [0, 1, 2].map(() => new MeshBasicMaterial());
  const letters: TextLabel[] = ["X", "Y", "Z"].map((text) => makeTextLabel(text, font));
  const group = new Group();
  group.position.set(corner, corner, corner);
  for (let axis = 0; axis < 3; axis++) {
    const mesh = new Mesh(shaft, materials[axis]);
    orientAlongAxis(mesh, axis);
    const letter = labelSprite(letters[axis], 0.3);
    letter.position.setComponent(axis, length + 0.16);
    group.add(mesh, letter);
  }
  return {
    group,
    materials,
    letters,
    dispose() {
      shaft.dispose();
      for (const material of materials) material.dispose();
      for (const letter of letters) disposeLabel(letter);
    },
  };
}
