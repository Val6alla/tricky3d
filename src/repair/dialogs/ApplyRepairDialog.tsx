// Repair mode › Apply repair: how the repaired working copy goes into the parts list. The three choices are the
// designed ones, Remove old part first and picked; src/repair/session.ts makes the choice one main undo step "Repair".
import { useEffect, useId, useMemo, useState } from "react";
import { TriangleAlert } from "lucide-react";
import { Dialog } from "../../dialogs/Dialog";
import { useStore } from "../../store";
import { badgeText, checkFromStatus } from "../badges";
import { applyRepair, repairedPartName, workingCopyChanged } from "../session";
import { useRepair } from "../store";
import type { ApplyMode, RepairStatus } from "../types";
import "./dialogs.css";

export interface ApplyChoice {
  mode: ApplyMode;
  label: string;
  help: string;
}

/** The dialog's choices in order, worded for the part named `partName`. */
export function applyChoices(partName: string): ApplyChoice[] {
  return [
    { mode: "remove", label: "Remove old part", help: "The repaired part takes the old part's place, name and colour." },
    { mode: "keep", label: "Keep old part", help: `The repaired part is added after it as “${partName} (repaired)”.` },
    { mode: "compare", label: "Keep old part and compare", help: "Both parts stay, with the old one made transparent." },
  ];
}

/** What the working copy still needs, from its last status ("Needs repair: 4 holes"); null when fine or not known. */
export function applyWarning(status: RepairStatus | null): string | null {
  return status ? badgeText(checkFromStatus(status)) : null;
}

/** Apply: closes the dialog, then applies the repair (which leaves repair mode, or says with a toast why not). */
export function confirmApply(mode: ApplyMode, onClose: () => void): void {
  onClose();
  applyRepair(mode);
}

export function ApplyRepairDialog({ onClose }: { onClose(): void }) {
  const session = useRepair((s) => s.session);
  // The part can be renamed in repair mode; Keep old part names the result after its name now, as applyRepair does.
  const parts = useStore((s) => s.parts);
  const changed = useMemo(() => (session ? workingCopyChanged(session) : false), [session?.mesh, session?.initialMesh]);

  // Repair mode can end while the dialog is open; then there is nothing to apply.
  useEffect(() => {
    if (!session) onClose();
  }, [session, onClose]);

  if (!session) return null;
  return (
    <ApplyRepairForm
      partName={repairedPartName(session, parts)}
      changed={changed}
      warning={applyWarning(session.status)}
      onClose={onClose}
    />
  );
}

export interface ApplyRepairFormProps {
  partName: string;
  /** False when the working copy is still the part as it was: applying then changes nothing. */
  changed: boolean;
  /** What the working copy still needs (applyWarning), or null. */
  warning: string | null;
  onClose(): void;
}

export function ApplyRepairForm({ partName, changed, warning, onClose }: ApplyRepairFormProps) {
  const [mode, setMode] = useState<ApplyMode>("remove");
  const group = useId();

  return (
    <Dialog title="Apply repair" onClose={onClose} onOk={() => confirmApply(mode, onClose)} okLabel="Apply">
      <div className="dlg-choices" role="radiogroup" aria-label="Apply repair">
        {applyChoices(partName).map((c) => (
          <label key={c.mode} className="dlg-check">
            <input type="radio" name={group} checked={mode === c.mode} onChange={() => setMode(c.mode)} />
            <span>
              <span className="dlg-check-label">{c.label}</span>
              <span className="dlg-help">{c.help}</span>
            </span>
          </label>
        ))}
      </div>
      {!changed && <p className="dlg-note">Nothing was changed, so the part stays as it was.</p>}
      {changed && warning && (
        <p className="rp-warning">
          <TriangleAlert size={13} aria-hidden="true" />
          {warning}
        </p>
      )}
    </Dialog>
  );
}
