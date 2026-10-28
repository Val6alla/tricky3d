// The Part library's tile pictures, drawn by the viewport's own code: its part material and lights (src/viewport),
// one shared offscreen renderer, the iso direction. Each picture is a mesh Rust built (src/library/ipc.ts), drawn
// once into an image URL the tile shows; nothing is drawn while the library is idle.
import { BufferAttribute, BufferGeometry, FrontSide, Mesh, PerspectiveCamera, Scene, Sphere, Vector3, WebGLRenderer } from "three";
import { PART_COLORS, type MeshData } from "../store";
import { surfaceMaterial } from "../viewport/parts";
import { addLights } from "../viewport/scene";

/** Tile picture size in CSS px; drawn at twice that for sharp edges on high-density screens. */
export const THUMB_WIDTH = 132;
export const THUMB_HEIGHT = 100;

/** From the front right, a little above: the platform's iso view. */
const VIEW = new Vector3(1, -1, 0.8).normalize();

interface Painter {
  renderer: WebGLRenderer;
  scene: Scene;
  camera: PerspectiveCamera;
  lights: ReturnType<typeof addLights>;
}

let painter: Painter | null = null;

function getPainter(): Painter {
  if (painter) return painter;
  const renderer = new WebGLRenderer({ antialias: true, alpha: true, preserveDrawingBuffer: true });
  renderer.setPixelRatio(2);
  renderer.setSize(THUMB_WIDTH, THUMB_HEIGHT, false);
  renderer.setClearAlpha(0);
  const scene = new Scene();
  const camera = new PerspectiveCamera(30, THUMB_WIDTH / THUMB_HEIGHT, 0.1, 1e6);
  camera.up.set(0, 0, 1);
  painter = { renderer, scene, camera, lights: addLights(scene) };
  return painter;
}

/** Frees the offscreen renderer; the next picture makes a new one. Called when the library closes. */
export function releaseThumbnails(): void {
  if (!painter) return;
  painter.lights.dispose();
  painter.renderer.forceContextLoss();
  painter.renderer.dispose();
  painter = null;
}

/** The mesh drawn in the part colour, filling the picture, as an image URL. */
export function thumbnail(mesh: MeshData, color: string = PART_COLORS[0]): string {
  const { renderer, scene, camera, lights } = getPainter();
  const geometry = new BufferGeometry();
  geometry.setAttribute("position", new BufferAttribute(mesh.positions, 3));
  geometry.setIndex(new BufferAttribute(mesh.indices, 1));
  geometry.computeBoundingSphere();
  const material = surfaceMaterial(FrontSide);
  material.color.set(color);
  const object = new Mesh(geometry, material);
  scene.add(object);
  const sphere = geometry.boundingSphere ?? new Sphere(new Vector3(), 1);
  const radius = Math.max(sphere.radius, 1e-3);
  // Far enough for the sphere round the mesh to fit the narrower of the two angles.
  const half = (camera.fov * Math.PI) / 360;
  const distance = (radius / Math.sin(Math.min(half, Math.atan(Math.tan(half) * camera.aspect)))) * 1.05;
  camera.position.copy(sphere.center).addScaledVector(VIEW, distance);
  camera.lookAt(sphere.center);
  camera.near = distance / 100;
  camera.far = distance * 3;
  camera.updateProjectionMatrix();
  lights.follow(camera);
  renderer.render(scene, camera);
  const url = renderer.domElement.toDataURL("image/png");
  scene.remove(object);
  geometry.dispose();
  material.dispose();
  return url;
}
