// Saved settings ("Manage settings"): presets by scope (a packing method, an analysis, a mesh tool), then by name,
// each the settings as the dialog had them. Every group keeps its own file in the app's settings folder
// (src-tauri/src/jobs/arrange.rs): packing presets, analysis presets and mesh tool presets. Pure helpers plus the IPC.
import { invoke } from "@tauri-apps/api/core";

export type PresetValues = Record<string, unknown>;

/** Scope, then preset name, then the saved values. */
export type PresetMap = Record<string, Record<string, PresetValues>>;

/** Where one group of presets is kept, and what messages call it. */
export interface PresetStore {
  /** "packing presets", "analysis presets": for messages. */
  what: string;
  load(): Promise<PresetMap>;
  save(presets: PresetMap): Promise<void>;
}

export const PACKING_PRESETS: PresetStore = {
  what: "packing presets",
  load: () => invoke<PresetMap>("packing_presets_load"),
  save: (presets) => invoke("packing_presets_save", { presets }),
};

function group(name: "analysis" | "modify", what: string): PresetStore {
  return {
    what,
    load: () => invoke<PresetMap>("settings_presets_load", { group: name }),
    save: (presets) => invoke("settings_presets_save", { group: name, presets }),
  };
}

export const ANALYSIS_PRESETS = group("analysis", "analysis presets");
export const TOOL_PRESETS = group("modify", "mesh tool presets");

/** `presets` with `values` saved as `name` in `scope`, replacing one of that name. */
export function presetsWith(presets: PresetMap, scope: string, name: string, values: PresetValues): PresetMap {
  return { ...presets, [scope]: { ...(presets[scope] ?? {}), [name]: values } };
}

/** `presets` without `name` in `scope`. */
export function presetsWithout(presets: PresetMap, scope: string, name: string): PresetMap {
  const { [name]: _gone, ...left } = presets[scope] ?? {};
  return { ...presets, [scope]: left };
}

/** The preset names of `scope`, sorted as people read them. */
export function presetNames(presets: PresetMap | null, scope: string): string[] {
  return Object.keys(presets?.[scope] ?? {}).sort((a, b) => a.localeCompare(b));
}

/**
 * The saved values that fit `defaults`: the keys `defaults` has, of the same type (finite numbers only), so a preset
 * saved by an older version, or edited by hand, never puts a wrong value into a dialog.
 */
export function fitPreset<T extends object>(saved: PresetValues, defaults: T): Partial<T> {
  const out: Partial<T> = {};
  for (const key of Object.keys(defaults) as (keyof T & string)[]) {
    const v = saved[key];
    if (typeof v === typeof defaults[key] && (typeof v !== "number" || Number.isFinite(v))) out[key] = v as T[keyof T & string];
  }
  return out;
}
