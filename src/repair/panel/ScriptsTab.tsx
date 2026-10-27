// Repair panel › Repair scripts: pick a script, edit its steps (switch each on or off, set parameters, reorder by
// dragging or Alt+Up/Down, add, remove with Del), save it or run it, import and export scripts, and read what the
// last run did. Built-in scripts can be edited as well; "Save as copy" turns the edits into an own script. Unsaved
// edits are kept per script for as long as the app runs, so switching scripts or tabs loses nothing.
import { useId, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type PointerEvent } from "react";
import { create } from "zustand";
import { isTauri } from "@tauri-apps/api/core";
import { message } from "@tauri-apps/plugin-dialog";
import type { LucideIcon } from "lucide-react";
import {
  ArrowDownUp, ChevronDown, ChevronRight, ChevronsDownUp, CircleDashed, CircleDot, CopyX, Dot, Eraser, FileOutput,
  GripVertical, Grid3x3, Import, IterationCcw, LayoutGrid, Link, Minimize2, Package, Play, Plus, Save, Scale3d,
  Settings, Shrink, SquaresIntersect, TextCursorInput, TriangleDashed, Undo2, Unlink2, WavesHorizontal, Wrench, X,
} from "lucide-react";
import { IS_MAC, type Feature } from "../../features";
import { errorMessage } from "../../io/files";
import type { MenuEntry } from "../../menus/entries";
import { Menu } from "../../menus/Menu";
import { toast, useUi } from "../../ui";
import { actionCountText, formatDuration, stepParamsText } from "../format";
import {
  actionSpec,
  addStep,
  canAddAction,
  deleteOwnScript,
  enabledSteps,
  exportScript,
  importScript,
  moveStep,
  orderedScripts,
  removeStep,
  sameScript,
  saveScript,
  saveScriptAsCopy,
  scriptActions,
  scriptProblems,
  setStepParam,
  toggleStep,
} from "../scripts";
import { runScript } from "../session";
import { useRepair } from "../store";
import type { ActionId, ActionReport, ActionSpec, Catalog, ParamSpec, ParamValue, RepairScript, ScriptStep } from "../types";
import { NumberField, cx } from "./controls";

// ---------------------------------------------------------------------------------------------
// Unsaved edits (pure helpers, then the module store)

/** Unsaved changes to one script. */
export interface ScriptEdit {
  /** The steps as edited; null while they are the saved ones. */
  actions: ScriptStep[] | null;
  /** One key per step (of `actions`, or of the saved steps when null) that stays with its step as it moves. */
  keys: number[];
}

/** The script as the tab shows it: the saved one with its unsaved steps, when it has any. */
export function editedScript(saved: RepairScript, edit: ScriptEdit | undefined): RepairScript {
  return edit?.actions ? { ...saved, actions: edit.actions } : saved;
}

/** A key per shown step: the edit's while they still fit the steps, otherwise each step's index. */
export function stepKeys(shown: RepairScript, edit: ScriptEdit | undefined): number[] {
  return edit && edit.keys.length === shown.actions.length ? edit.keys : shown.actions.map((_, i) => i);
}

/**
 * The keys for `after`, made from `before` (keyed by `keys`) by one of the step functions of src/repair/scripts.ts:
 * a step that kept its object keeps its key (moved, or others removed), a changed step in a list of the same length
 * keeps the key of its place (switched, parameter set), and any other step gets a fresh key (added).
 */
export function carryKeys(before: readonly ScriptStep[], keys: readonly number[], after: readonly ScriptStep[], fresh: () => number): number[] {
  const byStep = new Map<ScriptStep, number>();
  before.forEach((step, i) => byStep.set(step, keys[i]));
  const used = new Set<number>();
  return after.map((step, i) => {
    let key = byStep.get(step);
    if (key === undefined && after.length === before.length) key = keys[i];
    while (key === undefined || used.has(key)) key = fresh();
    used.add(key);
    return key;
  });
}

/** The edit once `next` replaced `shown` (keyed by `keys`), an edit of `saved`; no unsaved steps when `next` is `saved` again. */
export function nextEdit(saved: RepairScript, shown: RepairScript, keys: readonly number[], next: RepairScript, fresh: () => number): ScriptEdit {
  return { actions: sameScript(next, saved) ? null : next.actions, keys: carryKeys(shown.actions, keys, next.actions, fresh) };
}

/** Where a dragged row would land: before the first row whose middle is below `y`, or after the last. */
export function insertionIndex(middles: readonly number[], y: number): number {
  const i = middles.findIndex((m) => y < m);
  return i < 0 ? middles.length : i;
}

/** moveStep's target for the row at `from` dropped before row `insertion` (0..count, count = after the last). */
export function moveTarget(from: number, insertion: number): number {
  return insertion > from ? insertion - 1 : insertion;
}

/** The dropdown's text for a script: its name, marked when it has unsaved edits. */
export function optionLabel(script: RepairScript, edited: boolean): string {
  return edited ? `${script.name} (edited)` : script.name;
}

/** Built-in scripts are never saved over: their Save button makes an own copy. */
export function saveLabel(script: RepairScript): string {
  return script.builtIn ? "Save as copy" : "Save";
}

/** The line under the script picker. */
export function scriptInfo(script: RepairScript, edited: boolean): string {
  const count = actionCountText(script.actions.length);
  if (script.builtIn) return edited ? `Built-in, ${count}. Save your changes as a copy to keep them.` : `Built-in, ${count}.`;
  return edited ? `${count}, changes not saved.` : `${count}.`;
}

/** Why Execute cannot run the script now, or null when it can. */
export function executeBlocked(script: RepairScript, catalog: Catalog, busy: boolean): string | null {
  if (busy) return "Wait for the current repair to finish.";
  const problems = scriptProblems(script, catalog);
  if (problems.length > 0) return problems[0];
  if (script.actions.length === 0) return "Add an action first.";
  if (enabledSteps(script).length === 0) return "Switch on at least one action.";
  return null;
}

/** A float parameter as its field shows it: the spec's decimals, no thousands separators ("0.010"). */
export function paramText(spec: ParamSpec, value: number): string {
  return spec.decimals === undefined ? String(value) : value.toFixed(spec.decimals);
}

/** The whole run's time, the sum of the actions' times. */
export function runTime(reports: readonly ActionReport[]): string {
  return formatDuration(reports.reduce((sum, r) => sum + r.millis, 0));
}

/** Unsaved edits by script id. */
const useEdits = create<{ edits: Record<string, ScriptEdit> }>()(() => ({ edits: {} }));

/** Keys for added steps; a saved script's steps are keyed by index, far below these. */
let keySeq = 1_000_000;
const freshKey = () => ++keySeq;

function setEdit(id: string, edit: ScriptEdit | null) {
  useEdits.setState((s) => {
    const edits = { ...s.edits };
    if (edit) edits[id] = edit;
    else delete edits[id];
    return { edits };
  });
}

const editOf = (id: string): ScriptEdit | undefined => useEdits.getState().edits[id];

/** The ✕ button's question. True for Delete (the button's label, or a platform's plain Ok). */
async function askDelete(name: string): Promise<boolean> {
  const question = `Delete the repair script "${name}"? This cannot be undone.`;
  // Plain browser (development): no native dialog.
  if (!isTauri()) return window.confirm(question);
  try {
    const answer = await message(question, { title: "Delete script", kind: "warning", buttons: { ok: "Delete", cancel: "Cancel" } });
    return answer === "Delete" || answer === "Ok";
  } catch (err) {
    toast(errorMessage(err, "Could not ask about deleting the script"), "error");
    return false;
  }
}

// ---------------------------------------------------------------------------------------------
// The tab

export function ScriptsTab({ missing }: { missing: string | null }) {
  const catalog = useRepair((s) => s.catalog);
  const ownScripts = useRepair((s) => s.ownScripts);
  const order = useRepair((s) => s.order);
  const editingId = useRepair((s) => s.editingScriptId);
  const busy = useRepair((s) => s.busy !== null);
  const lastRun = useRepair((s) => s.session?.lastRun ?? null);
  const edits = useEdits((s) => s.edits);
  const scripts = useMemo(() => orderedScripts(catalog, ownScripts, order), [catalog, ownScripts, order]);
  const saved = scripts.find((s) => s.id === editingId) ?? scripts[0];

  if (!catalog || !saved) {
    return (
      <div className="rp-body">
        <p className="rpanel-muted">{missing ?? "There are no repair scripts."}</p>
      </div>
    );
  }

  const edit = edits[saved.id];
  const shown = editedScript(saved, edit);
  const keys = stepKeys(shown, edit);
  const edited = shown !== saved;
  const problems = scriptProblems(shown, catalog);
  const blocked = executeBlocked(shown, catalog, busy);

  function change(next: RepairScript) {
    if (next !== shown) setEdit(saved.id, nextEdit(saved, shown, keys, next, freshKey));
  }

  async function save() {
    const actions = shown.actions;
    if (saved.builtIn) {
      const copy = await saveScriptAsCopy(shown);
      if (!copy) return;
      // The copy now holds the edits, so the built-in shows its own steps again (unless it was edited meanwhile).
      setEdit(copy.id, { actions: null, keys });
      if (editOf(saved.id)?.actions === actions) setEdit(saved.id, null);
    } else if (await saveScript(shown)) {
      const now = editOf(saved.id);
      if (now?.actions === actions) setEdit(saved.id, { actions: null, keys: now.keys });
    }
  }

  async function remove() {
    if (saved.builtIn || !(await askDelete(saved.name))) return;
    if (await deleteOwnScript(saved.id)) setEdit(saved.id, null);
  }

  const openSettings = (scriptId: string | null) => useUi.getState().openDialog({ kind: "repair-script-settings", scriptId });

  return (
    <div className="rp-body">
      <section className="rp-group" aria-label="Script">
        <div className="rp-row rp-picker">
          <select
            className="clip-select"
            aria-label="Repair script"
            value={saved.id}
            onChange={(e) => useRepair.getState().setEditingScriptId(e.target.value)}
          >
            {scripts.map((s) => (
              <option key={s.id} value={s.id}>
                {optionLabel(s, edits[s.id]?.actions != null)}
              </option>
            ))}
          </select>
          <button type="button" className="clip-icon-btn" aria-label="New script" title="New script" onClick={() => openSettings(null)}>
            <Plus size={14} aria-hidden="true" />
          </button>
          <button
            type="button"
            className="clip-icon-btn"
            aria-label="Script settings"
            title={saved.builtIn ? "Built-in scripts cannot be renamed. Save a copy to make your own." : "Script settings: rename the script"}
            disabled={saved.builtIn}
            onClick={() => openSettings(saved.id)}
          >
            <Settings size={14} aria-hidden="true" />
          </button>
          <button
            type="button"
            className="clip-icon-btn"
            aria-label="Delete script"
            title={saved.builtIn ? "Built-in scripts cannot be deleted." : "Delete this script"}
            disabled={saved.builtIn}
            onClick={() => void remove()}
          >
            <X size={14} aria-hidden="true" />
          </button>
        </div>
        <div className="rp-row">
          <button type="button" className="clip-btn" title="Add a script from a .json file or a .repair file" onClick={() => void importScript()}>
            <Import size={13} aria-hidden="true" />
            Import script…
          </button>
          <button type="button" className="clip-btn" title="Save this script, as shown, to a .json file" onClick={() => void exportScript(shown)}>
            <FileOutput size={13} aria-hidden="true" />
            Export…
          </button>
        </div>
        <p className="rpanel-muted">{scriptInfo(shown, edited)}</p>
      </section>

      <section className="rp-group" aria-label="Actions of the script">
        <StepList key={saved.id} script={shown} keys={keys} catalog={catalog} onChange={change} />
        <AddAction catalog={catalog} onAdd={(id) => change(addStep(shown, id, catalog))} />
        <div className="rp-row">
          <button
            type="button"
            className="clip-btn"
            title={saved.builtIn ? "Save the script as shown as a new script of your own" : edited ? "Save the changes" : "No changes to save"}
            disabled={!saved.builtIn && !edited}
            onClick={() => void save()}
          >
            <Save size={13} aria-hidden="true" />
            {saveLabel(saved)}
          </button>
          {edited && (
            <button type="button" className="clip-btn" title="Drop the changes and show the saved script" onClick={() => setEdit(saved.id, null)}>
              <Undo2 size={13} aria-hidden="true" />
              Revert
            </button>
          )}
          <button
            type="button"
            className="clip-btn clip-btn-primary rp-push"
            title={blocked ?? "Run the script, as shown, on the working copy"}
            disabled={blocked !== null}
            onClick={() => void runScript(shown)}
          >
            <Play size={13} aria-hidden="true" />
            Execute
          </button>
        </div>
        {problems.length > 0 && (
          <ul className="rp-problems">
            {problems.slice(0, 3).map((p) => (
              <li key={p}>{p}</li>
            ))}
            {problems.length > 3 && <li>{`And ${problems.length - 3} more.`}</li>}
          </ul>
        )}
      </section>

      {lastRun && <LastRun reports={lastRun} />}
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// Steps

/** How far the pointer moves before a press on a row becomes a drag, and how near an edge the list scrolls. */
const DRAG_START_PX = 4;
const SCROLL_EDGE_PX = 56;
const SCROLL_STEP_PX = 14;

interface Drag {
  pointer: number;
  from: number;
  y: number;
  /** Insertion index while dragging; null until the pointer has moved far enough. */
  to: number | null;
}

/** focusAfter's mark for the empty list's note (row keys are never negative). */
const EMPTY_NOTE = -1;

/**
 * Keys on the empty list's note, which takes the focus when the last row is removed: Del (Backspace on macOS), pressed
 * again or still held from removing the rows, is marked handled, so the window's shortcut does not go on to remove the
 * selected triangles.
 */
export function onEmptyStepsKeyDown(e: Pick<KeyboardEvent, "key" | "preventDefault">, isMac: boolean = IS_MAC): void {
  if (e.key === "Delete" || (isMac && e.key === "Backspace")) e.preventDefault();
}

/** The nearest ancestor that scrolls vertically. */
function scrollParent(el: HTMLElement): HTMLElement | null {
  for (let p = el.parentElement; p; p = p.parentElement) {
    const { overflowY } = getComputedStyle(p);
    if ((overflowY === "auto" || overflowY === "scroll") && p.scrollHeight > p.clientHeight) return p;
  }
  return null;
}

interface StepListProps {
  script: RepairScript;
  keys: readonly number[];
  catalog: Catalog;
  onChange(next: RepairScript): void;
}

/**
 * The script's steps as a tree: each row has its switch, its number and name, and a ▸ that opens its parameters.
 * Rows move by dragging (pointer events: HTML5 drag events do not reach the page on Windows while the window takes
 * file drops) or with Alt+Up/Down; Del removes the focused row; arrows move the focus, Right and Left open and close
 * the parameters, Space switches the step. With no steps it shows a note instead, which takes the focus when the last
 * row is removed (Del or its ✕): dropped to the page, the focus would leave the next Del, or the one still held, to
 * the window's shortcut, which removes the selected triangles.
 */
export function StepList({ script, keys, catalog, onChange }: StepListProps) {
  const listRef = useRef<HTMLUListElement>(null);
  const emptyRef = useRef<HTMLParagraphElement>(null);
  const rows = useRef(new Map<number, HTMLLIElement>());
  const [open, setOpen] = useState<ReadonlySet<number>>(() => new Set());
  const [current, setCurrent] = useState<number | null>(null);
  const [dropAt, setDropAt] = useState<{ from: number; to: number } | null>(null);
  const drag = useRef<Drag | null>(null);
  /** The row to focus once the change that moved or removed rows has rendered, or EMPTY_NOTE for the note. */
  const focusAfter = useRef<number | null>(null);
  const n = script.actions.length;
  const tabKey = current !== null && keys.includes(current) ? current : keys[0];

  useLayoutEffect(() => {
    const key = focusAfter.current;
    if (key === null) return;
    focusAfter.current = null;
    (key === EMPTY_NOTE ? emptyRef.current : rows.current.get(key))?.focus();
  });

  function toggleOpen(key: number, to?: boolean) {
    setOpen((o) => {
      const next = new Set(o);
      if (to ?? !next.has(key)) next.add(key);
      else next.delete(key);
      return next;
    });
  }

  function move(i: number, to: number) {
    if (to < 0 || to >= n || to === i) return;
    focusAfter.current = keys[i];
    onChange(moveStep(script, i, to));
  }

  function remove(i: number) {
    focusAfter.current = keys[i + 1] ?? keys[i - 1] ?? EMPTY_NOTE;
    onChange(removeStep(script, i));
  }

  function onKeyDown(e: KeyboardEvent<HTMLLIElement>, i: number, hasParams: boolean) {
    // Keys typed in a parameter field are the field's.
    if (e.target !== e.currentTarget) return;
    const key = keys[i];
    if (e.altKey) {
      if (e.key === "ArrowUp") move(i, i - 1);
      else if (e.key === "ArrowDown") move(i, i + 1);
      else return;
    } else if (e.key === "ArrowUp" || e.key === "ArrowDown" || e.key === "Home" || e.key === "End") {
      const to = e.key === "Home" ? 0 : e.key === "End" ? n - 1 : i + (e.key === "ArrowUp" ? -1 : 1);
      if (to >= 0 && to < n) rows.current.get(keys[to])?.focus();
    } else if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
      if (hasParams) toggleOpen(key, e.key === "ArrowRight");
    } else if (e.key === "Enter") {
      if (hasParams) toggleOpen(key);
    } else if (e.key === " ") {
      onChange(toggleStep(script, i));
    } else if (e.key === "Delete" || (IS_MAC && e.key === "Backspace")) {
      remove(i);
    } else {
      return;
    }
    // Marked handled, so the window's shortcuts (Del removes selected triangles) leave it alone.
    e.preventDefault();
  }

  function onPointerDown(e: PointerEvent<HTMLDivElement>, i: number) {
    if (e.button !== 0 || (e.target as Element).closest("button, input, select")) return;
    drag.current = { pointer: e.pointerId, from: i, y: e.clientY, to: null };
    e.currentTarget.setPointerCapture(e.pointerId);
  }

  function onPointerMove(e: PointerEvent<HTMLDivElement>) {
    const d = drag.current;
    const list = listRef.current;
    if (!d || d.pointer !== e.pointerId || !list) return;
    if (d.to === null && Math.abs(e.clientY - d.y) < DRAG_START_PX) return;
    const box = scrollParent(list);
    if (box) {
      const r = box.getBoundingClientRect();
      if (e.clientY < r.top + SCROLL_EDGE_PX) box.scrollTop -= SCROLL_STEP_PX;
      else if (e.clientY > r.bottom - SCROLL_EDGE_PX) box.scrollTop += SCROLL_STEP_PX;
    }
    const middles = Array.from(list.children, (el) => {
      const r = el.getBoundingClientRect();
      return (r.top + r.bottom) / 2;
    });
    d.to = insertionIndex(middles, e.clientY);
    setDropAt({ from: d.from, to: d.to });
  }

  function onPointerUp(e: PointerEvent<HTMLDivElement>) {
    const d = drag.current;
    if (!d || d.pointer !== e.pointerId) return;
    drag.current = null;
    setDropAt(null);
    if (d.to !== null) move(d.from, moveTarget(d.from, d.to));
  }

  function cancelDrag() {
    drag.current = null;
    setDropAt(null);
  }

  // Where the drop line shows: before row `to`, or after the last row; nothing where the row would not move.
  const line = dropAt && moveTarget(dropAt.from, dropAt.to) !== dropAt.from ? dropAt.to : null;

  if (n === 0) {
    return (
      <p ref={emptyRef} className="rpanel-muted rp-empty" tabIndex={-1} onKeyDown={onEmptyStepsKeyDown}>
        No actions yet. Pick one below and click Add.
      </p>
    );
  }

  return (
    <ul ref={listRef} className={cx("rp-steps", dropAt && "is-dragging")} role="tree" aria-label="Repair actions">
      {script.actions.map((step, i) => {
        const key = keys[i];
        const spec = actionSpec(catalog, step.type);
        const label = spec?.label ?? String(step.type);
        const hasParams = (spec?.params.length ?? 0) > 0;
        const isOpen = hasParams && open.has(key);
        const summary = spec && hasParams && !isOpen ? stepParamsText(step, spec) : "";
        return (
          <li
            key={key}
            ref={(el) => {
              if (el) rows.current.set(key, el);
              else rows.current.delete(key);
            }}
            role="treeitem"
            aria-level={1}
            aria-selected={key === tabKey}
            aria-expanded={hasParams ? isOpen : undefined}
            aria-label={`${i + 1}. ${label}${step.enabled ? "" : ", switched off"}`}
            tabIndex={key === tabKey ? 0 : -1}
            className={cx(
              "rp-step",
              !step.enabled && "is-off",
              !spec && "is-unknown",
              dropAt?.from === i && "is-dragged",
              line === i && "is-drop-before",
              line === n && i === n - 1 && "is-drop-after",
            )}
            onFocus={(e) => {
              if (e.target === e.currentTarget) setCurrent(key);
            }}
            onKeyDown={(e) => onKeyDown(e, i, hasParams)}
          >
            <div
              className="rp-step-row"
              title="Drag to move; Alt+Up and Alt+Down move the focused action"
              onMouseDown={(e) => {
                // A click anywhere on the row, its switch and buttons included, makes it the focused row.
                e.preventDefault();
                rows.current.get(key)?.focus({ preventScroll: true });
              }}
              onPointerDown={(e) => onPointerDown(e, i)}
              onPointerMove={onPointerMove}
              onPointerUp={onPointerUp}
              onPointerCancel={cancelDrag}
              onLostPointerCapture={cancelDrag}
            >
              <GripVertical className="rp-grip" size={12} aria-hidden="true" />
              {hasParams ? (
                <button
                  type="button"
                  className="rp-expand"
                  tabIndex={-1}
                  aria-label={isOpen ? `Hide the parameters of ${label}` : `Show the parameters of ${label}`}
                  onClick={() => toggleOpen(key)}
                >
                  <ChevronRight size={13} className={cx("rpanel-chevron", isOpen && "is-open")} aria-hidden="true" />
                </button>
              ) : (
                <span className="rp-expand" aria-hidden="true" />
              )}
              <input
                type="checkbox"
                className="rp-step-check"
                tabIndex={-1}
                checked={step.enabled}
                aria-label={`Run ${label}`}
                title={step.enabled ? "Switched on: click to skip this action" : "Switched off: click to run this action"}
                onChange={() => onChange(toggleStep(script, i))}
              />
              <span className="rp-step-num" aria-hidden="true">
                {i + 1}
              </span>
              <span className="rp-step-text">
                <span className="rp-step-label">{label}</span>
                {spec && !spec.implemented && <span className="soon-pill">Soon</span>}
                {summary && <span className="rp-step-summary">{summary}</span>}
              </span>
              <button
                type="button"
                className="rp-step-remove"
                tabIndex={-1}
                aria-label={`Remove ${label}`}
                title={`Remove this action (${IS_MAC ? "Backspace" : "Del"})`}
                onClick={() => remove(i)}
              >
                <X size={12} aria-hidden="true" />
              </button>
            </div>
            {isOpen && spec && (
              <StepParams step={step} spec={spec} onParam={(id, value) => onChange(setStepParam(script, i, id, value, catalog))} />
            )}
          </li>
        );
      })}
    </ul>
  );
}

interface StepParamsProps {
  step: ScriptStep;
  spec: ActionSpec;
  onParam(id: string, value: ParamValue): void;
}

/** A step's parameters, edited in place: each with its label, the spec's decimals and its unit. */
function StepParams({ step, spec, onParam }: StepParamsProps) {
  return (
    <div className="rp-params" role="group" aria-label={`Parameters of ${spec.label}`}>
      {spec.params.map((p) => (
        <ParamEditor key={p.id} spec={p} value={step.params[p.id] ?? p.default} onChange={(v) => onParam(p.id, v)} />
      ))}
    </div>
  );
}

function ParamEditor({ spec, value, onChange }: { spec: ParamSpec; value: ParamValue; onChange(v: ParamValue): void }) {
  const id = useId();
  switch (spec.kind) {
    case "float": {
      const number = typeof value === "number" ? value : Number(spec.default);
      return (
        <div className="rp-param">
          <label className="rp-param-label" htmlFor={id}>
            {spec.label}
          </label>
          <NumberField id={id} value={number} format={(v) => paramText(spec, v)} unit={spec.unit} onCommit={onChange} />
        </div>
      );
    }
    case "bool":
      return (
        <label className="clip-check rp-param-check">
          <input type="checkbox" checked={value === true} onChange={(e) => onChange(e.target.checked)} />
          {spec.label}
        </label>
      );
    case "choice":
      return (
        <div className="rp-param rp-param-choice">
          <label className="rp-param-label" htmlFor={id}>
            {spec.label}
          </label>
          <select id={id} className="clip-select" value={String(value)} onChange={(e) => onChange(e.target.value)}>
            {(spec.choices ?? []).map((c) => (
              <option key={c.id} value={c.id}>
                {c.label}
              </option>
            ))}
          </select>
        </div>
      );
  }
}

// ---------------------------------------------------------------------------------------------
// Adding actions

/** Icons for the add-action dropdown, the same as the ribbon's for these actions. */
const ACTION_ICONS: Partial<Record<ActionId, LucideIcon>> = {
  close_all_holes: CircleDot,
  close_trivial_holes: CircleDashed,
  fix_flipped_triangles: ArrowDownUp,
  invert_negative_shells: IterationCcw,
  prefix_suffix: TextCursorInput,
  reduce_triangles: Minimize2,
  refine_triangles: Grid3x3,
  remesh: LayoutGrid,
  remove_degenerate_faces: TriangleDashed,
  remove_double_triangles: CopyX,
  remove_problem_areas: Eraser,
  remove_self_intersections: SquaresIntersect,
  remove_tiny_shells: Shrink,
  sanitize_duplicate_nodes: Dot,
  scale: Scale3d,
  smooth_triangles: WavesHorizontal,
  split_non_manifold_edges: Unlink2,
  split_off_self_intersections: SquaresIntersect,
  stitch_triangles: Link,
  wrap_part_surface: Package,
  z_compensation: ChevronsDownUp,
};

/**
 * One dropdown row: a ready action picks itself; a Soon one shows the Soon pill and, like every Soon entry, only says
 * it is coming (and hides with Settings › Hide unavailable features).
 */
function addEntry(spec: ActionSpec, chosen: ActionId | undefined, choose: (id: ActionId) => void): Feature {
  return {
    id: `repair-add.${spec.id}`,
    tab: "repair",
    group: "Actions",
    label: spec.label,
    icon: ACTION_ICONS[spec.id] ?? Wrench,
    status: spec.implemented ? "ready" : "soon",
    hint: spec.hint,
    run: spec.implemented ? () => choose(spec.id) : undefined,
    isActive: () => spec.id === chosen,
  };
}

/** The add-action dropdown and Add, which appends the chosen action at its default parameters. */
function AddAction({ catalog, onAdd }: { catalog: Catalog; onAdd(id: ActionId): void }) {
  const actions = useMemo(() => scriptActions(catalog), [catalog]);
  const [choice, setChoice] = useState<ActionId | null>(null);
  const [menuAt, setMenuAt] = useState<HTMLElement | null>(null);
  const chosen = actions.find((a) => a.id === choice && canAddAction(a)) ?? actions.find(canAddAction);
  const entries: MenuEntry[] = actions.map((a) => addEntry(a, chosen?.id, setChoice));

  return (
    <div className="rp-row rp-add">
      <button
        type="button"
        className="clip-select rp-add-pick"
        aria-label="Action to add"
        aria-haspopup="menu"
        aria-expanded={menuAt !== null}
        title={chosen?.hint}
        onClick={(e) => setMenuAt(menuAt ? null : e.currentTarget)}
      >
        <span className="rp-add-label">{chosen?.label ?? "No action to add"}</span>
        <ChevronDown size={12} aria-hidden="true" />
      </button>
      <button
        type="button"
        className="clip-btn"
        title={chosen ? `Add ${chosen.label} at the end of the script` : undefined}
        disabled={!chosen}
        onClick={() => chosen && onAdd(chosen.id)}
      >
        <Plus size={13} aria-hidden="true" />
        Add
      </button>
      {menuAt && <Menu entries={entries} anchor={{ el: menuAt }} label="Action to add" onClose={() => setMenuAt(null)} />}
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// Last run

/** What each action of the last script run did, and how long it took. */
export function LastRun({ reports }: { reports: ActionReport[] }) {
  const [open, setOpen] = useState(true);
  return (
    <section className="rp-group rp-lastrun" aria-label="Last run">
      <h3 className="rp-title">
        <button type="button" className="rp-fold" aria-expanded={open} onClick={() => setOpen(!open)}>
          <ChevronRight size={13} className={cx("rpanel-chevron", open && "is-open")} aria-hidden="true" />
          Last run
          <span className="rp-fold-time">{runTime(reports)}</span>
        </button>
      </h3>
      {open && (
        <ol className="rp-reports">
          {reports.map((r, i) => (
            <li key={i} className={cx("rp-report", !r.changed && "is-unchanged")}>
              <div className="rp-report-head">
                <span className="rp-report-label">{r.label}</span>
                <span className="rp-report-time">{formatDuration(r.millis)}</span>
              </div>
              {r.message && <div className="rp-report-msg">{r.message}</div>}
              {r.warnings.map((w) => (
                <div key={w} className="rp-report-warn">
                  {w}
                </div>
              ))}
              {r.resolved.map((w) => (
                <div key={w} className="rp-report-resolved">
                  {w}
                </div>
              ))}
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}
