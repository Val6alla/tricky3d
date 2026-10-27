// Repair scripts in the window. The first half is pure: the script model behind the Repair scripts tab and the
// script picker (picker order, new, rename, delete, save as copy, steps and their parameters, validation against
// the catalog). The second half is the glue that loads and saves through src/repair/ipc.ts into the repair store.
// Built-in scripts come from the catalog (src-tauri/src/repair/defaults.rs) and are never saved; the user's own
// scripts and the picker order of all scripts live in the config folder's repair-scripts.json.
import { isTauri } from "@tauri-apps/api/core";
import { errorMessage } from "../io/files";
import { parseNumber } from "../numbers";
import { toast } from "../ui";
import { exportScriptFile, fetchCatalog, importScriptFile, loadStoredScripts, saveStoredScripts } from "./ipc";
import { useRepair } from "./store";
import type { ActionId, ActionSpec, Catalog, ParamSpec, ParamValue, RepairScript, ScriptStep, StoredScripts } from "./types";

/** Ids of built-in scripts start with this; the user's own with OWN_PREFIX. */
export const BUILTIN_PREFIX = "builtin.";
export const OWN_PREFIX = "own.";

/** The name a new script gets when none is typed. */
export const NEW_SCRIPT_NAME = "New script";

// ---------------------------------------------------------------------------------------------
// Catalog lookups (pure)

/** The catalog entry of an action; undefined when the catalog does not know the id. */
export function actionSpec(catalog: Catalog, id: string): ActionSpec | undefined {
  return catalog.actions.find((a) => a.id === id);
}

/** The add-action dropdown: every action a script may hold, in catalog order; unimplemented ones show a Soon badge. */
export function scriptActions(catalog: Catalog): ActionSpec[] {
  return catalog.actions.filter((a) => a.inScripts);
}

/** True when the action can be added to a script: listed for scripts and implemented. */
export function canAddAction(spec: ActionSpec | undefined): spec is ActionSpec {
  return spec !== undefined && spec.inScripts && spec.implemented;
}

/** Every parameter of the action at its catalog default. */
export function defaultParams(spec: ActionSpec): Record<string, ParamValue> {
  return Object.fromEntries(spec.params.map((p) => [p.id, p.default]));
}

// ---------------------------------------------------------------------------------------------
// Scripts (pure)

/**
 * All scripts in picker order: the catalog's built-ins and the user's own, sorted by `order`. Scripts the order
 * does not list keep their sequence (built-ins first) and follow the listed ones.
 */
export function orderedScripts(catalog: Catalog | null, own: readonly RepairScript[], order: readonly string[]): RepairScript[] {
  const all = [...(catalog?.scripts ?? []), ...own];
  const rank = new Map(order.map((id, i) => [id, i]));
  const key = (s: RepairScript, i: number) => rank.get(s.id) ?? order.length + i;
  return all
    .map((script, i) => ({ script, key: key(script, i) }))
    .sort((a, b) => a.key - b.key)
    .map((x) => x.script);
}

/** The picker order after moving script `id` one place up (-1) or down (+1); `scripts` are all scripts in picker order. */
export function moveInOrder(scripts: readonly RepairScript[], id: string, delta: -1 | 1): string[] {
  const ids = scripts.map((s) => s.id);
  const from = ids.indexOf(id);
  const to = from + delta;
  if (from < 0 || to < 0 || to >= ids.length) return ids;
  [ids[from], ids[to]] = [ids[to], ids[from]];
  return ids;
}

/** A fresh id for a new own script: "own.<timestamp>", with a counter added when that is taken. */
export function newScriptId(taken: Iterable<string>, now: number = Date.now()): string {
  const used = new Set(taken);
  const base = `${OWN_PREFIX}${now}`;
  let id = base;
  for (let k = 2; used.has(id); k++) id = `${base}-${k}`;
  return id;
}

/** `name` when no script uses it, otherwise "name 2", "name 3"... */
export function uniqueName(name: string, names: Iterable<string>): string {
  const used = new Set(names);
  let out = name;
  for (let k = 2; used.has(out); k++) out = `${name} ${k}`;
  return out;
}

/** A new empty own script; a blank name becomes "New script". The name is made unique among `existing`. */
export function createScript(name: string, existing: readonly RepairScript[], now?: number): RepairScript {
  return {
    id: newScriptId(
      existing.map((s) => s.id),
      now,
    ),
    name: uniqueName(name.trim() || NEW_SCRIPT_NAME, existing.map((s) => s.name)),
    builtIn: false,
    actions: [],
  };
}

/** The script with a new name; a blank name changes nothing (the same object comes back). */
export function renameScript(script: RepairScript, name: string): RepairScript {
  const trimmed = name.trim();
  return trimmed && trimmed !== script.name ? { ...script, name: trimmed } : script;
}

/**
 * Save as copy: an own script with the same steps, a fresh id, and `name` (default "<name> (copy)") made unique
 * among `existing`. This is how a built-in script is edited.
 */
export function copyScript(script: RepairScript, existing: readonly RepairScript[], name?: string, now?: number): RepairScript {
  return {
    id: newScriptId(
      existing.map((s) => s.id),
      now,
    ),
    name: uniqueName(name?.trim() || `${script.name} (copy)`, existing.map((s) => s.name)),
    builtIn: false,
    actions: script.actions.map((step) => ({ ...step, params: { ...step.params } })),
  };
}

/** True when two scripts have the same name and steps (same actions, switches and parameter values, in order). */
export function sameScript(a: RepairScript, b: RepairScript): boolean {
  if (a.name !== b.name || a.actions.length !== b.actions.length) return false;
  return a.actions.every((s, i) => {
    const t = b.actions[i];
    const keys = Object.keys(s.params);
    return (
      s.type === t.type &&
      s.enabled === t.enabled &&
      keys.length === Object.keys(t.params).length &&
      keys.every((k) => Object.prototype.hasOwnProperty.call(t.params, k) && t.params[k] === s.params[k])
    );
  });
}

// ---------------------------------------------------------------------------------------------
// Steps and parameters (pure). Every function returns a new script, or the same object when nothing changes.

/** A new step for the action, enabled, with every parameter at its catalog default. */
export function newStep(spec: ActionSpec): ScriptStep {
  return { type: spec.id, enabled: true, params: defaultParams(spec) };
}

/** The script with the action appended at its defaults; unchanged when the action cannot be added (Soon, unknown). */
export function addStep(script: RepairScript, action: ActionId, catalog: Catalog): RepairScript {
  const spec = actionSpec(catalog, action);
  return canAddAction(spec) ? { ...script, actions: [...script.actions, newStep(spec)] } : script;
}

export function removeStep(script: RepairScript, index: number): RepairScript {
  if (!(index >= 0 && index < script.actions.length)) return script;
  return { ...script, actions: script.actions.filter((_, i) => i !== index) };
}

/** The step at `from` moved to position `to` (clamped to the list), the others keeping their order. */
export function moveStep(script: RepairScript, from: number, to: number): RepairScript {
  const n = script.actions.length;
  if (!(from >= 0 && from < n)) return script;
  const target = Math.max(0, Math.min(n - 1, Math.trunc(to)));
  if (target === from) return script;
  const actions = [...script.actions];
  const [step] = actions.splice(from, 1);
  actions.splice(target, 0, step);
  return { ...script, actions };
}

export function toggleStep(script: RepairScript, index: number): RepairScript {
  if (!(index >= 0 && index < script.actions.length)) return script;
  return { ...script, actions: script.actions.map((s, i) => (i === index ? { ...s, enabled: !s.enabled } : s)) };
}

/** Rounds to `decimals` places the way the editor shows the value (0.0104 with 3 → 0.01). */
function roundTo(value: number, decimals: number): number {
  return Number(value.toFixed(Math.max(0, Math.min(20, decimals))));
}

/**
 * `value` made valid for the parameter: a number (a typed decimal comma is fine) rounded to the decimals the editor
 * shows and clamped to the minimum; Yes/No as a boolean; a choice id among the choices. Anything unusable becomes
 * the default.
 */
export function normalizeParam(spec: ParamSpec, value: unknown): ParamValue {
  switch (spec.kind) {
    case "float": {
      const n = typeof value === "number" ? value : typeof value === "string" ? parseNumber(value) : null;
      if (n === null || !Number.isFinite(n)) return spec.default;
      const rounded = spec.decimals === undefined ? n : roundTo(n, spec.decimals);
      return spec.min === undefined ? rounded : Math.max(spec.min, rounded);
    }
    case "bool":
      return typeof value === "boolean" ? value : spec.default;
    case "choice":
      return typeof value === "string" && (spec.choices ?? []).some((c) => c.id === value) ? value : spec.default;
  }
}

/**
 * The script with one parameter of step `index` set, normalised by normalizeParam. Unchanged when the step or the
 * parameter is unknown, or the value stays the same.
 */
export function setStepParam(script: RepairScript, index: number, paramId: string, value: unknown, catalog: Catalog): RepairScript {
  const step = script.actions[index];
  const spec = step && actionSpec(catalog, step.type)?.params.find((p) => p.id === paramId);
  if (!step || !spec) return script;
  const next = normalizeParam(spec, value);
  if (step.params[paramId] === next) return script;
  return { ...script, actions: script.actions.map((s, i) => (i === index ? { ...s, params: { ...s.params, [paramId]: next } } : s)) };
}

/**
 * The step with exactly its action's catalog parameters, each normalised (missing ones at the default, unknown ones
 * dropped). A step whose action the catalog does not know comes back as it is.
 */
export function normalizeStep(step: ScriptStep, catalog: Catalog): ScriptStep {
  const spec = actionSpec(catalog, step.type);
  if (!spec) return step;
  const params = Object.fromEntries(spec.params.map((p) => [p.id, p.id in step.params ? normalizeParam(p, step.params[p.id]) : p.default]));
  return { type: step.type, enabled: step.enabled !== false, params };
}

/** Every step normalised (see normalizeStep). */
export function normalizeScript(script: RepairScript, catalog: Catalog): RepairScript {
  return { ...script, actions: script.actions.map((s) => normalizeStep(s, catalog)) };
}

/** Why a parameter value cannot run, as a clause ("Tolerance must be a number of 0 mm or more"); null when it can. */
function paramProblem(spec: ParamSpec, value: unknown): string | null {
  switch (spec.kind) {
    case "float": {
      const ok = typeof value === "number" && Number.isFinite(value) && (spec.min === undefined || value >= spec.min);
      if (ok) return null;
      const unit = spec.unit ? ` ${spec.unit}` : "";
      return spec.min === undefined ? `${spec.label} must be a number` : `${spec.label} must be a number of ${spec.min}${unit} or more`;
    }
    case "bool":
      return typeof value === "boolean" ? null : `${spec.label} must be Yes or No`;
    case "choice":
      return typeof value === "string" && (spec.choices ?? []).some((c) => c.id === value) ? null : `${spec.label} is not one of its choices`;
  }
}

/**
 * What stops the script from running, as sentences; empty when it can run. Unknown actions count wherever they are;
 * Soon actions, actions that need a selection (scripts run without one) and bad parameter values count in enabled
 * steps only, since disabled steps are skipped. Missing parameters are fine: they take the catalog default.
 */
export function scriptProblems(script: RepairScript, catalog: Catalog): string[] {
  const problems: string[] = [];
  script.actions.forEach((step, i) => {
    const n = i + 1;
    const spec = actionSpec(catalog, step.type);
    if (!spec) {
      problems.push(`Step ${n} uses an action tricky3D does not know (${String(step.type)}).`);
      return;
    }
    if (!step.enabled) return;
    if (!spec.implemented) problems.push(`Step ${n}, ${spec.label}, is not available yet.`);
    else if (spec.needsSelection) problems.push(`Step ${n}, ${spec.label}, needs selected triangles, which a script does not have.`);
    for (const p of spec.params) {
      if (!(p.id in step.params)) continue;
      const problem = paramProblem(p, step.params[p.id]);
      if (problem) problems.push(`Step ${n}, ${spec.label}: ${problem}.`);
    }
  });
  return problems;
}

/** The enabled steps of a script: what a run executes. */
export function enabledSteps(script: RepairScript): ScriptStep[] {
  return script.actions.filter((s) => s.enabled);
}

/**
 * The config file's content made usable: own scripts only (a built-in or repeated id is dropped, builtIn is false,
 * missing fields get defaults), steps normalised against the catalog, and the order without repeated ids.
 */
export function cleanStored(data: StoredScripts | null, catalog: Catalog): StoredScripts {
  const seen = new Set(catalog.scripts.map((s) => s.id));
  const scripts: RepairScript[] = [];
  for (const s of Array.isArray(data?.scripts) ? data.scripts : []) {
    if (!s || typeof s.id !== "string" || seen.has(s.id) || s.id.startsWith(BUILTIN_PREFIX)) continue;
    seen.add(s.id);
    const actions = (Array.isArray(s.actions) ? s.actions : []).map((a) => ({
      type: a.type,
      enabled: a.enabled !== false,
      params: a.params && typeof a.params === "object" ? a.params : {},
    }));
    scripts.push(normalizeScript({ id: s.id, name: typeof s.name === "string" && s.name.trim() ? s.name : NEW_SCRIPT_NAME, builtIn: false, actions }, catalog));
  }
  const order = [...new Set((Array.isArray(data?.order) ? data.order : []).filter((id): id is string => typeof id === "string"))];
  return { scripts, order };
}

// ---------------------------------------------------------------------------------------------
// Loading and saving (the repair store, through src/repair/ipc.ts)

let loading: Promise<boolean> | null = null;
/** False when the scripts file exists but could not be read: saving then would overwrite the user's scripts. */
let writable = true;
let saving: Promise<unknown> = Promise.resolve();

const repair = () => useRepair.getState();

/** All scripts in picker order, from the repair store. */
export function allScripts(): RepairScript[] {
  const s = repair();
  return orderedScripts(s.catalog, s.ownScripts, s.order);
}

/** The script with this id among all scripts, or undefined. */
export function scriptById(id: string | null): RepairScript | undefined {
  return id === null ? undefined : allScripts().find((s) => s.id === id);
}

/**
 * Loads the catalog and the user's scripts into the repair store, once; calls during the load share it. Resolves
 * false after a toast when the catalog could not be loaded, so a later call tries again. Scripts that cannot be read
 * leave the user's file alone: changes are then not saved, and saving says so.
 */
export function loadScripts(): Promise<boolean> {
  if (repair().catalog) return Promise.resolve(true);
  if (!isTauri()) return Promise.resolve(false);
  loading ??= (async () => {
    try {
      const catalog = await fetchCatalog();
      let stored: StoredScripts | null = null;
      try {
        stored = await loadStoredScripts();
        writable = true;
      } catch (err) {
        writable = false;
        toast(errorMessage(err, "Could not read your repair scripts"), "error");
      }
      const clean = cleanStored(stored, catalog);
      useRepair.setState({ catalog, ownScripts: clean.scripts, order: clean.order });
      return true;
    } catch (err) {
      toast(errorMessage(err, "Could not load the repair actions"), "error");
      return false;
    } finally {
      loading = null;
    }
  })();
  return loading;
}

/**
 * Writes the user's scripts and the full picker order. Writes run one after another, each with the store as it is
 * when its turn comes, so the file always ends up with the latest state. Resolves false after a toast on failure.
 */
function persist(): Promise<boolean> {
  const turn = saving.then(async () => {
    if (!writable) {
      toast("Your repair scripts file could not be read, so changes to scripts are not saved.", "warn");
      return false;
    }
    try {
      await saveStoredScripts({ scripts: repair().ownScripts, order: allScripts().map((s) => s.id) });
      return true;
    } catch (err) {
      toast(errorMessage(err, "Could not save the repair scripts"), "error");
      return false;
    }
  });
  saving = turn.catch(() => undefined);
  return turn;
}

/** Puts the user's scripts and a picker order into the store; the stored order always lists every script. */
function commitScripts(ownScripts: RepairScript[], order: string[]) {
  const catalog = repair().catalog;
  useRepair.setState({ ownScripts, order: orderedScripts(catalog, ownScripts, order).map((s) => s.id) });
}

/**
 * Save: adds the user's script, or replaces the saved one with the same id, normalised against the catalog, and
 * writes the file. A built-in script is never saved over (false); "Save as copy" makes an own script of it.
 */
export async function saveScript(script: RepairScript): Promise<boolean> {
  if (script.builtIn || script.id.startsWith(BUILTIN_PREFIX) || !(await loadScripts())) return false;
  const { catalog, ownScripts, order } = repair();
  const clean = catalog ? normalizeScript({ ...script, builtIn: false }, catalog) : script;
  const exists = ownScripts.some((s) => s.id === clean.id);
  commitScripts(exists ? ownScripts.map((s) => (s.id === clean.id ? clean : s)) : [...ownScripts, clean], exists ? order : [...allScripts().map((s) => s.id), clean.id]);
  return persist();
}

/** The + button: a new empty script, selected in the Repair scripts tab and saved. Null when the scripts are not loaded. */
export async function createOwnScript(name: string): Promise<RepairScript | null> {
  if (!(await loadScripts())) return null;
  const script = createScript(name, allScripts());
  commitScripts([...repair().ownScripts, script], [...allScripts().map((s) => s.id), script.id]);
  useRepair.setState({ editingScriptId: script.id });
  await persist();
  return script;
}

/** The gear button: renames one of the user's scripts and saves. False for a built-in or unknown script. */
export async function renameOwnScript(id: string, name: string): Promise<boolean> {
  if (!(await loadScripts())) return false;
  const { ownScripts, order } = repair();
  const script = ownScripts.find((s) => s.id === id);
  if (!script) return false;
  const renamed = renameScript(script, name);
  if (renamed === script) return true;
  commitScripts(
    ownScripts.map((s) => (s.id === id ? renamed : s)),
    order,
  );
  return persist();
}

/** The ✕ button: deletes one of the user's scripts and saves; the tab then shows the script before it. Built-ins stay. */
export async function deleteOwnScript(id: string): Promise<boolean> {
  if (!(await loadScripts())) return false;
  const { ownScripts, order, editingScriptId } = repair();
  if (!ownScripts.some((s) => s.id === id)) return false;
  const before = allScripts();
  const at = before.findIndex((s) => s.id === id);
  commitScripts(
    ownScripts.filter((s) => s.id !== id),
    order.filter((x) => x !== id),
  );
  if (editingScriptId === id) {
    const rest = before.filter((s) => s.id !== id);
    useRepair.setState({ editingScriptId: rest[Math.max(0, at - 1)]?.id ?? null });
  }
  return persist();
}

/** "Save as copy": `script` (built-in or own, saved or edited) as a new own script right after the original. */
export async function saveScriptAsCopy(script: RepairScript, name?: string): Promise<RepairScript | null> {
  if (!(await loadScripts())) return null;
  const copy = copyScript(script, allScripts(), name);
  const ids = allScripts().map((s) => s.id);
  const at = ids.indexOf(script.id);
  ids.splice(at < 0 ? ids.length : at + 1, 0, copy.id);
  const { catalog } = repair();
  commitScripts([...repair().ownScripts, catalog ? normalizeScript(copy, catalog) : copy], ids);
  useRepair.setState({ editingScriptId: copy.id });
  await persist();
  return copy;
}

/** The picker's arrows: moves a script one place up (-1) or down (+1) and saves the order. */
export async function moveScript(id: string, delta: -1 | 1): Promise<void> {
  if (!(await loadScripts())) return;
  const scripts = allScripts();
  const order = moveInOrder(scripts, id, delta);
  if (order.every((x, i) => x === scripts[i].id)) return;
  commitScripts(repair().ownScripts, order);
  await persist();
}

/**
 * Import script…: reads a .json or .repair file picked in the open dialog and adds it as an own script (selected in
 * the tab), with a fresh id when its id is taken and its name made unique among all scripts, as New and Save as copy
 * do (Extended repair exported from another tool would otherwise sit next to the built-in one under the same name).
 * Warnings about skipped actions are shown. Null when cancelled.
 */
export async function importScript(): Promise<RepairScript | null> {
  if (!(await loadScripts())) return null;
  let imported: { script: RepairScript; warnings: string[] } | null;
  try {
    imported = await importScriptFile();
  } catch (err) {
    toast(errorMessage(err, "Could not import the script"), "error");
    return null;
  }
  if (!imported) return null;
  const existing = allScripts();
  const { id } = imported.script;
  const usable = typeof id === "string" && id.startsWith(OWN_PREFIX) && !existing.some((s) => s.id === id);
  const { catalog } = repair();
  const base: RepairScript = {
    ...imported.script,
    id: usable ? id : newScriptId(existing.map((s) => s.id)),
    name: uniqueName(imported.script.name?.trim() || NEW_SCRIPT_NAME, existing.map((s) => s.name)),
    builtIn: false,
  };
  const script = catalog ? normalizeScript(base, catalog) : base;
  commitScripts([...repair().ownScripts, script], [...existing.map((s) => s.id), script.id]);
  useRepair.setState({ editingScriptId: script.id });
  await persist();
  for (const w of imported.warnings) toast(w, "warn");
  toast(`Imported "${script.name}".`);
  return script;
}

/** The export button: writes the script as a .json file chosen in the save dialog. */
export async function exportScript(script: RepairScript): Promise<void> {
  try {
    if (await exportScriptFile(script)) toast(`Exported "${script.name}".`);
  } catch (err) {
    toast(errorMessage(err, `Could not export "${script.name}"`), "error");
  }
}
