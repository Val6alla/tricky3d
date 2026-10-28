// Part library › the right panel: the new part's name and the picked primitive's parameters, each checked as it is
// typed (src/library/session.ts parseValues), with Create part and Cancel at the bottom.
import { useId } from "react";
import { useLibrary } from "./store";
import { cancelCreate, closePartLibrary, createPart, fieldProblem, resetValues, setName, setValue, valueText } from "./session";
import "./library.css";

export function LibraryPanel() {
  const session = useLibrary((s) => s.session);
  const catalog = useLibrary((s) => s.catalog);
  const busy = useLibrary((s) => s.busy);
  const nameId = useId();
  const primitive = catalog?.find((p) => p.id === session?.primitive);
  if (!session || !primitive) return null;

  return (
    <section className="lib-panel" aria-label="Part configuration">
      <h2 className="rpanel-heading lib-panel-heading">Part configuration</h2>
      <div className="lib-form">
        <label className="lib-field" htmlFor={nameId}>
          <span className="lib-label">Part name</span>
          <input
            id={nameId}
            className="dlg-input"
            type="text"
            spellCheck={false}
            value={session.name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => e.stopPropagation()}
          />
        </label>
        <h3 className="lib-subheading">{primitive.label}</h3>
        {primitive.params.map((p) => {
          const text = session.values[p.key] ?? valueText(p.default);
          const problem = fieldProblem(p, text);
          return (
            <label key={p.key} className="lib-field">
              <span className="lib-label">
                {p.label}
                {p.unit && <span className="dlg-unit"> {p.unit}</span>}
              </span>
              <input
                className="dlg-input"
                type="text"
                inputMode={p.integer ? "numeric" : "decimal"}
                spellCheck={false}
                autoComplete="off"
                value={text}
                aria-invalid={problem ? true : undefined}
                title={problem ?? `${valueText(p.min)} to ${valueText(p.max)}${p.unit ? ` ${p.unit}` : ""}`}
                onChange={(e) => setValue(p.key, e.target.value)}
                onKeyDown={(e) => {
                  // Typing must not reach the app shortcuts; Enter creates the part.
                  e.stopPropagation();
                  if (e.key === "Enter") void createPart();
                }}
              />
              {problem && <span className="lib-problem">{problem}</span>}
            </label>
          );
        })}
        <button type="button" className="dlg-btn lib-reset" onClick={resetValues}>
          Reset to defaults
        </button>
      </div>
      <div className="lib-actions">
        {busy ? (
          <>
            <span className="lib-busy" role="status">
              <span className="statusbar-spinner" aria-hidden="true" />
              {busy.cancelling ? "Cancelling…" : "Creating the part…"}
            </span>
            <button type="button" className="dlg-btn" onClick={cancelCreate} disabled={busy.cancelling}>
              Cancel
            </button>
          </>
        ) : (
          <>
            <button type="button" className="dlg-btn" onClick={closePartLibrary}>
              Cancel
            </button>
            <button type="button" className="dlg-btn dlg-btn-primary" onClick={() => void createPart()}>
              Create part
            </button>
          </>
        )}
      </div>
    </section>
  );
}
