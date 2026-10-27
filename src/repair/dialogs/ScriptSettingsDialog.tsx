// Repair mode › script settings (the Repair scripts tab's gear and + buttons): rename one of the user's scripts, or
// name a new one. Built-in scripts keep their names; the tab offers Save as copy for them instead.
import { useEffect, useId, useState } from "react";
import { Dialog } from "../../dialogs/Dialog";
import { actionCountText } from "../format";
import { createOwnScript, NEW_SCRIPT_NAME, renameOwnScript, uniqueName } from "../scripts";
import type { RepairScript } from "../types";
import { useScriptList } from "./ScriptPickerDialog";
import "./dialogs.css";

/** Why `name` cannot be used for the script `selfId` (null for a new one): it is blank, or another script has it. */
export function nameProblem(name: string, scripts: readonly RepairScript[], selfId: string | null): string | null {
  const trimmed = name.trim();
  if (!trimmed) return "Enter a name.";
  return scripts.some((s) => s.id !== selfId && s.name.trim() === trimmed) ? `Another script is already called “${trimmed}”.` : null;
}

/** The name field's first value: the script's name, or for a new script "New script" made unique. */
export function initialName(script: RepairScript | null, scripts: readonly RepairScript[]): string {
  return script ? script.name : uniqueName(NEW_SCRIPT_NAME, scripts.map((s) => s.name));
}

/** OK: closes the dialog, then renames the script or creates it (src/repair/scripts.ts saves and reports failures). */
export function submitName(script: RepairScript | null, name: string, onClose: () => void): void {
  onClose();
  if (script) void renameOwnScript(script.id, name);
  else void createOwnScript(name);
}

const dialogTitle = (scriptId: string | null) => (scriptId === null ? "New repair script" : "Script settings");

export function ScriptSettingsDialog({ scriptId, onClose }: { scriptId: string | null; onClose(): void }) {
  const { scripts, state } = useScriptList();
  const script = scriptId === null ? null : (scripts.find((s) => s.id === scriptId) ?? null);
  const gone = state === "ready" && scriptId !== null && script === null;

  // The script can be deleted while the dialog is open; then there is nothing left to rename.
  useEffect(() => {
    if (gone) onClose();
  }, [gone, onClose]);

  if (gone) return null;
  if (state !== "ready") {
    return (
      <Dialog title={dialogTitle(scriptId)} onClose={onClose} className="dlg-rp-settings">
        {state === "loading" ? (
          <p className="dlg-note" role="status">
            Loading the repair scripts…
          </p>
        ) : (
          <p className="dlg-error" role="alert">
            The repair scripts could not be loaded.
          </p>
        )}
      </Dialog>
    );
  }
  return <ScriptSettingsForm script={script} scripts={scripts} onClose={onClose} />;
}

export interface ScriptSettingsFormProps {
  /** The script to rename, or null to name a new one. */
  script: RepairScript | null;
  /** Every script, for the name check. */
  scripts: readonly RepairScript[];
  onClose(): void;
}

export function ScriptSettingsForm({ script, scripts, onClose }: ScriptSettingsFormProps) {
  const [name, setName] = useState(() => initialName(script, scripts));
  const errorId = useId();
  const problem = nameProblem(name, scripts, script?.id ?? null);
  const title = dialogTitle(script?.id ?? null);

  if (script?.builtIn) {
    return (
      <Dialog title={title} onClose={onClose} className="dlg-rp-settings">
        <label className="dlg-field">
          <span className="dlg-label">Name</span>
          <input className="dlg-input" type="text" value={script.name} readOnly />
        </label>
        <p className="dlg-note">Built-in scripts keep their names. Use Save as copy to make a version of your own.</p>
      </Dialog>
    );
  }

  return (
    <Dialog
      title={title}
      onClose={onClose}
      onOk={() => {
        if (problem === null) submitName(script, name, onClose);
      }}
      okLabel={script ? "Rename" : "Create"}
      okDisabled={problem !== null}
      className="dlg-rp-settings"
    >
      <label className="dlg-field">
        <span className="dlg-label">Name</span>
        <input
          className="dlg-input"
          type="text"
          spellCheck={false}
          value={name}
          aria-invalid={problem !== null}
          aria-describedby={problem ? errorId : undefined}
          onChange={(e) => setName(e.target.value)}
        />
      </label>
      {script && <p className="dlg-note">{actionCountText(script.actions.length)}</p>}
      {problem && (
        <p id={errorId} className="dlg-error" role="alert">
          {problem}
        </p>
      )}
    </Dialog>
  );
}
