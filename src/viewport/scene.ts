import { DirectionalLight, HemisphereLight, OrthographicCamera, Vector3, WebGLRenderer, type Scene, type Sphere } from "three";
import type { ViewCamera } from "./cameras";

export function createRenderer(host: HTMLElement): WebGLRenderer {
  const renderer = new WebGLRenderer({ antialias: true, alpha: true });
  renderer.setPixelRatio(window.devicePixelRatio);
  // Transparent clear: the CSS gradient on the container is the background.
  renderer.setClearAlpha(0);
  // The view cube draws over the finished scene, so clears are explicit.
  renderer.autoClear = false;
  renderer.domElement.className = "viewport-canvas";
  host.prepend(renderer.domElement);
  return renderer;
}

/**
 * Calls `onChange` when devicePixelRatio changes, e.g. when the window moves to another monitor.
 * A resolution query matches one ratio only, so it is re-armed for the new ratio after each change.
 */
export function watchPixelRatio(onChange: () => void): () => void {
  let query = window.matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
  const onQueryChange = () => {
    query.removeEventListener("change", onQueryChange);
    query = window.matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
    query.addEventListener("change", onQueryChange);
    onChange();
  };
  query.addEventListener("change", onQueryChange);
  return () => query.removeEventListener("change", onQueryChange);
}

/** Light direction in camera space: from above-left, behind the viewer. */
const HEADLIGHT = new Vector3(-0.4, 0.6, 1).normalize();

export interface Lights {
  /** Turns the headlight with the camera so the side facing the user is always lit. */
  follow(camera: ViewCamera): void;
  dispose(): void;
}

/** Neutral lighting: a sky/ground fill (top faces read lighter) plus a headlight. White and grey set brightness only. */
export function addLights(scene: Scene): Lights {
  const fill = new HemisphereLight(0xffffff, 0x6b6b6b, 1.4);
  fill.position.set(0, 0, 1);
  const headlight = new DirectionalLight(0xffffff, 2.1);
  scene.add(fill, headlight);
  return {
    follow(camera) {
      headlight.position.copy(HEADLIGHT).applyQuaternion(camera.quaternion);
    },
    dispose() {
      scene.remove(fill, headlight);
      fill.dispose();
      headlight.dispose();
    },
  };
}

const toCentre = new Vector3();
const forward = new Vector3();

/**
 * Fits near/far around the scene sphere so depth precision holds from 0.25 mm to whole-platform zoom.
 * The perspective near plane also stays within a tenth of the orbit distance, so zooming in close
 * on a small part on a big platform never clips it.
 */
export function fitClipPlanes(camera: ViewCamera, sphere: Sphere, orbitDistance: number) {
  const radius = sphere.radius * 1.05 + 1;
  camera.getWorldDirection(forward);
  const depth = toCentre.subVectors(sphere.center, camera.position).dot(forward);
  let near: number;
  let far: number;
  if (camera instanceof OrthographicCamera) {
    // An orthographic near plane may sit behind the camera; depth stays linear either way.
    near = depth - radius;
    far = depth + radius;
  } else {
    far = Math.max(depth + radius, 1);
    near = Math.max(depth - radius, Math.min(far / 2000, orbitDistance * 0.1));
  }
  if (camera.near === near && camera.far === far) return;
  camera.near = near;
  camera.far = far;
  camera.updateProjectionMatrix();
}
