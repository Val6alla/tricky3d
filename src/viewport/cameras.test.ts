import { BoxGeometry, Mesh, MeshBasicMaterial, Raycaster, Vector2, Vector3 } from "three";
import { describe, expect, it } from "vitest";
import { applyView, createCameras, FOV, holdOrthoDistance, PRESETS, rayFromCamera, setAspect, type ViewCamera, type ViewState } from "./cameras";

const TAN_HALF_FOV = Math.tan((FOV * Math.PI) / 360);
const A = new Vector3(100, 100, 10);

function cameras() {
  const all = createCameras();
  setAspect(all, 4 / 3);
  return all;
}

/** Places `camera` for `view` and aims it as OrbitControls.update() would; returns the target. */
function place(camera: ViewCamera, view: ViewState): Vector3 {
  const target = new Vector3();
  applyView(camera, view, target);
  camera.lookAt(target);
  camera.updateMatrixWorld();
  return target;
}

describe("holdOrthoDistance", () => {
  it("moves the camera to the distance of its zoom without changing the picture", () => {
    const camera = cameras().orthographic;
    const target = place(camera, { target: A.clone(), ...PRESETS.iso, halfHeight: 20 });
    expect(camera.position.distanceTo(target)).toBeCloseTo(20 / TAN_HALF_FOV, 9);

    // A wheel zoom-out in OrbitControls changes the zoom only.
    camera.zoom /= 5;
    camera.updateProjectionMatrix();
    const points = [A, new Vector3(180, 20, 10), new Vector3(0, 0, 0), new Vector3(200, 200, 200)];
    const before = points.map((p) => p.clone().project(camera));
    holdOrthoDistance(camera, target);
    expect(camera.position.distanceTo(target)).toBeCloseTo(100 / TAN_HALF_FOV, 9);
    points.forEach((p, i) => {
      const after = p.clone().project(camera);
      expect(after.x).toBeCloseTo(before[i].x, 12);
      expect(after.y).toBeCloseTo(before[i].y, 12);
    });

    // Where applyView would put it for the new size.
    const fresh = cameras().orthographic;
    place(fresh, { target: A.clone(), ...PRESETS.iso, halfHeight: 100 });
    expect(camera.position.distanceTo(fresh.position)).toBeLessThan(1e-9);
  });

  it("keeps a target that is off the line of sight off it, moving only along the view", () => {
    const camera = cameras().orthographic;
    const target = place(camera, { target: A.clone(), ...PRESETS.front, halfHeight: 50 });
    target.x += 30;
    camera.zoom = 4;
    camera.updateProjectionMatrix();
    const { x, z } = camera.position;
    holdOrthoDistance(camera, target);
    expect(camera.position.x).toBeCloseTo(x, 12);
    expect(camera.position.z).toBeCloseTo(z, 12);
    expect(target.y - camera.position.y).toBeCloseTo(25 / TAN_HALF_FOV, 9);
  });
});

describe("rayFromCamera", () => {
  const pointer = new Vector2(0, 0);

  it("starts an orthographic ray on a near plane behind the camera, so parts nearer than the camera are hit", () => {
    const camera = cameras().orthographic;
    place(camera, { target: A.clone(), ...PRESETS.front, halfHeight: 20 });
    // Zoomed in close the camera is inside the scene: fitClipPlanes puts the near plane behind it.
    camera.near = -200;
    camera.far = 400;
    camera.updateProjectionMatrix();
    // A cube between the near plane and the camera, on the line of sight (front view looks along +Y).
    const nearer = new Mesh(new BoxGeometry(10, 10, 10), new MeshBasicMaterial());
    nearer.position.copy(camera.position).add(new Vector3(0, -30, 0));
    nearer.updateMatrixWorld();

    const raycaster = new Raycaster();
    raycaster.setFromCamera(pointer, camera);
    expect(raycaster.intersectObject(nearer)).toHaveLength(0);
    rayFromCamera(raycaster, pointer, camera);
    expect(raycaster.ray.origin.y).toBeCloseTo(camera.position.y - 200, 9);
    expect(raycaster.intersectObject(nearer)).toHaveLength(1);
  });

  it("never starts an orthographic ray after the camera plane", () => {
    const camera = cameras().orthographic;
    place(camera, { target: A.clone(), ...PRESETS.front, halfHeight: 20 });
    // Zoomed out the camera is outside the scene and the near plane lies ahead of it.
    camera.near = 40;
    camera.far = 400;
    camera.updateProjectionMatrix();
    const raycaster = new Raycaster();
    rayFromCamera(raycaster, pointer, camera);
    expect(raycaster.ray.origin.y).toBeCloseTo(camera.position.y, 9);
  });

  it("leaves perspective rays at the camera", () => {
    const camera = cameras().perspective;
    place(camera, { target: A.clone(), ...PRESETS.iso, halfHeight: 20 });
    const raycaster = new Raycaster();
    rayFromCamera(raycaster, new Vector2(0.3, -0.2), camera);
    const plain = new Raycaster();
    plain.setFromCamera(new Vector2(0.3, -0.2), camera);
    expect(raycaster.ray.origin.distanceTo(plain.ray.origin)).toBe(0);
    expect(raycaster.ray.direction.distanceTo(plain.ray.direction)).toBe(0);
  });

  it("can stand in for a raycaster's own setFromCamera", () => {
    const camera = cameras().orthographic;
    place(camera, { target: A.clone(), ...PRESETS.front, halfHeight: 20 });
    camera.near = -200;
    camera.updateProjectionMatrix();
    const raycaster = new Raycaster();
    raycaster.setFromCamera = (coords, cam) => rayFromCamera(raycaster, coords, cam);
    raycaster.setFromCamera(pointer, camera);
    // Offset once, no recursion.
    expect(raycaster.ray.origin.y).toBeCloseTo(camera.position.y - 200, 9);
  });
});
