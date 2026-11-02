import { useId, useState } from "react";
import { useStore, type ObjectsChoice, type UnitChoice } from "../store";
import { Dialog } from "./Dialog";

export const UNIT_OPTIONS: { value: UnitChoice; label: string }[] = [
  { value: "detect", label: "Detect from file" },
  { value: "mm", label: "Millimetres" },
  { value: "cm", label: "Centimetres" },
  { value: "m", label: "Metres" },
  { value: "inch", label: "Inches" },
  { value: "foot", label: "Feet" },
];

const OBJECT_OPTIONS: { value: ObjectsChoice; label: string; help: string }[] = [
  { value: "ask", label: "Ask each time", help: "A question for each file with several objects." },
  { value: "separate", label: "One part per object", help: "Each object becomes a part of its own." },
  { value: "merge", label: "Merge into one part", help: "All of the file's objects become one part, each where it sits." },
];

/**
 * File › Import options: the unit imported files are read in and what a file with several objects becomes. Both are
 * remembered and apply to Import part, File › Open of a mesh file and files dropped on the window.
 */
export function ImportOptionsDialog({ onClose }: { onClose(): void }) {
  const saved = useStore((s) => s.importOptions);
  const [units, setUnits] = useState(saved.units);
  const [objects, setObjects] = useState(saved.objects);
  const unitId = useId();
  const unitHelpId = useId();
  const objectsTitle = useId();
  const objectsName = useId();

  return (
    <Dialog
      title="Import options"
      onClose={onClose}
      onOk={() => {
        useStore.getState().setImportOptions({ units, objects });
        onClose();
      }}
    >
      <section className="dlg-section">
        <h3 className="dlg-section-title">Units</h3>
        <label className="dlg-field" htmlFor={unitId}>
          <span className="dlg-label">Read the numbers in a file as</span>
          <select
            id={unitId}
            className="dlg-input dlg-select"
            value={units}
            aria-describedby={unitHelpId}
            onChange={(e) => setUnits(UNIT_OPTIONS.find((o) => o.value === e.target.value)?.value ?? "detect")}
          >
            {UNIT_OPTIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
          <span id={unitHelpId} className="dlg-help">
            {units === "detect"
              ? "3MF, AMF and X3D files that name their unit are converted to millimetres. Other files are read as millimetres; a very small one asks whether it was made in metres."
              : "Every file is read in this unit, whatever it says, and converted to millimetres."}
          </span>
        </label>
      </section>
      <section className="dlg-section" aria-labelledby={objectsTitle}>
        <h3 id={objectsTitle} className="dlg-section-title">
          Files with several objects
        </h3>
        <div className="dlg-choices" role="radiogroup" aria-labelledby={objectsTitle}>
          {OBJECT_OPTIONS.map((o) => (
            <label key={o.value} className="dlg-check">
              <input type="radio" name={objectsName} checked={objects === o.value} onChange={() => setObjects(o.value)} />
              <span>
                <span className="dlg-check-label">{o.label}</span>
                <span className="dlg-help">{o.help}</span>
              </span>
            </label>
          ))}
        </div>
      </section>
    </Dialog>
  );
}
