// Where a label lies, for drawing it and dragging its handles. Rust puts the label into the part
// (src-tauri/src/label/emboss.rs Frame::label); this repeats its frame so the drawing matches.
import type { Vec3 } from "../store";

export interface Frame {
  origin: Vec3;
  /** Along the text. */
  u: Vec3;
  /** Up the text. */
  v: Vec3;
  /** Out of the surface. */
  w: Vec3;
}

export const add = (a: Vec3, b: Vec3): Vec3 => [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
export const sub = (a: Vec3, b: Vec3): Vec3 => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
export const scale = (a: Vec3, s: number): Vec3 => [a[0] * s, a[1] * s, a[2] * s];
export const dot = (a: Vec3, b: Vec3): number => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
export const cross = (a: Vec3, b: Vec3): Vec3 => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];

export function unit(a: Vec3): Vec3 | null {
  const l = Math.hypot(a[0], a[1], a[2]);
  return l > 1e-12 && Number.isFinite(l) ? scale(a, 1 / l) : null;
}

/**
 * The frame of a label at `point` on a surface facing `normal`: w along the normal, v "up" on the surface (world Z laid
 * onto it, or world Y on a level surface) and u = v × w along the text, both turned `angle` degrees about w.
 */
export function labelFrame(point: Vec3, normal: Vec3, angle: number): Frame | null {
  const w = unit(normal);
  if (!w) return null;
  const up: Vec3 = Math.abs(w[2]) < 0.99 ? [0, 0, 1] : [0, 1, 0];
  const v0 = unit(sub(up, scale(w, dot(up, w))));
  if (!v0) return null;
  const u0 = cross(v0, w);
  const a = (angle * Math.PI) / 180;
  const u = add(scale(u0, Math.cos(a)), scale(v0, Math.sin(a)));
  return { origin: point, u, v: cross(w, u), w };
}

/** A point of the frame's plane (`x` along u, `y` along v) lifted `z` along w, in world space. */
export function at(f: Frame, x: number, y: number, z = 0): Vec3 {
  return add(f.origin, add(scale(f.u, x), add(scale(f.v, y), scale(f.w, z))));
}

/** The point where a ray meets the frame's plane, as (x, y) in it; null when the ray runs along the plane. */
export function onPlane(f: Frame, origin: Vec3, direction: Vec3): [number, number] | null {
  const d = dot(direction, f.w);
  if (Math.abs(d) < 1e-9) return null;
  const t = dot(sub(f.origin, origin), f.w) / d;
  const p = sub(add(origin, scale(direction, t)), f.origin);
  return [dot(p, f.u), dot(p, f.v)];
}

/** The tag plate's size round a caption (src-tauri/src/label/tag.rs plate_size): plate width and height. */
export function plateSize(width: number, height: number): [number, number] {
  const margin = Math.max(0.2 * height, 1);
  return [width + 2 * margin, height + 2 * margin];
}

/** How far the tag plate stands off the surface (tag.rs GAP). */
export const TAG_GAP = 1;

/**
 * The tag plate face the caption is drawn on (tag.rs caption_face), for a tag frame `tag` (u along the text, v across
 * the plate, w the direction it stands out in): its front when it stands up from a level surface, else its upper face
 * with the caption's top towards the wall.
 */
export function captionFace(tag: Frame, thickness: number, plateH: number): Frame {
  const middle = TAG_GAP + plateH / 2;
  if (Math.abs(tag.w[2]) >= Math.cos(Math.PI / 4)) {
    return { origin: at(tag, 0, -thickness / 2, middle), u: tag.u, v: tag.w, w: scale(tag.v, -1) };
  }
  const side = tag.v[2] >= 0 ? 1 : -1;
  const w = scale(tag.v, side);
  const v = scale(tag.w, -1);
  return { origin: at(tag, 0, (side * thickness) / 2, middle), u: cross(v, w), v, w };
}
