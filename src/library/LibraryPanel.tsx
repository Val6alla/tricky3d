// Part library › the right panel: the new part's name and the picked item's parameters (numbers, choices, texts,
// toggles, a picture), each checked as it is typed (src/library/session.ts), a read-only Triangles count, and Create
// part and Cancel at the bottom.
import { useId } from "react";
import { useStore } from "../store";
import type { LibraryParam } from "./ipc";
import { useLibrary, type FieldValue } from "./store";
import {
  cancelCreate,
  choosePicture,
  clearPicture,
  closePartLibrary,
  createPart,
  defaultValue,
  resetValues,
  setName,
  setValue,
  shows,
  triangleText,
  valueProblem,
  valueText,
} from "./session";
import "./library.css";

/** Typing must not reach the app shortcuts; Enter in a field creates the part. */
function keys(e: React.KeyboardEvent) {
  e.stopPropagation();
  if (e.key === "Enter" && !(e.target instanceof HTMLTextAreaElement)) void createPart();
}

function Field({ param, value }: { param: LibraryParam; value: FieldValue }) {
  const id = useId();
  const picture = useLibrary((s) => s.picture);
  const problem = valueProblem(param, value);
  const help = param.help || undefined;
  if (param.kind === "toggle") {
    return (
      <label className="lib-check" title={help}>
        <input type="checkbox" checked={value === true} onChange={(e) => setValue(param.key, e.target.checked)} onKeyDown={keys} />
        <span>{param.label}</span>
      </label>
    );
  }
  if (param.kind === "image") {
    return (
      <div className="lib-field" title={help}>
        <span className="lib-label">{param.label}</span>
        <div className="lib-picture">
          <span className="lib-picture-name">{picture ? picture.name : "Built-in sample"}</span>
          <button type="button" className="dlg-btn" onClick={() => void choosePicture()}>
            Choose…
          </button>
          {picture && (
            <button type="button" className="dlg-btn" onClick={clearPicture}>
              Use sample
            </button>
          )}
        </div>
      </div>
    );
  }
  return (
    <label className="lib-field" htmlFor={id}>
      <span className="lib-label">
        {param.label}
        {param.kind === "number" && param.unit && <span className="dlg-unit"> {param.unit}</span>}
      </span>
      {param.kind === "choice" ? (
        <select id={id} className="dlg-input" value={String(value)} title={help} onChange={(e) => setValue(param.key, e.target.value)} onKeyDown={keys}>
          {param.options.map((o) => (
            <option key={o.value} value={o.value} disabled={o.soon}>
              {o.soon ? `${o.label} (Soon)` : o.label}
            </option>
          ))}
        </select>
      ) : (
        <input
          id={id}
          className="dlg-input"
          type="text"
          inputMode={param.kind === "number" ? (param.integer ? "numeric" : "decimal") : undefined}
          spellCheck={false}
          autoComplete="off"
          value={String(value)}
          maxLength={param.kind === "text" ? param.maxLength : undefined}
          aria-invalid={problem ? true : undefined}
          title={
            problem ??
            help ??
            (param.kind === "number" ? `${valueText(param.min)} to ${valueText(param.max)}${param.unit ? ` ${param.unit}` : ""}` : undefined)
          }
          onChange={(e) => setValue(param.key, e.target.value)}
          onKeyDown={keys}
        />
      )}
      {problem && <span className="lib-problem">{problem}</span>}
    </label>
  );
}

export function LibraryPanel() {
  const session = useLibrary((s) => s.session);
  const catalog = useLibrary((s) => s.catalog);
  const busy = useLibrary((s) => s.busy);
  const selection = useStore((s) => s.selection);
  const parts = useStore((s) => s.parts);
  const nameId = useId();
  const item = catalog?.find((p) => p.id === session?.primitive);
  if (!session || !item) return null;
  const from = item.input === "parts" ? parts.filter((p) => selection.includes(p.id) && p.visible) : [];

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
            onKeyDown={keys}
          />
        </label>
        <h3 className="lib-subheading">{item.label}</h3>
        <p className="lib-description">{item.description}</p>
        {item.input === "parts" && (
          <p className={`lib-from${from.length === 0 ? " is-missing" : ""}`} role="status">
            {from.length === 0
              ? "Select the part to make it from in the parts list."
              : `Made from ${from.length === 1 ? `“${from[0].name}”` : `${from.length} parts`}.`}
          </p>
        )}
        {item.params
          .filter((p) => shows(p, session.values, item))
          .map((p) => (
            <Field key={p.key} param={p} value={session.values[p.key] ?? defaultValue(p)} />
          ))}
        <div className="lib-field">
          <span className="lib-label">Triangles</span>
          <output className="lib-readout" aria-live="polite">
            {session.triangles === null ? "…" : triangleText(session.triangles)}
          </output>
        </div>
        <button type="button" className="dlg-btn lib-reset" onClick={resetValues}>
          Reset to defaults
        </button>
      </div>
      <div className="lib-actions">
        {busy ? (
          <>
            <span className="lib-busy" role="status">
              <span className="statusbar-spinner" aria-hidden="true" />
              {busy.cancelling ? "Cancelling…" : `Creating the part… ${Math.round(busy.fraction * 100)}%`}
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
