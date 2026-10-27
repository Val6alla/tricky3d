import { useId, useState } from "react";
import { parseNumber } from "../numbers";
import { useStore } from "../store";
import { Dialog } from "./Dialog";

const AXES = ["X", "Y", "Z"] as const;
/** Matches the inputs' max; a bigger platform would draw an unreasonably dense grid. */
const MAX_MM = 10000;

export function PlatformDialog({ onClose }: { onClose(): void }) {
  const [values, setValues] = useState(() => useStore.getState().platform.map(String));
  const [error, setError] = useState<string | null>(null);
  const errorId = useId();

  function setAxis(i: number, value: string) {
    setValues((v) => v.map((old, j) => (j === i ? value : old)));
    setError(null);
  }

  function ok() {
    const [x, y, z] = values.map((v) => parseNumber(v) ?? NaN);
    if (![x, y, z].every((v) => Number.isFinite(v) && v > 0 && v <= MAX_MM)) {
      setError(`Enter a size greater than 0 and up to ${MAX_MM} mm for X, Y and Z.`);
      return;
    }
    useStore.getState().setPlatform([x, y, z]);
    onClose();
  }

  return (
    <Dialog title="Platform size" onClose={onClose} onOk={ok}>
      <div className="dlg-grid-3">
        {AXES.map((axis, i) => (
          <label key={axis} className="dlg-field">
            <span className="dlg-label">
              {axis} <span className="dlg-unit">mm</span>
            </span>
            <input
              className="dlg-input"
              type="text"
              inputMode="decimal"
              spellCheck={false}
              autoComplete="off"
              value={values[i]}
              aria-invalid={error ? true : undefined}
              aria-describedby={error ? errorId : undefined}
              onChange={(e) => setAxis(i, e.target.value)}
            />
          </label>
        ))}
      </div>
      {error && (
        <p id={errorId} className="dlg-error" role="alert">
          {error}
        </p>
      )}
    </Dialog>
  );
}
