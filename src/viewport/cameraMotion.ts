import { Vector3 } from "three";
import type { ViewState } from "./cameras";

const DURATION_MS = 350;

export interface CameraMotion {
  start(from: ViewState, to: ViewState): void;
  cancel(): void;
  /** Where the running move ends, or null when none is running. */
  destination(): ViewState | null;
  /** Applies the view for time `now` (ms); true while more frames are needed. */
  step(now: number): boolean;
}

/**
 * Animated camera moves. Azimuth and elevation are interpolated separately (spherical coordinates
 * around the target), so Z stays up on screen throughout and the poles never flip the view. The
 * size changes geometrically, which reads as an even zoom.
 */
export function createCameraMotion(apply: (view: ViewState) => void): CameraMotion {
  let run: { from: ViewState; to: ViewState; startedAt: number } | null = null;

  return {
    start(from, to) {
      const theta = from.theta + shortestTurn(to.theta - from.theta);
      run = { from, to: { ...to, theta }, startedAt: performance.now() };
    },
    cancel() {
      run = null;
    },
    destination() {
      return run ? run.to : null;
    },
    step(now) {
      if (!run) return false;
      const t = Math.min(1, Math.max(0, (now - run.startedAt) / DURATION_MS));
      const { to } = run;
      if (t >= 1) run = null;
      apply(run ? blend(run.from, to, easeInOut(t)) : to);
      return run !== null;
    },
  };
}

function blend(a: ViewState, b: ViewState, t: number): ViewState {
  return {
    target: new Vector3().lerpVectors(a.target, b.target, t),
    theta: a.theta + (b.theta - a.theta) * t,
    phi: a.phi + (b.phi - a.phi) * t,
    halfHeight: a.halfHeight * Math.pow(b.halfHeight / a.halfHeight, t),
  };
}

/** The same turn wrapped into [-π, π]. */
function shortestTurn(delta: number): number {
  const d = delta % (2 * Math.PI);
  if (d > Math.PI) return d - 2 * Math.PI;
  if (d < -Math.PI) return d + 2 * Math.PI;
  return d;
}

function easeInOut(t: number): number {
  return t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
}
