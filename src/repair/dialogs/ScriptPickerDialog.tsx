// Repair mode › Run repair script (the panel button and the ribbon's Repair script): pick a script and execute it on
// the working copy. The list is in picker order, which the arrows change for good: src/repair/scripts.ts saves the
// order with the user's scripts. The search field filters by name; Enter executes the picked script.
import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { ArrowDown, ArrowUp } from "lucide-react";
import { Dialog } from "../../dialogs/Dialog";
import { actionCountText } from "../format";
import { enabledSteps, loadScripts, moveScript, orderedScripts, scriptProblems } from "../scripts";
import { runScript } from "../session";
import { useRepair } from "../store";
import type { Catalog, RepairScript } from "../types";
import "./dialogs.css";

/** Whether the catalog and the user's scripts are in the repair store yet. */
export type ScriptsState = "loading" | "ready" | "failed";

export interface ScriptList {
  /** Every script, built-in and own, in picker order. */
  scripts: RepairScript[];
  catalog: Catalog | null;
  state: ScriptsState;
}

/** The scripts in the repair store, loading the catalog and the user's scripts first when that has not happened. */
export function useScriptList(): ScriptList {
  const catalog = useRepair((s) => s.catalog);
  const ownScripts = useRepair((s) => s.ownScripts);
  const order = useRepair((s) => s.order);
  const scripts = useMemo(() => orderedScripts(catalog, ownScripts, order), [catalog, ownScripts, order]);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    if (catalog) return;
    let live = true;
    void loadScripts().then((ok) => {
      if (live && !ok) setFailed(true);
    });
    return () => {
      live = false;
    };
  }, [catalog]);
  return { scripts, catalog, state: catalog ? "ready" : failed ? "failed" : "loading" };
}

/** The scripts whose name contains `query`, ignoring case and the spaces around it; every script for a blank query. */
export function filterScripts(scripts: readonly RepairScript[], query: string): RepairScript[] {
  const q = query.trim().toLocaleLowerCase();
  return q ? scripts.filter((s) => s.name.toLocaleLowerCase().includes(q)) : [...scripts];
}

/** The script the picker acts on: the picked one while it is listed, else the first listed; null for an empty list. */
export function pickedIn(listed: readonly RepairScript[], pickedId: string | null): RepairScript | null {
  return listed.find((s) => s.id === pickedId) ?? listed[0] ?? null;
}

/** The id `delta` places from `currentId` in the list, clamped to its ends (Home and End pass ±Infinity). */
export function stepPick(listed: readonly RepairScript[], currentId: string | null, delta: number): string | null {
  if (listed.length === 0) return null;
  const at = Math.max(0, listed.findIndex((s) => s.id === currentId));
  return listed[Math.max(0, Math.min(listed.length - 1, at + delta))].id;
}

/** What a key does: moves the pick by `pick` places, or moves the picked script `move` places in the picker order. */
export type ListKey = { pick: number } | { move: -1 | 1 } | null;

/**
 * The script list's keys: Up and Down move the pick, Home and End jump to the ends, Alt+Up and Alt+Down move the
 * picked script in the order. In the search field (`inSearch`) Home and End stay with the text.
 */
export function listKey(e: { key: string; altKey: boolean }, inSearch: boolean): ListKey {
  if (e.key === "ArrowUp" || e.key === "ArrowDown") {
    const delta: -1 | 1 = e.key === "ArrowUp" ? -1 : 1;
    return e.altKey ? { move: delta } : { pick: delta };
  }
  if (inSearch || e.altKey) return null;
  if (e.key === "Home") return { pick: -Infinity };
  if (e.key === "End") return { pick: Infinity };
  return null;
}

/** Why the script cannot run, as one sentence for the note under the list; null when it can. */
export function runProblem(script: RepairScript, catalog: Catalog | null): string | null {
  const problem = catalog ? scriptProblems(script, catalog)[0] : undefined;
  if (problem) return problem;
  return enabledSteps(script).length === 0 ? "This script has no enabled actions." : null;
}

/** The script last executed from the picker; the picker opens with it picked. */
let lastExecuted: string | null = null;

/** The picker's first pick: the script last executed from it while that still exists, else the first in order. */
export function initialPick(scripts: readonly RepairScript[]): string | null {
  return scripts.some((s) => s.id === lastExecuted) ? lastExecuted : (scripts[0]?.id ?? null);
}

/** Execute: closes the picker first, so the progress dialog can open, then runs the script on the working copy. */
export function executeScript(script: RepairScript, onClose: () => void): void {
  lastExecuted = script.id;
  onClose();
  void runScript(script);
}

export function ScriptPickerDialog({ onClose }: { onClose(): void }) {
  const { scripts, catalog, state } = useScriptList();
  return <ScriptPickerForm scripts={scripts} catalog={catalog} state={state} onClose={onClose} />;
}

export interface ScriptPickerFormProps {
  /** Every script in picker order. */
  scripts: readonly RepairScript[];
  catalog: Catalog | null;
  state: ScriptsState;
  onClose(): void;
  /** The search text it opens with; empty in the app. */
  initialQuery?: string;
}

export function ScriptPickerForm({ scripts, catalog, state, onClose, initialQuery = "" }: ScriptPickerFormProps) {
  const [query, setQuery] = useState(initialQuery);
  const [pickedId, setPickedId] = useState<string | null>(null);
  const listId = useId();
  const listRef = useRef<HTMLUListElement>(null);
  const listed = useMemo(() => filterScripts(scripts, query), [scripts, query]);
  const current = pickedIn(listed, pickedId ?? initialPick(scripts));
  const problem = current ? runProblem(current, catalog) : null;
  // The arrows move a script one place in the whole order, which a filtered list would not show, so they wait.
  const searching = query.trim() !== "";
  const at = current ? scripts.findIndex((s) => s.id === current.id) : -1;
  const canUp = !searching && at > 0;
  const canDown = !searching && at >= 0 && at < scripts.length - 1;
  const optionId = (i: number) => `${listId}-${i}`;
  const pickedOption = current ? optionId(listed.indexOf(current)) : undefined;

  // Keeps the picked row in view as the keys move through a long list.
  useEffect(() => {
    listRef.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
  }, [current?.id]);

  function execute(script: RepairScript | null) {
    if (script && runProblem(script, catalog) === null) executeScript(script, onClose);
  }

  function move(delta: -1 | 1) {
    if (!current || !(delta < 0 ? canUp : canDown)) return;
    // Pin the pick first: a default pick of "the first script" would otherwise jump to the script moving up.
    setPickedId(current.id);
    void moveScript(current.id, delta);
  }

  function onKey(e: KeyboardEvent<HTMLElement>, inSearch: boolean) {
    const action = listKey(e, inSearch);
    if (!action) return;
    e.preventDefault();
    if ("move" in action) move(action.move);
    else {
      const id = stepPick(listed, current?.id ?? null, action.pick);
      if (id !== null) setPickedId(id);
    }
  }

  const moveTitle = (where: string, key: string) => (searching ? "Clear the search to change the order." : `Move the script ${where} (Alt+${key})`);

  return (
    <Dialog
      title="Run repair script"
      onClose={onClose}
      onOk={() => execute(current)}
      okLabel="Execute"
      okDisabled={current === null || problem !== null}
      className="dlg-rp-picker"
    >
      {/* A combobox over the list: Up and Down move the pick while the field keeps the focus, and a screen reader
          names the picked script only through the focused element's active descendant. */}
      <input
        type="search"
        className="dlg-input"
        placeholder="Search scripts"
        role="combobox"
        aria-label="Search scripts"
        aria-controls={listId}
        aria-expanded="true"
        aria-autocomplete="list"
        aria-activedescendant={pickedOption}
        spellCheck={false}
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape" && query !== "") {
            // The first Esc clears the search; the next one closes the dialog.
            e.preventDefault();
            e.stopPropagation();
            setQuery("");
          } else onKey(e, true);
        }}
      />
      <div className="rp-picker">
        <ul
          ref={listRef}
          id={listId}
          className="rp-scripts"
          role="listbox"
          aria-label="Repair scripts"
          aria-activedescendant={pickedOption}
          tabIndex={0}
          onKeyDown={(e) => onKey(e, false)}
        >
          {listed.map((s, i) => (
            <li
              key={s.id}
              id={optionId(i)}
              className="rp-script"
              role="option"
              aria-selected={s.id === current?.id}
              title={s.name}
              onClick={() => setPickedId(s.id)}
              onDoubleClick={() => execute(s)}
            >
              <span className="rp-script-name">{s.name}</span>
              {s.builtIn && <span className="rp-tag">Default</span>}
              <span className="rp-script-count">{actionCountText(s.actions.length)}</span>
            </li>
          ))}
        </ul>
        <div className="rp-move" role="group" aria-label="Picker order">
          <button type="button" className="rp-icon-btn" aria-label="Move script up" title={moveTitle("up", "↑")} disabled={!canUp} onClick={() => move(-1)}>
            <ArrowUp size={14} aria-hidden="true" />
          </button>
          <button type="button" className="rp-icon-btn" aria-label="Move script down" title={moveTitle("down", "↓")} disabled={!canDown} onClick={() => move(1)}>
            <ArrowDown size={14} aria-hidden="true" />
          </button>
        </div>
      </div>
      {state === "loading" && (
        <p className="dlg-note" role="status">
          Loading the repair scripts…
        </p>
      )}
      {state === "failed" && (
        <p className="dlg-error" role="alert">
          The repair scripts could not be loaded.
        </p>
      )}
      {state === "ready" && listed.length === 0 && (
        <p className="dlg-note" role="status">
          {searching ? `No script matches “${query.trim()}”.` : "There are no repair scripts."}
        </p>
      )}
      {problem && (
        <p className="dlg-error" role="alert">
          {problem}
        </p>
      )}
    </Dialog>
  );
}
