// Pack: the methods, their fields as the dialog holds them, checked into the settings Rust takes
// (src-tauri/src/arrange/pack.rs Method), the presets kept in the app's settings folder, and the run itself: the parts
// packed, Rust's answer applied as one undo step, and those that do not fit named.
import { invoke } from "@tauri-apps/api/core";
import { loadSetting, saveSetting, useStore, type Part } from "../store";
import { parseField, transformTargets } from "../transformTools";
import { toast, useUi } from "../ui";
import { arrangeJob, refusedNow } from "./session";

export type PackMethod = "bounding-box" | "scanline2d" | "scanline3d" | "monte-carlo";

/** The Pack menu's entries in the reference's order; the last three are Soon. */
export const PACK_METHODS: { value: PackMethod | "gravity" | "true-shape-2d" | "true-shape-3d"; label: string; soon?: boolean }[] = [
  { value: "bounding-box", label: "3D packing: Bounding box" },
  { value: "scanline2d", label: "2D packing: Scanline" },
  { value: "scanline3d", label: "3D packing: Scanline" },
  { value: "monte-carlo", label: "3D packing: Monte Carlo" },
  { value: "gravity", label: "3D packing: Gravity", soon: true },
  { value: "true-shape-2d", label: "2D packing: True Shape", soon: true },
  { value: "true-shape-3d", label: "3D packing: True Shape", soon: true },
];

export const methodLabel = (m: PackMethod) => PACK_METHODS.find((p) => p.value === m)?.label ?? m;

/** Z-rotation steps: turns tried per full turn (1 is none), as the reference lists them by angle. */
export const Z_STEPS: { value: number; label: string }[] = [
  { value: 1, label: "None" },
  { value: 2, label: "180°" },
  { value: 4, label: "90°" },
  { value: 6, label: "60°" },
  { value: 8, label: "45°" },
  { value: 12, label: "30°" },
  { value: 18, label: "20°" },
];

/** Every field of every method, as typed. One object, so switching methods keeps what was typed. */
export interface PackFields {
  method: PackMethod;
  /** 1 (accurate) to 9 (fast). */
  performance: number;
  distance: string;
  voxel: string;
  wall: string;
  floor: string;
  ceiling: string;
  stack: boolean;
  align: boolean;
  zSteps: number;
  /** 2D: cells added round each outline, 0 (none), 1 (small) or 2 (large). */
  spacing: number;
  flip: boolean;
  limitHeight: boolean;
  height: string;
  rotation: "free" | "z" | "none";
  seed: string;
  onlySelected: boolean;
}

/** The reference's defaults, method by method: they share a field where they agree. */
export const DEFAULT_PACK: Record<PackMethod, PackFields> = (() => {
  const base: PackFields = {
    method: "bounding-box",
    performance: 5,
    distance: "1",
    voxel: "5",
    wall: "0",
    floor: "0",
    ceiling: "0",
    stack: true,
    align: true,
    zSteps: 12,
    spacing: 1,
    flip: true,
    limitHeight: false,
    height: "100",
    rotation: "z",
    seed: "1",
    onlySelected: false,
  };
  return {
    "bounding-box": base,
    scanline2d: { ...base, method: "scanline2d", voxel: "1", wall: "4" },
    scanline3d: { ...base, method: "scanline3d", voxel: "2" },
    "monte-carlo": { ...base, method: "monte-carlo" },
  };
})();

export const PACK_KEY = "tricky3d.pack";

/** What is remembered: the method used last, and each method's own fields (their ranges differ). */
interface SavedPack {
  method?: PackMethod;
  fields?: Partial<Record<PackMethod, Partial<PackFields>>>;
}

function loadSaved(): SavedPack {
  try {
    const saved = JSON.parse(loadSetting(PACK_KEY) ?? "{}") as SavedPack | null;
    return saved && typeof saved === "object" ? saved : {};
  } catch {
    return {};
  }
}

const isMethod = (m: unknown): m is PackMethod => typeof m === "string" && m in DEFAULT_PACK;

/** `method`'s fields as used last, else its defaults; without `method`, the method used last. */
export function loadPack(method?: PackMethod): PackFields {
  const saved = loadSaved();
  const m = method ?? (isMethod(saved.method) ? saved.method : "bounding-box");
  const own = saved.fields?.[m];
  return { ...DEFAULT_PACK[m], ...(own && typeof own === "object" ? pickFields(own) : {}), method: m };
}

/** Only the known fields of the right types, so a damaged or older entry cannot break the dialog. */
export function pickFields(raw: Partial<PackFields>): Partial<PackFields> {
  const out: Partial<PackFields> = {};
  const d = DEFAULT_PACK["bounding-box"];
  for (const key of Object.keys(d) as (keyof PackFields)[]) {
    if (key === "method") continue;
    if (raw[key] !== undefined && typeof raw[key] === typeof d[key]) (out as Record<string, unknown>)[key] = raw[key];
  }
  return out;
}

/** Remembers `fields` as their method's, and that method as the one used last. */
export function savePack(fields: PackFields): void {
  const saved = loadSaved();
  saveSetting(PACK_KEY, JSON.stringify({ method: fields.method, fields: { ...(saved.fields ?? {}), [fields.method]: fields } }));
}

const mm = (text: string): number | null => {
  const v = parseField(text);
  return Number.isFinite(v) && v >= 0 && v <= 100000 ? v : null;
};

export type PackCheck = { settings: Record<string, unknown>; problem: null } | { settings: null; problem: string };

/** The `pack` settings for Rust, or what to fix. `platformHeight` turns a height limit into a ceiling distance. */
export function checkPack(f: PackFields, platformHeight: number): PackCheck {
  const fail = (problem: string): PackCheck => ({ settings: null, problem });
  const wall = mm(f.wall);
  if (wall === null) return fail("Enter a distance to the side walls of 0 mm or more.");
  const floor = mm(f.floor);
  if (floor === null) return fail("Enter a distance to the platform of 0 mm or more.");
  let ceiling = 0;
  if (f.method === "bounding-box") {
    const c = mm(f.ceiling);
    if (c === null) return fail("Enter a distance to the ceiling of 0 mm or more.");
    ceiling = c;
  } else if (f.limitHeight) {
    const h = mm(f.height);
    if (h === null || h <= 0 || h > platformHeight) return fail(`Enter a height limit above 0 and up to ${platformHeight} mm.`);
    ceiling = platformHeight - h;
  }
  const voxel = mm(f.voxel);
  switch (f.method) {
    case "bounding-box": {
      const distance = mm(f.distance);
      if (distance === null) return fail("Enter a minimum part distance of 0 mm or more.");
      if (voxel === null || voxel < 0.001) return fail("Enter a voxel size of at least 0.001 mm.");
      return { settings: { method: f.method, distance, voxel, wall, floor, ceiling, stack: f.stack }, problem: null };
    }
    case "scanline2d":
      if (voxel === null || voxel < 0.5 || voxel > 2.5) return fail("Pick a voxel size from 0.5 to 2.5 mm.");
      return {
        settings: { method: f.method, performance: f.performance, align: f.align, zSteps: f.zSteps, wall, spacing: f.spacing, voxel },
        problem: null,
      };
    case "scanline3d":
      if (voxel === null || voxel < 1 || voxel > 5) return fail("Pick a voxel size from 1 to 5 mm.");
      return {
        settings: { method: f.method, performance: f.performance, voxel, zSteps: f.zSteps, flip: f.flip, wall, floor, ceiling },
        problem: null,
      };
    case "monte-carlo": {
      const distance = mm(f.distance);
      if (distance === null) return fail("Enter a minimum distance between parts of 0 mm or more.");
      const seed = parseField(f.seed);
      if (!Number.isInteger(seed) || seed < 0 || seed > Number.MAX_SAFE_INTEGER) return fail("Enter a whole number of 0 or more for the seed.");
      return {
        settings: { method: f.method, performance: f.performance, distance, rotation: f.rotation, wall, floor, ceiling, seed },
        problem: null,
      };
    }
  }
}

/** The parts a pack moves (shown, unlocked; only the selected ones if asked) and the ones that stay. */
export function packTargets(parts: Part[], selection: string[], onlySelected: boolean): { targets: Part[]; others: Part[] } {
  const chosen = onlySelected ? transformTargets({ parts, selection }).parts : parts.filter((p) => p.visible && !p.locked);
  const ids = new Set(chosen.map((p) => p.id));
  return { targets: chosen, others: parts.filter((p) => !ids.has(p.id)) };
}

/** Home › Pack ▸ a method, Arrange › Packing: opens the dialog, or says why not. */
export function openPack(method?: PackMethod): void {
  if (refusedNow("Packing")) return;
  const s = useStore.getState();
  if (!s.parts.some((p) => p.visible && !p.locked)) {
    toast(s.parts.length ? "Every part is locked or hidden: there is nothing to pack." : "There are no parts to pack.");
    return;
  }
  useUi.getState().openDialog({ kind: "pack", method: method ?? loadPack().method });
}

const percent = (v: number) => `${(v * 100).toFixed(1)} %`;

/**
 * Packs with the dialog's fields: one undo step "Pack" when anything moved. Parts that do not fit go beside the platform
 * and are named. Returns the answer's numbers, or null when it was cancelled, refused or failed (said in a toast).
 */
export async function runPack(fields: PackFields): Promise<{ packed: number; aside: string[]; height: number; density: number } | null> {
  const s = useStore.getState();
  const check = checkPack(fields, s.platform[2]);
  if (!check.settings) {
    toast(check.problem, "warn");
    return null;
  }
  if (refusedNow("Packing")) return null;
  const { targets, others } = packTargets(s.parts, s.selection, fields.onlySelected);
  if (targets.length === 0) {
    toast(fields.onlySelected ? "Select the shown, unlocked parts to pack first." : "There are no shown, unlocked parts to pack.");
    return null;
  }
  savePack(fields);
  const answer = await arrangeJob("Pack", "pack", { pack: check.settings }, targets, others);
  if (!answer) return null;
  const list = (answer.matrices ?? []) as number[][];
  const fits = (answer.fits ?? []) as boolean[];
  const matrices = new Map<string, number[]>();
  targets.forEach((p, i) => {
    if (list[i]?.length === 16) matrices.set(p.id, list[i]);
  });
  useStore.getState().setPartMatrices(matrices, "Pack");
  const aside = targets.filter((_, i) => fits[i] === false).map((p) => p.name);
  const height = Number(answer.height ?? 0);
  const density = Number(answer.density ?? 0);
  const packed = targets.length - aside.length;
  const summary = `Packed ${packed === 1 ? "1 part" : `${packed} parts`}: ${height.toFixed(1)} mm high, ${percent(density)} of the room up to there.`;
  if (aside.length) {
    const names = aside.length > 6 ? `${aside.slice(0, 6).join(", ")} and ${aside.length - 6} more` : aside.join(", ");
    toast(`${summary} ${aside.length === 1 ? "1 part does" : `${aside.length} parts do`} not fit and ${aside.length === 1 ? "is" : "are"} beside the platform: ${names}.`, "warn");
  } else {
    toast(summary);
  }
  return { packed, aside, height, density };
}

// ---------------------------------------------------------------------------------------------
// Presets: by method, then by name, in the app's settings folder (src-tauri/src/jobs/arrange.rs PRESETS_FILE).

export type Presets = Record<string, Record<string, Partial<PackFields>>>;

export function loadPresets(): Promise<Presets> {
  return invoke<Presets>("packing_presets_load");
}

export function savePresets(presets: Presets): Promise<void> {
  return invoke("packing_presets_save", { presets });
}

/** `presets` with the fields saved as `name` for the fields' method (replacing one of that name). */
export function withPreset(presets: Presets, name: string, fields: PackFields): Presets {
  const { method, ...rest } = fields;
  return { ...presets, [method]: { ...(presets[method] ?? {}), [name]: rest } };
}

/** `presets` without `name` for `method`. */
export function withoutPreset(presets: Presets, method: PackMethod, name: string): Presets {
  const { [name]: _gone, ...left } = presets[method] ?? {};
  return { ...presets, [method]: left };
}
