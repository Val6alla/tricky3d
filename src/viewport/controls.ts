import { MOUSE, OrthographicCamera } from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { holdOrthoDistance, type ViewCamera } from "./cameras";

export type ViewControls = OrbitControls<ViewCamera>;

/** Right-drag orbits, middle-drag pans, the wheel zooms towards the cursor. Left is kept for picking. */
export function createControls(camera: ViewCamera, canvas: HTMLCanvasElement): ViewControls {
  const controls = new OrbitControls<ViewCamera>(camera, canvas);
  // OrbitControls maps an unknown action to "no state", so null leaves the left button alone.
  controls.mouseButtons = { LEFT: null, MIDDLE: MOUSE.PAN, RIGHT: MOUSE.ROTATE };
  controls.enableDamping = false;
  controls.zoomToCursor = true;
  controls.screenSpacePanning = true;
  // Orthographic zoom changes only `zoom`; keep the camera at the distance that zoom stands for.
  // update() sends "change" before it records the camera position, so this move never sends another.
  controls.addEventListener("change", () => {
    if (controls.object instanceof OrthographicCamera) holdOrthoDistance(controls.object, controls.target);
  });
  return controls;
}

type WheelInput = Pick<WheelEvent, "deltaX" | "deltaY" | "deltaMode"> & { wheelDeltaX?: number; wheelDeltaY?: number };

/**
 * True for a trackpad's two-finger scroll, false for a mouse wheel's notches. WebKit and Chromium on macOS give a
 * trackpad's legacy wheelDelta as -3 × its delta, and a wheel's as multiples of 120 whatever the delta; without the
 * legacy fields, only a trackpad scrolls sideways (Shift + wheel is handled before this is asked).
 */
export function isTrackpadScroll(e: WheelInput): boolean {
  if (e.deltaMode !== 0) return false;
  if (e.wheelDeltaY !== undefined && e.deltaY !== 0) return Math.abs(e.wheelDeltaY + 3 * e.deltaY) < 1;
  if (e.wheelDeltaX !== undefined && e.deltaX !== 0) return Math.abs(e.wheelDeltaX + 3 * e.deltaX) < 1;
  return e.deltaX !== 0;
}

/**
 * Wheel gestures OrbitControls does not know. Shift + wheel (Shift + two-finger scroll on a trackpad) pans in screen
 * space instead of zooming. On macOS a trackpad's two-finger scroll orbits, as a right-drag of the same length would.
 * Listens in the capture phase on `host` so OrbitControls on the canvas never sees these events. Pinch arrives as
 * ctrlKey + wheel, and a mouse wheel as plain wheel events: both are left to OrbitControls to zoom.
 */
export function attachWheelNavigation(host: HTMLElement, controls: ViewControls, onMove: () => void, isMac: boolean): () => void {
  const onWheel = (e: WheelEvent) => {
    if (e.ctrlKey) return;
    const orbit = !e.shiftKey && isMac && isTrackpadScroll(e);
    if (!e.shiftKey && !orbit) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    if (orbit) {
      // The fingers move against the scroll delta (natural scrolling); the scene follows them.
      const turn = (2 * Math.PI * controls.rotateSpeed) / controls.domElement!.clientHeight;
      controls.rotateLeft(-e.deltaX * turn);
      controls.rotateUp(-e.deltaY * turn);
    } else {
      const scale = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? 100 : 1;
      // Some platforms turn a Shift + vertical scroll into deltaX, so both deltas are used as given.
      // Negated so the scene moves like scrolled content.
      controls.pan(-e.deltaX * scale, -e.deltaY * scale);
    }
    onMove();
  };
  host.addEventListener("wheel", onWheel, { capture: true, passive: false });
  return () => host.removeEventListener("wheel", onWheel, { capture: true });
}

/**
 * macOS: Option + left-drag pans, for a trackpad without a middle button. Each press sets what the left button does
 * before OrbitControls on the canvas sees it (capture phase on `host`); a press `onGizmo` claims stays the gizmo's.
 */
export function attachOptionDragPan(host: HTMLElement, controls: ViewControls, onGizmo: () => boolean): () => void {
  const onDown = (e: PointerEvent) => {
    controls.mouseButtons.LEFT = e.button === 0 && e.altKey && !onGizmo() ? MOUSE.PAN : null;
  };
  host.addEventListener("pointerdown", onDown, { capture: true });
  return () => host.removeEventListener("pointerdown", onDown, { capture: true });
}
