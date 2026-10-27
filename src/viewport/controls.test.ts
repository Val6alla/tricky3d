import { MOUSE, Vector3 } from "three";
import { describe, expect, it } from "vitest";
import { applyView, createCameras, FOV, PRESETS, readView, setAspect, zoomLimits, type ViewCamera } from "./cameras";
import { attachOptionDragPan, attachWheelNavigation, createControls, isTrackpadScroll } from "./controls";

const TAN_HALF_FOV = Math.tan((FOV * Math.PI) / 360);
const W = 800;
const H = 600;

/** Just enough of a canvas for OrbitControls; `fire` sends an event to its listeners. */
function fakeCanvas() {
  const listeners = new Map<string, (e: unknown) => void>();
  const root = { addEventListener() {}, removeEventListener() {} };
  const canvas = {
    style: {},
    clientWidth: W,
    clientHeight: H,
    addEventListener: (type: string, fn: (e: unknown) => void) => listeners.set(type, fn),
    removeEventListener() {},
    ownerDocument: root,
    getRootNode: () => root,
    getBoundingClientRect: () => ({ left: 0, top: 0, width: W, height: H }),
  };
  return { canvas: canvas as unknown as HTMLCanvasElement, fire: (type: string, e: object) => listeners.get(type)?.(e) };
}

function cameras() {
  const all = createCameras();
  setAspect(all, W / H);
  return all;
}

const wheel = (deltaY: number, clientX: number, clientY: number) => ({ deltaY, deltaMode: 0, clientX, clientY, ctrlKey: false, preventDefault() {} });

/** Controls on `camera` (aspect set), looking at A in iso from the distance of a 20 mm half-height. */
function setUp(camera: ViewCamera) {
  const { canvas, fire } = fakeCanvas();
  const controls = createControls(camera, canvas);
  Object.assign(controls, zoomLimits(300));
  applyView(camera, { target: new Vector3(100, 100, 10), ...PRESETS.iso, halfHeight: 20 }, controls.target);
  controls.update();
  camera.updateMatrixWorld();
  let changes = 0;
  controls.addEventListener("change", () => changes++);
  return { controls, fire, changes: () => changes };
}

describe("createControls, orthographic wheel zoom", () => {
  it("keeps the camera at the distance of its zoom, and the point under the cursor in place", () => {
    const camera = cameras().orthographic;
    const { controls, fire, changes } = setUp(camera);
    // The world point under the cursor at (200, 150) px.
    const under = new Vector3((200 / W) * 2 - 1, -(150 / H) * 2 + 1, 0).unproject(camera);

    for (let i = 0; i < 5; i++) fire("wheel", wheel(300, 200, 150));
    expect(changes()).toBe(5);
    const { halfHeight } = readView(camera, controls.target);
    // Each 300-unit step zooms out by 1 / 0.95³.
    expect(halfHeight).toBeCloseTo(20 / Math.pow(0.95, 15), 9);
    expect(camera.position.distanceTo(controls.target)).toBeCloseTo(halfHeight / TAN_HALF_FOV, 6);
    // The target stays on the line of sight.
    const toTarget = controls.target.clone().sub(camera.position).normalize();
    expect(toTarget.distanceTo(camera.getWorldDirection(new Vector3()))).toBeLessThan(1e-9);
    // Zoom to cursor: the same point is still under the cursor.
    camera.updateMatrixWorld();
    const p = under.clone().project(camera);
    expect(p.x).toBeCloseTo((200 / W) * 2 - 1, 9);
    expect(p.y).toBeCloseTo(-(150 / H) * 2 + 1, 9);

    // The move is recorded as the controls' own: nothing changed, so no further event.
    controls.update();
    expect(changes()).toBe(5);
  });

  it("leaves the perspective camera to OrbitControls", () => {
    const camera = cameras().perspective;
    const { controls, fire } = setUp(camera);
    const before = camera.position.distanceTo(controls.target);
    fire("wheel", wheel(300, W / 2, H / 2));
    // Zooming out at the centre moves the camera straight back by the wheel's factor.
    expect(camera.position.distanceTo(controls.target) / before).toBeCloseTo(1 / Math.pow(0.95, 3), 9);
  });
});

describe("isTrackpadScroll", () => {
  const scroll = (e: Partial<{ deltaX: number; deltaY: number; deltaMode: number; wheelDeltaX: number; wheelDeltaY: number }>) =>
    isTrackpadScroll({ deltaX: 0, deltaY: 0, deltaMode: 0, ...e });

  it("tells a trackpad's two-finger scroll by its legacy delta of -3 × delta", () => {
    expect(scroll({ deltaY: 3, wheelDeltaY: -9 })).toBe(true);
    // The legacy value is rounded to whole units.
    expect(scroll({ deltaY: 0.5, wheelDeltaY: -1 })).toBe(true);
    expect(scroll({ deltaX: -7, wheelDeltaX: 21, wheelDeltaY: 0 })).toBe(true);
  });

  it("leaves a mouse wheel's notches to zoom", () => {
    // WebKit on macOS: a notch is 120 legacy units, whatever the accelerated delta.
    expect(scroll({ deltaY: 4.000244140625, wheelDeltaY: -120 })).toBe(false);
    expect(scroll({ deltaY: -12, wheelDeltaY: 360 })).toBe(false);
    // Lines or pages only come from a wheel.
    expect(scroll({ deltaY: 3, deltaMode: 1, wheelDeltaY: -9 })).toBe(false);
    // Without the legacy fields only a sideways scroll counts.
    expect(scroll({ deltaY: 100 })).toBe(false);
    expect(scroll({ deltaX: 5 })).toBe(true);
  });
});

/** A host element that keeps its capture listeners, for attachWheelNavigation and attachOptionDragPan. */
function fakeHost() {
  const listeners = new Map<string, (e: unknown) => void>();
  const host = {
    addEventListener: (type: string, fn: (e: unknown) => void) => listeners.set(type, fn),
    removeEventListener: (type: string) => listeners.delete(type),
  };
  return { host: host as unknown as HTMLElement, fire: (type: string, e: object) => listeners.get(type)?.(e) };
}

/** A wheel event that records whether it was taken. */
function wheelEvent(e: Partial<WheelEvent> & { wheelDeltaX?: number; wheelDeltaY?: number }) {
  const event = { deltaX: 0, deltaY: 0, deltaMode: 0, ctrlKey: false, shiftKey: false, taken: false, ...e };
  return Object.assign(event, {
    preventDefault() {
      event.taken = true;
    },
    stopImmediatePropagation() {},
  });
}

describe("attachWheelNavigation", () => {
  const trackpad = { deltaX: -30, deltaY: 12, wheelDeltaX: 90, wheelDeltaY: -36 };

  function navigate(isMac: boolean) {
    const camera = cameras().perspective;
    const { controls } = setUp(camera);
    const { host, fire } = fakeHost();
    let moves = 0;
    attachWheelNavigation(host, controls, () => moves++, isMac);
    const offset = () => camera.position.clone().sub(controls.target);
    return { controls, fire, offset, moves: () => moves };
  }

  it("orbits on a Mac trackpad's two-finger scroll, as far as a right-drag of the same length", () => {
    const { controls, fire, offset, moves } = navigate(true);
    const before = offset();
    const target = controls.target.clone();
    const e = wheelEvent(trackpad);
    fire("wheel", e);
    expect(e.taken).toBe(true);
    expect(moves()).toBe(1);
    const after = offset();
    // The target and the distance stay; the view turned by 2π × 30 / 600 about the vertical axis.
    expect(controls.target.distanceTo(target)).toBe(0);
    expect(after.length()).toBeCloseTo(before.length(), 9);
    const azimuth = (v: Vector3) => Math.atan2(v.y, v.x);
    const turned = azimuth(after) - azimuth(before);
    expect(Math.abs(Math.abs(turned) - (2 * Math.PI * 30) / H)).toBeLessThan(1e-9);
    expect(after.z).not.toBeCloseTo(before.z, 6);
  });

  it("leaves a mouse wheel, a pinch and other platforms' trackpads to zoom", () => {
    const mac = navigate(true);
    const wheel = wheelEvent({ deltaY: 4.000244140625, wheelDeltaY: -120 });
    const pinch = wheelEvent({ ...trackpad, ctrlKey: true });
    mac.fire("wheel", wheel);
    mac.fire("wheel", pinch);
    expect([wheel.taken, pinch.taken, mac.moves()]).toEqual([false, false, 0]);

    const windows = navigate(false);
    const e = wheelEvent(trackpad);
    windows.fire("wheel", e);
    expect([e.taken, windows.moves()]).toEqual([false, 0]);
  });

  it("pans on Shift + scroll everywhere", () => {
    for (const isMac of [true, false]) {
      const { controls, fire, offset, moves } = navigate(isMac);
      const before = offset();
      const target = controls.target.clone();
      const e = wheelEvent({ ...trackpad, shiftKey: true });
      fire("wheel", e);
      expect(e.taken).toBe(true);
      expect(moves()).toBe(1);
      expect(controls.target.distanceTo(target)).toBeGreaterThan(0);
      expect(offset().distanceTo(before)).toBeLessThan(1e-9);
    }
  });
});

describe("attachOptionDragPan", () => {
  it("makes an Option + left-drag pan, unless the gizmo takes the press", () => {
    const { controls } = setUp(cameras().perspective);
    const { host, fire } = fakeHost();
    let onGizmo = false;
    attachOptionDragPan(host, controls, () => onGizmo);
    fire("pointerdown", { button: 0, altKey: true });
    expect(controls.mouseButtons.LEFT).toBe(MOUSE.PAN);
    // The next plain press is a click again.
    fire("pointerdown", { button: 0, altKey: false });
    expect(controls.mouseButtons.LEFT).toBeNull();
    onGizmo = true;
    fire("pointerdown", { button: 0, altKey: true });
    expect(controls.mouseButtons.LEFT).toBeNull();
    // Right-drag still orbits.
    expect(controls.mouseButtons.RIGHT).toBe(MOUSE.ROTATE);
  });
});
