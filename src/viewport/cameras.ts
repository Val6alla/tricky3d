import { OrthographicCamera, PerspectiveCamera, Raycaster, Vector3, type Camera, type Vector2 } from "three";
import type { Bounds } from "../mesh";
import type { ViewPreset } from "./api";

export type ViewCamera = PerspectiveCamera | OrthographicCamera;

export const FOV = 35;
const TAN_HALF_FOV = Math.tan((FOV * Math.PI) / 360);
/** Orthographic frustum half-height at zoom 1, in mm. Zoom then sets the visible size. */
const ORTHO_HALF = 100;
/**
 * Top and bottom views sit this far (radians) off the pole, on the front side. Exactly on the
 * pole, lookAt with Z up has no defined screen "up" and the view would flip or spin.
 */
const POLE_EPS = 1e-4;
const FIT_MARGIN = 1.15;
export const MIN_HALF_HEIGHT = 0.25;

/** Where the camera is around its orbit target. The same state means the same picture in either projection. */
export interface ViewState {
  target: Vector3;
  /** Azimuth of the camera around +Z, from +X towards +Y. */
  theta: number;
  /** Angle from +Z to the camera. */
  phi: number;
  /** Half the visible height at the target, mm. Perspective distance is halfHeight / tan(fov / 2). */
  halfHeight: number;
}

export type ViewDirection = Pick<ViewState, "theta" | "phi">;

const FRONT = -Math.PI / 2;

/** Front: camera on -Y looking +Y. Top keeps +Y up on screen; bottom keeps -Y up. Iso: from (+1, -1, +1). */
export const PRESETS: Record<ViewPreset, ViewDirection> = {
  front: { theta: FRONT, phi: Math.PI / 2 },
  back: { theta: Math.PI / 2, phi: Math.PI / 2 },
  left: { theta: Math.PI, phi: Math.PI / 2 },
  right: { theta: 0, phi: Math.PI / 2 },
  top: { theta: FRONT, phi: POLE_EPS },
  bottom: { theta: FRONT, phi: Math.PI - POLE_EPS },
  iso: { theta: -Math.PI / 4, phi: Math.acos(1 / Math.sqrt(3)) },
};

export interface Cameras {
  perspective: PerspectiveCamera;
  orthographic: OrthographicCamera;
}

export function createCameras(): Cameras {
  const perspective = new PerspectiveCamera(FOV, 1, 0.1, 10000);
  const orthographic = new OrthographicCamera(-ORTHO_HALF, ORTHO_HALF, ORTHO_HALF, -ORTHO_HALF, -10000, 10000);
  for (const camera of [perspective, orthographic]) {
    camera.up.set(0, 0, 1);
    camera.position.set(1, -1, 1);
  }
  return { perspective, orthographic };
}

export function setAspect({ perspective, orthographic }: Cameras, aspect: number) {
  perspective.aspect = aspect;
  perspective.updateProjectionMatrix();
  orthographic.left = -ORTHO_HALF * aspect;
  orthographic.right = ORTHO_HALF * aspect;
  orthographic.top = ORTHO_HALF;
  orthographic.bottom = -ORTHO_HALF;
  orthographic.updateProjectionMatrix();
}

/** Unit vector from the target towards the camera. */
export function directionOf({ theta, phi }: ViewDirection, out = new Vector3()): Vector3 {
  const s = Math.sin(phi);
  return out.set(Math.cos(theta) * s, Math.sin(theta) * s, Math.cos(phi));
}

export function readView(camera: ViewCamera, target: Vector3): ViewState {
  const offset = new Vector3().subVectors(camera.position, target);
  const distance = offset.length() || 1;
  const halfHeight = camera instanceof OrthographicCamera ? ORTHO_HALF / camera.zoom : distance * TAN_HALF_FOV;
  return {
    target: target.clone(),
    theta: Math.atan2(offset.y, offset.x),
    phi: Math.acos(Math.min(1, Math.max(-1, offset.z / distance))),
    halfHeight,
  };
}

/**
 * Places the camera for `view` and moves `target` (the controls' target) with it. The orthographic
 * camera sits at the perspective distance too, so switching projection keeps the same picture.
 * Call controls.update() afterwards so OrbitControls re-reads the camera.
 */
export function applyView(camera: ViewCamera, view: ViewState, target: Vector3) {
  const distance = view.halfHeight / TAN_HALF_FOV;
  directionOf(view, camera.position).multiplyScalar(distance).add(view.target);
  if (camera instanceof OrthographicCamera) {
    camera.zoom = ORTHO_HALF / view.halfHeight;
    camera.updateProjectionMatrix();
  }
  target.copy(view.target);
}

const forward = new Vector3();
const toTarget = new Vector3();

/**
 * Moves an orthographic camera along its line of sight to the distance applyView gives its zoom;
 * the picture does not change. OrbitControls zooms it through `zoom` alone, which would leave it
 * at the old distance: inside the scene after a zoom-out.
 */
export function holdOrthoDistance(camera: OrthographicCamera, target: Vector3) {
  camera.getWorldDirection(forward);
  const depth = toTarget.subVectors(target, camera.position).dot(forward);
  camera.position.addScaledVector(forward, depth - ORTHO_HALF / camera.zoom / TAN_HALF_FOV);
  camera.updateMatrixWorld();
}

/**
 * Raycaster.setFromCamera, but an orthographic ray starts on the near plane when that lies behind
 * the camera: zoomed in close, the camera sits inside the scene (fitClipPlanes), and what is drawn
 * between the two must still be hit. Never later than the camera plane, so a gizmo's drag plane
 * stays reachable there. Calls the prototype's method, so it can stand in for a raycaster's own.
 */
export function rayFromCamera(raycaster: Raycaster, pointer: Vector2, camera: Camera) {
  Raycaster.prototype.setFromCamera.call(raycaster, pointer, camera);
  if (camera instanceof OrthographicCamera && camera.near < 0) raycaster.ray.origin.addScaledVector(raycaster.ray.direction, camera.near);
}

/**
 * View from `direction` centred on `box`, sized to show all of it with a margin. `fill` is the
 * share of the view's width and height the box may use (less than 1 leaves room for the rulers);
 * the box stays centred so orbiting still pivots on it.
 */
export function fitView(box: Bounds, direction: ViewDirection, aspect: number, orthographic: boolean, fill: readonly [number, number] = [1, 1]): ViewState {
  const { min, max } = box;
  const target = new Vector3((min[0] + max[0]) / 2, (min[1] + max[1]) / 2, (min[2] + max[2]) / 2);
  // Camera basis, as Matrix4.lookAt builds it with Z up: right = up × back, screen up = back × right.
  const back = directionOf(direction);
  const right = new Vector3(-back.y, back.x, 0);
  if (right.lengthSq() < 1e-24) right.set(1, 0, 0);
  right.normalize();
  const up = new Vector3().crossVectors(back, right);

  const tanV = (TAN_HALF_FOV / FIT_MARGIN) * fill[1];
  const tanH = (TAN_HALF_FOV / FIT_MARGIN) * aspect * fill[0];
  const corner = new Vector3();
  let halfHeight = MIN_HALF_HEIGHT;
  let distance = 0;
  for (let i = 0; i < 8; i++) {
    corner.set(i & 1 ? max[0] : min[0], i & 2 ? max[1] : min[1], i & 4 ? max[2] : min[2]).sub(target);
    const x = Math.abs(corner.dot(right));
    const y = Math.abs(corner.dot(up));
    const z = corner.dot(back); // towards the camera
    if (orthographic) {
      halfHeight = Math.max(halfHeight, (y * FIT_MARGIN) / fill[1], ((x / aspect) * FIT_MARGIN) / fill[0]);
    } else {
      // A corner at depth (distance - z) must project inside the frustum on both axes.
      distance = Math.max(distance, z + y / tanV, z + x / tanH);
    }
  }
  if (!orthographic) halfHeight = Math.max(halfHeight, distance * TAN_HALF_FOV);
  return { target, theta: direction.theta, phi: direction.phi, halfHeight };
}

/** OrbitControls zoom limits for both projections, scaled to the scene. */
export function zoomLimits(sceneRadius: number) {
  const maxHalfHeight = Math.max(1000, sceneRadius * 20);
  return {
    minDistance: MIN_HALF_HEIGHT / TAN_HALF_FOV,
    maxDistance: maxHalfHeight / TAN_HALF_FOV,
    minZoom: ORTHO_HALF / maxHalfHeight,
    maxZoom: ORTHO_HALF / MIN_HALF_HEIGHT,
  };
}
