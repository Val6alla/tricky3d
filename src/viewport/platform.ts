import { Group, LineBasicMaterial, MathUtils } from "three";
import type { Vec3 } from "../store";
import { disposeLabel, labelSprite, makeTextLabel, type TextLabel } from "./labels";
import type { VpTheme } from "./theme";
import { lineSegments, type SceneLayer } from "./util";

/** Per axis. Past this a 1 mm grid is only moiré, so it is left out. */
const MAX_GRID_LINES = 5000;

/** Build platform spanning 0..size: wireframe box, floor grid, and rulers on the front and left edges. */
export function buildPlatform(size: Vec3, theme: VpTheme): SceneLayer {
  const [sx, sy] = size;
  const floorSide = Math.max(sx, sy);
  const tick = MathUtils.clamp(Math.max(...size) * 0.01, 1, 20);
  const labelHeight = MathUtils.clamp(floorSide * 0.03, 3, 80);

  const materials = {
    box: new LineBasicMaterial(),
    minor: new LineBasicMaterial(),
    major: new LineBasicMaterial(),
    ruler: new LineBasicMaterial(),
  };
  const minor = lineSegments(gridPositions(sx, sy, 1, 10), materials.minor);
  const major = lineSegments(gridPositions(sx, sy, 10, 0), materials.major);
  const box = lineSegments(boxPositions(size), materials.box);
  const ticks = lineSegments(rulerTicks(sx, sy, tick), materials.ruler);
  // Where lines coincide at equal depth the later draw wins, so the box edge goes last.
  major.renderOrder = 1;
  ticks.renderOrder = 1;
  box.renderOrder = 2;

  const group = new Group();
  group.add(minor, major, ticks, box);
  const labels = addRulerLabels(group, sx, sy, tick * 2 + labelHeight * 0.3, labelHeight, theme.font);
  const lines = [minor, major, ticks, box];

  function applyTheme(t: VpTheme) {
    materials.box.color.set(t.platform);
    materials.minor.color.set(t.gridMinor);
    materials.major.color.set(t.gridMajor);
    materials.ruler.color.set(t.ruler);
    for (const label of labels) label.material.color.set(t.ruler);
  }
  applyTheme(theme);

  return {
    group,
    applyTheme,
    dispose() {
      for (const line of lines) line.geometry.dispose();
      for (const material of Object.values(materials)) material.dispose();
      for (const label of labels) disposeLabel(label);
    },
  };
}

function boxPositions([x, y, z]: Vec3): Float32Array {
  const corners = [
    [0, 0, 0], [x, 0, 0], [x, y, 0], [0, y, 0],
    [0, 0, z], [x, 0, z], [x, y, z], [0, y, z],
  ];
  const edges = [0, 1, 1, 2, 2, 3, 3, 0, 4, 5, 5, 6, 6, 7, 7, 4, 0, 4, 1, 5, 2, 6, 3, 7];
  return new Float32Array(edges.flatMap((i) => corners[i]));
}

/**
 * Floor lines at z = 0 every `step` mm inside 0..x by 0..y. The platform edges are left to the box,
 * and with `skipEvery` every n-th line is left to the coarser grid.
 */
function gridPositions(x: number, y: number, step: number, skipEvery: number): Float32Array {
  if (x / step > MAX_GRID_LINES || y / step > MAX_GRID_LINES) return new Float32Array(0);
  const xs = gridStops(x, step, skipEvery);
  const ys = gridStops(y, step, skipEvery);
  const out = new Float32Array((xs.length + ys.length) * 6);
  let o = 0;
  for (const v of xs) {
    out.set([v, 0, 0, v, y, 0], o);
    o += 6;
  }
  for (const v of ys) {
    out.set([0, v, 0, x, v, 0], o);
    o += 6;
  }
  return out;
}

function gridStops(extent: number, step: number, skipEvery: number): number[] {
  const out: number[] = [];
  for (let i = 1; i * step < extent - 1e-6; i++) {
    if (skipEvery && i % skipEvery === 0) continue;
    out.push(i * step);
  }
  return out;
}

/** Ticks pointing outwards: along X from the front edge (y = 0) and along Y from the left edge (x = 0). */
function rulerTicks(x: number, y: number, tick: number): Float32Array {
  const out: number[] = [];
  for (const { at, long } of tickStops(x)) out.push(at, 0, 0, at, long ? -2 * tick : -tick, 0);
  for (const { at, long } of tickStops(y)) out.push(0, at, 0, long ? -2 * tick : -tick, at, 0);
  return new Float32Array(out);
}

/** A tick every 10 mm, long every 50 mm and at the far end. */
function tickStops(extent: number): { at: number; long: boolean }[] {
  const out: { at: number; long: boolean }[] = [];
  for (let i = 0; i * 10 <= extent + 1e-6; i++) out.push({ at: i * 10, long: i % 5 === 0 });
  const last = out[out.length - 1];
  if (extent - last.at > 1e-6) out.push({ at: extent, long: true });
  else last.long = true;
  return out;
}

/** Numeric labels every 50 mm (coarser on huge platforms) and at the far end; returns the labels to dispose. */
function addRulerLabels(group: Group, x: number, y: number, gap: number, height: number, font: string): TextLabel[] {
  const cache = new Map<string, TextLabel>();
  const labelFor = (value: number) => {
    const text = String(Math.round(value * 10) / 10);
    let label = cache.get(text);
    if (!label) {
      label = makeTextLabel(text, font);
      cache.set(text, label);
    }
    return label;
  };
  for (const at of labelStops(x, height * 2)) {
    const sprite = labelSprite(labelFor(at), height);
    sprite.position.set(at, -(gap + height / 2), 0);
    group.add(sprite);
  }
  for (const at of labelStops(y, height * 2)) {
    const label = labelFor(at);
    const sprite = labelSprite(label, height);
    // Right-aligned against the ruler so the numbers line up in the top view.
    sprite.position.set(-(gap + (height * label.aspect) / 2), at, 0);
    group.add(sprite);
  }
  return [...cache.values()];
}

/** 0 is left out: the axes start there. The last regular label gives way if it would crowd the end label. */
function labelStops(extent: number, minGap: number): number[] {
  const step = labelStep(extent);
  const out: number[] = [];
  for (let at = step; at <= extent + 1e-6; at += step) out.push(at);
  const last = out.length ? out[out.length - 1] : 0;
  if (extent - last > 1e-6) {
    if (out.length && extent - last < minGap) out.pop();
    out.push(extent);
  }
  return out;
}

function labelStep(extent: number): number {
  for (const step of [50, 100, 250, 500, 1000, 2500, 5000]) {
    if (extent / step <= 24) return step;
  }
  return Math.ceil(extent / 24 / 10000) * 10000;
}
