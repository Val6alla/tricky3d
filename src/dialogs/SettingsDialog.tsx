import { useId } from "react";
import { useStore, type ImportPlacement } from "../store";
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
  const helpId = useId();
  const placementHelpId = useId();

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
            Placing rests each imported file on the platform, centred.
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
