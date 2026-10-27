import { Vector3 } from "three";
import { describe, expect, it } from "vitest";
import type { Bounds } from "../mesh";
import type { ViewPreset } from "./api";
import { applyView, createCameras, FOV, PRESETS, setAspect, type ViewCamera, type ViewDirection } from "./cameras";
import {
  boxRange,
  chooseTicks,
  formatValue,
  rulerMappings,
  screenAt,
  screenAxes,
  straightAhead,
  tickKind,
  valueAt,
  type HeldAxes,
  type Point,
} from "./rulerScale";

const W = 1200;
const H = 800;

/** A camera placed the way the viewport places it: applyView, then lookAt as OrbitControls.update() does. */
function viewCamera(
  projection: "orthographic" | "perspective",
  preset: ViewPreset | ViewDirection,
  target: Point,
  halfHeight: number,
): ViewCamera {
  const cameras = createCameras();
  setAspect(cameras, W / H);
  const camera = cameras[projection];
  const orbitTarget = new Vector3();
  const direction = typeof preset === "string" ? PRESETS[preset] : preset;
  applyView(camera, { target: new Vector3(...target), ...direction, halfHeight }, orbitTarget);
  camera.lookAt(orbitTarget);
  camera.updateMatrixWorld();
  return camera;
}

function expectClose(actual: number[], expected: number[]) {
  expect(actual).toHaveLength(expected.length);
  actual.forEach((v, i) => expect(v).toBeCloseTo(expected[i], 6));
}

function mappings(camera: ViewCamera, reference: Point) {
  const m = rulerMappings(camera.matrixWorldInverse.elements, camera.projectionMatrix.elements, W, H, reference);
  if (!m) throw new Error("no mapping");
  return m;
}

/** Screen position (CSS px, y down) of a world point, by three's own projection. */
function screenOf(camera: ViewCamera, p: Point): [number, number] {
  const ndc = new Vector3(...p).project(camera);
  return [((ndc.x + 1) / 2) * W, ((1 - ndc.y) / 2) * H];
}

const REFERENCE: Point = [100, 100, 10];
/** The test cube: 20 mm at the platform centre. */
const CUBE: Bounds = { min: [90, 90, 0], max: [110, 110, 20] };
/** Points spread over the scene, at many depths, including the 20 mm cube's corners (90..110, 90..110, 0..20). */
const POINTS: Point[] = [
  [90, 90, 0],
  [110, 110, 20],
  [90, 110, 20],
  [110, 90, 0],
  [37.5, -250, 3.25],
  [250, 400, -42],
  [-1000, 7, 999],
];

describe("rulers in orthographic views", () => {
  // The target is off the reference on purpose: a pan must not shift the labels.
  const target: Point = [80, 60, 30];

  it("labels equal world X and Z in the front view, at every depth", () => {
    const camera = viewCamera("orthographic", "front", target, 30);
    const { horizontal, vertical } = mappings(camera, REFERENCE);
    expect([horizontal.axis, horizontal.sign, vertical.axis, vertical.sign]).toEqual([0, 1, 2, 1]);
    for (const p of POINTS) {
      const [x, y] = screenOf(camera, p);
      expect(valueAt(horizontal, x)).toBeCloseTo(p[0], 6);
      expect(valueAt(vertical, y)).toBeCloseTo(p[2], 6);
    }
  });

  it("a 20 mm cube at X 90..110 spans exactly the 90 and 110 ticks", () => {
    const camera = viewCamera("orthographic", "front", target, 45);
    const { horizontal, vertical } = mappings(camera, REFERENCE);
    expect(screenAt(horizontal, 90)).toBeCloseTo(screenOf(camera, [90, 90, 0])[0], 6);
    expect(screenAt(horizontal, 110)).toBeCloseTo(screenOf(camera, [110, 110, 20])[0], 6);
    expect(screenAt(vertical, 0)).toBeCloseTo(screenOf(camera, [90, 90, 0])[1], 6);
    expect(screenAt(vertical, 20)).toBeCloseTo(screenOf(camera, [110, 110, 20])[1], 6);
    expectClose(boxRange(horizontal, REFERENCE, CUBE), [90, 110]);
    expectClose(boxRange(vertical, REFERENCE, CUBE), [0, 20]);
  });

  it("labels equal world X in the back view, where X runs right to left", () => {
    const camera = viewCamera("orthographic", "back", target, 30);
    const { horizontal, vertical } = mappings(camera, REFERENCE);
    expect([horizontal.axis, horizontal.sign, vertical.axis]).toEqual([0, -1, 2]);
    for (const p of POINTS) {
      const [x, y] = screenOf(camera, p);
      expect(valueAt(horizontal, x)).toBeCloseTo(p[0], 6);
      expect(valueAt(vertical, y)).toBeCloseTo(p[2], 6);
    }
  });

  it.each([
    ["right", 1],
    ["left", -1],
  ] as const)("labels equal world Y and Z in the %s view", (preset, sign) => {
    const camera = viewCamera("orthographic", preset, target, 30);
    const { horizontal, vertical } = mappings(camera, REFERENCE);
    expect([horizontal.axis, horizontal.sign, vertical.axis, vertical.sign]).toEqual([1, sign, 2, 1]);
    for (const p of POINTS) {
      const [x, y] = screenOf(camera, p);
      expect(valueAt(horizontal, x)).toBeCloseTo(p[1], 6);
      expect(valueAt(vertical, y)).toBeCloseTo(p[2], 6);
    }
    expectClose(boxRange(horizontal, REFERENCE, CUBE), [90, 110]);
  });

  it("measures X and Y in the top view", () => {
    const camera = viewCamera("orthographic", "top", target, 30);
    const { horizontal, vertical } = mappings(camera, REFERENCE);
    expect([horizontal.axis, horizontal.sign, vertical.axis, vertical.sign]).toEqual([0, 1, 1, 1]);
    for (const p of POINTS) {
      const [x, y] = screenOf(camera, [p[0], p[1], REFERENCE[2]]);
      expect(valueAt(horizontal, x)).toBeCloseTo(p[0], 6);
      // The top view sits POLE_EPS (1e-4 rad) off the pole, so screen up is Y·cos(1e-4):
      // 5e-9 mm per mm from the reference, 2e-6 mm at the farthest point here.
      expect(Math.abs(valueAt(vertical, y) - p[1])).toBeLessThan(5e-9 * Math.abs(p[1] - REFERENCE[1]) + 1e-7);
    }
  });

  it("has the same scale at every depth and follows the zoom", () => {
    const near = mappings(viewCamera("orthographic", "front", target, 30), [100, -500, 10]);
    const far = mappings(viewCamera("orthographic", "front", target, 30), [100, 500, 10]);
    expect(near.horizontal.pxPerMm).toBeCloseTo(far.horizontal.pxPerMm, 9);
    // Visible height 2 × 30 mm over 800 px.
    expect(near.vertical.pxPerMm).toBeCloseTo(H / 60, 9);
    expect(near.horizontal.pxPerMm).toBeCloseTo(H / 60, 9);
  });
});

describe("rulers in perspective views", () => {
  it("shows real mm at the reference depth", () => {
    const camera = viewCamera("perspective", "front", [120, 90, 30], 60);
    const { horizontal, vertical } = mappings(camera, REFERENCE);
    const depth = new Vector3(...REFERENCE).sub(camera.position).dot(camera.getWorldDirection(new Vector3()));
    const expected = H / (2 * depth * Math.tan((FOV * Math.PI) / 360));
    expect(horizontal.pxPerMm).toBeCloseTo(expected, 9);
    expect(vertical.pxPerMm).toBeCloseTo(expected, 9);
    // Points at the reference depth land on their own coordinates.
    for (const [dx, dz] of [[-10, -10], [10, 10], [37, -4], [-80, 55]]) {
      const p: Point = [REFERENCE[0] + dx, REFERENCE[1], REFERENCE[2] + dz];
      const [x, y] = screenOf(camera, p);
      expect(valueAt(horizontal, x)).toBeCloseTo(p[0], 6);
      expect(valueAt(vertical, y)).toBeCloseTo(p[2], 6);
    }
  });

  it("scales with the reference depth", () => {
    const camera = viewCamera("perspective", "front", [100, 100, 10], 60);
    const close = mappings(camera, [100, 80, 10]).horizontal.pxPerMm;
    const far = mappings(camera, [100, 120, 10]).horizontal.pxPerMm;
    expect(close).toBeGreaterThan(far);
  });

  it("has no scale when the reference is behind the camera", () => {
    const camera = viewCamera("perspective", "front", [100, 100, 10], 60);
    const behind: Point = [100, camera.position.y - 50, 10];
    expect(rulerMappings(camera.matrixWorldInverse.elements, camera.projectionMatrix.elements, W, H, behind)).toBeNull();
  });

  it("has no scale when the reference is nearer than the near plane", () => {
    const camera = viewCamera("perspective", "front", [100, 100, 10], 60);
    const view = camera.matrixWorldInverse.elements;
    const ahead: Point = [100, camera.position.y + 5, 10];
    expect(rulerMappings(view, camera.projectionMatrix.elements, W, H, ahead, screenAxes(view), 4)).not.toBeNull();
    expect(rulerMappings(view, camera.projectionMatrix.elements, W, H, ahead, screenAxes(view), 6)).toBeNull();
  });

  it("falls back to the point straight ahead at the reference's distance", () => {
    const camera = viewCamera("perspective", "iso", [80, 120, 15], 60);
    const view = camera.matrixWorldInverse.elements;
    const forward = camera.getWorldDirection(new Vector3());
    // 70 mm behind the camera and 30 mm off to the side.
    const behind = camera.position.clone().addScaledVector(forward, -70).add(new Vector3(0, 0, 30));
    const reference: Point = [behind.x, behind.y, behind.z];
    expect(rulerMappings(view, camera.projectionMatrix.elements, W, H, reference)).toBeNull();
    const distance = behind.distanceTo(camera.position);
    const ahead = straightAhead(view, reference, 1);
    expectClose(ahead, camera.position.clone().addScaledVector(forward, distance).toArray());
    const { horizontal, vertical } = mappings(camera, ahead);
    const expected = H / (2 * distance * Math.tan((FOV * Math.PI) / 360));
    expect(horizontal.pxPerMm).toBeCloseTo(expected, 9);
    expect(vertical.pxPerMm).toBeCloseTo(expected, 9);
    // It lands mid-screen, labelled with its own coordinates.
    expect(horizontal.origin).toBeCloseTo(W / 2, 6);
    expect(vertical.origin).toBeCloseTo(H / 2, 6);
    // Never nearer than the given minimum.
    expectClose(straightAhead(view, [camera.position.x, camera.position.y, camera.position.z], 12), camera.position.clone().addScaledVector(forward, 12).toArray());
  });
});

describe("which axis a ruler measures", () => {
  const iso = PRESETS.iso;
  /** The iso view orbited by `degrees` around Z; positive turns towards the right view, where screen right is +Y. */
  const orbit = (degrees: number) => ({ theta: iso.theta + (degrees * Math.PI) / 180, phi: iso.phi });
  const axesAt = (direction: ViewDirection, held?: HeldAxes) =>
    screenAxes(viewCamera("perspective", direction, [40, 160, 10], 60).matrixWorldInverse.elements, held);

  it("measures X and Z in the iso view, where screen right is exactly X + Y", () => {
    const { horizontal, vertical } = axesAt(iso);
    expect([horizontal.axis, horizontal.sign, vertical.axis, vertical.sign]).toEqual([0, 1, 2, 1]);
    // Also when the bottom ruler measured Y until now.
    expect(axesAt(iso, { horizontal: 1, vertical: 2 }).horizontal.axis).toBe(0);
  });

  it("keeps X through a small orbit either way from the iso view", () => {
    for (const degrees of [-3.5, -1.44, 1.44, 3.5]) {
      expect(axesAt(orbit(degrees)).horizontal.axis).toBe(0);
      expect(axesAt(orbit(degrees), { horizontal: 0, vertical: 2 }).horizontal.axis).toBe(0);
    }
  });

  it("switches to Y only once clearly past 45°, and back to X at 45°", () => {
    const held: HeldAxes = { horizontal: 0, vertical: 2 };
    const step = (degrees: number) => {
      const { horizontal, vertical } = axesAt(orbit(degrees), held);
      held.horizontal = horizontal.axis;
      held.vertical = vertical.axis;
      return horizontal.axis;
    };
    // Turning towards the right view: X holds to about 4° past the iso view.
    expect([0, 2, 3.5, 4.5, 10, 45].map(step)).toEqual([0, 0, 0, 1, 1, 1]);
    // Turning back: Y holds until the tie, where the bottom ruler's preferred X takes over again.
    expect([10, 3.5, 0.5, 0, 2].map(step)).toEqual([1, 1, 1, 0, 0]);
  });

  it("measures Y on the left ruler only once the camera is clearly above 45°", () => {
    // phi is the angle from +Z: 45° is the tie between Z and Y for screen up in the front direction.
    const front = PRESETS.front.theta;
    const at = (phiDegrees: number, held?: HeldAxes) => axesAt({ theta: front, phi: (phiDegrees * Math.PI) / 180 }, held).vertical.axis;
    expect(at(43)).toBe(2);
    expect(at(40)).toBe(1);
    expect(at(43, { horizontal: 0, vertical: 1 })).toBe(1);
    expect(at(46, { horizontal: 0, vertical: 1 })).toBe(2);
    expect(at(0.01)).toBe(1);
  });
});

describe("tick layout", () => {
  /** A monospaced stand-in for canvas text: 6 px per character. */
  const measure = (label: string) => label.length * 6;
  const GAP = 10;
  const MIN_MINOR = 5;

  function labelsIn(lo: number, hi: number, step: number, decimals: number): string[] {
    const out: string[] = [];
    for (let k = Math.ceil(lo / step); k <= Math.floor(hi / step); k++) out.push(formatValue(k * step, decimals));
    return out;
  }

  it("picks the smallest 1-2-5 step that keeps labels apart", () => {
    // "300" is 18 px wide, so labels need 28 px: 14 mm at 2 px/mm, rounded up to 20.
    expect(chooseTicks(2, 0, 300, measure, GAP, MIN_MINOR)).toEqual({ step: 20, decimals: 0, parts: 4 });
    expect(chooseTicks(40, 99, 101, measure, GAP, MIN_MINOR)).toEqual({ step: 1, decimals: 0, parts: 5 });
    expect(chooseTicks(200, 0, 3, measure, GAP, MIN_MINOR)).toMatchObject({ step: 0.2, decimals: 1 });
    expect(chooseTicks(0.01, -20000, 20000, measure, GAP, MIN_MINOR).step).toBe(5000);
  });

  it("never lets labels overlap, and the next smaller step would", () => {
    const sequence = (step: number) => {
      const exp = Math.floor(Math.log10(step) + 1e-9);
      const m = Math.round(step / 10 ** exp);
      return m === 1 ? 0.5 * 10 ** exp : m === 2 ? 10 ** exp : 2 * 10 ** exp;
    };
    for (let i = 0; i < 400; i++) {
      const pxPerMm = 10 ** (-2 + (i % 40) * 0.13);
      const lo = -((i * 37.3) % 900) / (1 + (i % 7));
      const hi = lo + 1400 / pxPerMm;
      const { step, decimals, parts } = chooseTicks(pxPerMm, lo, hi, measure, GAP, MIN_MINOR);
      const widest = Math.max(0, ...labelsIn(lo, hi, step, decimals).map(measure));
      expect(step * pxPerMm).toBeGreaterThanOrEqual(widest + GAP);
      if (parts > 1) expect((step * pxPerMm) / parts).toBeGreaterThanOrEqual(MIN_MINOR);
      const smaller = sequence(step);
      const smallerDecimals = Math.max(0, -Math.floor(Math.log10(smaller) + 1e-9));
      const smallerWidest = Math.max(0, ...labelsIn(lo, hi, smaller, smallerDecimals).map(measure));
      expect(smaller * pxPerMm).toBeLessThan(smallerWidest + GAP);
    }
  });

  it("counts the wider label next to a round end (99.5 beside 100)", () => {
    // At 0.5 mm steps "99.5" (24 px) is wider than the end labels "0" and "100".
    const { step } = chooseTicks(68, 0, 100.2, measure, GAP, MIN_MINOR);
    expect(step).toBe(0.5);
    expect(chooseTicks(67, 0, 100.2, measure, GAP, MIN_MINOR).step).toBe(1);
  });

  it("subdivides only while minor ticks stay 5 px apart", () => {
    expect(chooseTicks(5, 0, 50, () => 30, GAP, MIN_MINOR)).toMatchObject({ step: 10, parts: 10 });
    expect(chooseTicks(4.5, 0, 50, () => 30, GAP, MIN_MINOR)).toMatchObject({ step: 10, parts: 5 });
    expect(chooseTicks(0.45, 0, 500, () => 30, GAP, MIN_MINOR)).toMatchObject({ step: 100, parts: 5 });
    expect(chooseTicks(0.9, 0, 500, () => 30, GAP, MIN_MINOR)).toMatchObject({ step: 50, parts: 5 });
    expect(chooseTicks(2, 0, 500, () => 30, GAP, 25)).toMatchObject({ step: 20, parts: 1 });
  });

  it("marks major, mid and minor ticks", () => {
    expect([0, 5, 3, -5, -10, -3, 20].map((j) => tickKind(j, 10))).toEqual([0, 1, 2, 1, 0, 2, 0]);
    expect([0, 1, 2, 3].map((j) => tickKind(j, 4))).toEqual([0, 2, 1, 2]);
    expect([0, 1, -1].map((j) => tickKind(j, 2))).toEqual([0, 1, 1]);
    expect([0, 1, 4].map((j) => tickKind(j, 5))).toEqual([0, 2, 2]);
    expect(tickKind(7, 1)).toBe(0);
  });
});

describe("formatValue", () => {
  it("drops trailing zeros and uses a true minus sign", () => {
    expect(formatValue(1.5, 1)).toBe("1.5");
    expect(formatValue(1, 1)).toBe("1");
    expect(formatValue(100, 0)).toBe("100");
    expect(formatValue(100, 2)).toBe("100");
    expect(formatValue(0.30000000000000004, 1)).toBe("0.3");
    expect(formatValue(-12.5, 1)).toBe("−12.5");
    expect(formatValue(-0.04, 1)).toBe("0");
    expect(formatValue(-0, 0)).toBe("0");
    expect(formatValue(0.05, 2)).toBe("0.05");
  });
});

describe("screenAxes across an orbit", () => {
  it("never lets both rulers measure the same axis", () => {
    // Steep and shallow views, each orbited a full turn in small steps with the axes held frame to frame.
    for (const phiDeg of [20, 30, 45, 60, 80]) {
      let held: HeldAxes | undefined;
      for (let deg = 0; deg >= -360; deg -= 0.5) {
        const direction = { theta: (deg * Math.PI) / 180, phi: (phiDeg * Math.PI) / 180 };
        const view = viewCamera("orthographic", direction, REFERENCE, 60).matrixWorldInverse.elements;
        const axes = screenAxes(view, held);
        expect(axes.vertical.axis, `phi ${phiDeg}°, theta ${deg}°`).not.toBe(axes.horizontal.axis);
        held = { horizontal: axes.horizontal.axis, vertical: axes.vertical.axis };
      }
    }
  });
});
