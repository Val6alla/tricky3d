import { useId } from "react";
import { AUTOSAVE_CHOICES, useStore, type AutosaveMinutes, type ImportPlacement } from "../store";
import { Dialog } from "./Dialog";

const PLACEMENT_OPTIONS: { value: ImportPlacement; label: string }[] = [
  { value: "ask", label: "Ask each time" },
  { value: "always", label: "Always" },
  { value: "never", label: "Never" },
];

export function SettingsDialog({ onClose }: { onClose(): void }) {
  const hideSoon = useStore((s) => s.hideSoon);
  const setHideSoon = useStore((s) => s.setHideSoon);
  const importPlacement = useStore((s) => s.importPlacement);
  const setImportPlacement = useStore((s) => s.setImportPlacement);
  const autosave = useStore((s) => s.autosaveMinutes);
  const setAutosave = useStore((s) => s.setAutosaveMinutes);
  const helpId = useId();
  const placementHelpId = useId();
  const autosaveHelpId = useId();

  return (
    <Dialog title="Settings" onClose={onClose}>
      <section className="dlg-section">
        <h3 className="dlg-section-title">Interface</h3>
        <label className="dlg-check">
          <input
            type="checkbox"
            checked={hideSoon}
            aria-describedby={helpId}
            onChange={(e) => setHideSoon(e.target.checked)}
          />
          <span>
            <span className="dlg-check-label">Hide unavailable features</span>
            <span id={helpId} className="dlg-help">
              Hides buttons marked Soon.
            </span>
          </span>
        </label>
      </section>
      <section className="dlg-section">
        <h3 className="dlg-section-title">Importing</h3>
        <label className="dlg-field">
          <span className="dlg-label">Place imported parts on the platform</span>
          <select
            className="dlg-input dlg-select"
            value={importPlacement}
            aria-describedby={placementHelpId}
            onChange={(e) => setImportPlacement(PLACEMENT_OPTIONS.find((o) => o.value === e.target.value)?.value ?? "ask")}
          >
            {PLACEMENT_OPTIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
          <span id={placementHelpId} className="dlg-help">
            Placing rests each imported file on the platform, centred. File › Import options sets the unit and what a
            file with several objects becomes.
          </span>
        </label>
      </section>
      <section className="dlg-section">
        <h3 className="dlg-section-title">Saving</h3>
        <label className="dlg-field">
          <span className="dlg-label">Save the project automatically</span>
          <select
            className="dlg-input dlg-select"
            value={autosave}
            aria-describedby={autosaveHelpId}
            onChange={(e) => setAutosave(AUTOSAVE_CHOICES.find((m) => m === Number(e.target.value)) ?? 5)}
          >
            {AUTOSAVE_CHOICES.map((m: AutosaveMinutes) => (
              <option key={m} value={m}>
                {m === 0 ? "Off" : `Every ${m} minutes`}
              </option>
            ))}
          </select>
          <span id={autosaveHelpId} className="dlg-help">
            Into tricky3D's own folder, never over your file; the last three are kept. After a crash, tricky3D offers
            the latest one when it starts.
          </span>
        </label>
      </section>
      <p className="dlg-muted">
        <span className="soon-pill">Soon</span>
        More settings are coming in a future version.
      </p>
    </Dialog>
  );
}
