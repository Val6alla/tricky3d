// The Arrange dialogs' fields as typed, checked into the settings Rust takes, and remembered between runs. Pure, so
// tests run it without the window.
import { loadSetting, saveSetting } from "../store";
import { parseField } from "../transformTools";
import type { DuplicateHow, GridAxis } from "./types";

/** The most copies one Duplicate makes (MAX_COPIES in src-tauri/src/arrange/duplicate.rs). */
export const MAX_COPIES = 10000;

export interface GridAxisFields {
  on: boolean;
  negative: boolean;
  count: string;
  gap: string;
}

export interface DuplicateFields {
  copies: string;
  spacing: "boxes" | "origins";
  axes: [GridAxisFields, GridAxisFields, GridAxisFields];
  distance: string;
  wall: string;
  limit: string;
  stack: boolean;
}

/** The reference's defaults: one copy; a grid of two along X, 1 mm apart; fill 5 mm apart, up to 1000, in 3D. */
export const DEFAULT_DUPLICATE: DuplicateFields = {
  copies: "1",
  spacing: "boxes",
  axes: [
    { on: true, negative: false, count: "2", gap: "1" },
    { on: true, negative: false, count: "1", gap: "1" },
    { on: false, negative: false, count: "1", gap: "1" },
  ],
  distance: "5",
  wall: "0",
  limit: "1000",
  stack: true,
};

export const DUPLICATE_KEY = "tricky3d.duplicate";

/** The fields used last, or the defaults for anything missing or unreadable. */
export function loadDuplicate(): DuplicateFields {
  try {
    const saved = JSON.parse(loadSetting(DUPLICATE_KEY) ?? "null") as Partial<DuplicateFields> | null;
    if (!saved || typeof saved !== "object") return DEFAULT_DUPLICATE;
    const text = (v: unknown, d: string) => (typeof v === "string" ? v : d);
    const axes = DEFAULT_DUPLICATE.axes.map((d, i) => {
      const a = (Array.isArray(saved.axes) ? saved.axes[i] : null) as Partial<GridAxisFields> | null;
      return a
        ? { on: typeof a.on === "boolean" ? a.on : d.on, negative: a.negative === true, count: text(a.count, d.count), gap: text(a.gap, d.gap) }
        : d;
    }) as DuplicateFields["axes"];
    return {
      copies: text(saved.copies, DEFAULT_DUPLICATE.copies),
      spacing: saved.spacing === "origins" ? "origins" : "boxes",
      axes,
      distance: text(saved.distance, DEFAULT_DUPLICATE.distance),
      wall: text(saved.wall, DEFAULT_DUPLICATE.wall),
      limit: text(saved.limit, DEFAULT_DUPLICATE.limit),
      stack: typeof saved.stack === "boolean" ? saved.stack : DEFAULT_DUPLICATE.stack,
    };
  } catch {
    return DEFAULT_DUPLICATE;
  }
}

export function saveDuplicate(fields: DuplicateFields): void {
  saveSetting(DUPLICATE_KEY, JSON.stringify(fields));
}

/** A whole number from `min` to `max`, or null. */
export function wholeNumber(text: string, min: number, max: number): number | null {
  const v = parseField(text);
  return Number.isInteger(v) && v >= min && v <= max ? v : null;
}

/** A length in mm from `min` up, or null. */
export function length(text: string, min = 0): number | null {
  const v = parseField(text);
  return Number.isFinite(v) && v >= min && v <= 100000 ? v : null;
}

export type DuplicateCheck = { how: DuplicateHow; problem: null } | { how: null; problem: string };

const fail = (problem: string): DuplicateCheck => ({ how: null, problem });

/** The settings for `mode`, or what to fix. Axes that are off count as one place. */
export function checkDuplicate(mode: DuplicateHow["mode"], f: DuplicateFields): DuplicateCheck {
  switch (mode) {
    case "in-place": {
      const copies = wholeNumber(f.copies, 1, MAX_COPIES);
      return copies === null ? fail(`Enter 1 to ${MAX_COPIES} copies.`) : { how: { mode, copies }, problem: null };
    }
    case "grid": {
      const axes: GridAxis[] = [];
      for (const [i, a] of f.axes.entries()) {
        const name = "XYZ"[i];
        if (!a.on) {
          axes.push({ count: 1, gap: 0, negative: false });
          continue;
        }
        const count = wholeNumber(a.count, 1, MAX_COPIES + 1);
        if (count === null) return fail(`Enter a whole number of at least 1 for the ${name} count.`);
        const gap = length(a.gap);
        if (gap === null) return fail(`Enter a ${name} gap of 0 mm or more.`);
        axes.push({ count, gap, negative: a.negative });
      }
      const total = axes.reduce((n, a) => n * a.count, 1);
      if (total < 2) return fail("Enter a count above 1 for at least one axis.");
      if (total - 1 > MAX_COPIES) return fail(`That makes ${total - 1} copies; at most ${MAX_COPIES} are possible.`);
      return { how: { mode, axes: axes as [GridAxis, GridAxis, GridAxis], spacing: f.spacing }, problem: null };
    }
    case "fill": {
      const spacing = length(f.distance);
      if (spacing === null) return fail("Enter a minimum part distance of 0 mm or more.");
      const wall = length(f.wall);
      if (wall === null) return fail("Enter a distance to the side walls of 0 mm or more.");
      const limit = wholeNumber(f.limit, 1, MAX_COPIES);
      if (limit === null) return fail(`Enter a limit of 1 to ${MAX_COPIES} copies.`);
      return { how: { mode, spacing, wall, limit, stack: f.stack }, problem: null };
    }
  }
}

/** A grid's places along each axis (1 for an axis that is off). */
export function gridCounts(f: DuplicateFields): number[] {
  return f.axes.map((a) => (a.on ? (wholeNumber(a.count, 1, MAX_COPIES + 1) ?? 1) : 1));
}

/** How far a grid reaches along each axis, mm, for the selection's box `size`: what the dialog shows as its size. */
export function gridExtent(f: DuplicateFields, size: readonly number[]): number[] {
  return f.axes.map((a, i) => {
    const n = a.on ? (wholeNumber(a.count, 1, MAX_COPIES + 1) ?? 1) : 1;
    const gap = a.on ? (length(a.gap) ?? 0) : 0;
    return f.spacing === "boxes" ? n * size[i] + (n - 1) * gap : (n - 1) * gap + size[i];
  });
}
