// Home › Duplicate and Arrange › Copies: one dialog per way of placing the copies, laid out like the reference's: in
// place (how many), filling the platform (distances, a limit, in 3D or 2D), or in a grid (distance type, a row per axis
// with its direction, count and gap, then the quantity and size it comes to). OK runs it (src/arrange/session.ts).
import { useId, useMemo, useState } from "react";
import { Dialog } from "../dialogs/Dialog";
import "../dialogs/transform.css";
import { useStore } from "../store";
import { boxSize, formatField, groupBox, transformTargets } from "../transformTools";
import "./arrange.css";
import { DUPLICATE_LABELS, duplicateParts } from "./session";
import { checkDuplicate, gridCounts, gridExtent, loadDuplicate, saveDuplicate, type DuplicateFields, type GridAxisFields } from "./settings";
import type { DuplicateMode } from "./types";

const AXES = ["X", "Y", "Z"] as const;

export function DuplicateDialog({ mode, onClose }: { mode: DuplicateMode; onClose(): void }) {
  const [fields, setFields] = useState<DuplicateFields>(loadDuplicate);
  const check = checkDuplicate(mode, fields);

  function run() {
    if (!check.how) return;
    saveDuplicate(fields);
    onClose();
    void duplicateParts(check.how);
  }

  return (
    <Dialog title={DUPLICATE_LABELS[mode]} onClose={onClose} onOk={run} okDisabled={!check.how} className={mode === "grid" ? "dlg-arrange dlg-arrange-wide" : "dlg-arrange"}>
      <DuplicateForm mode={mode} fields={fields} setFields={setFields} />
      {check.problem && (
        <p className="dlg-error" role="alert">
          {check.problem}
        </p>
      )}
    </Dialog>
  );
}

export interface DuplicateFormProps {
  mode: DuplicateMode;
  fields: DuplicateFields;
  setFields(update: (f: DuplicateFields) => DuplicateFields): void;
}

/** The fields of one mode, without the dialog frame. */
export function DuplicateForm({ mode, fields, setFields }: DuplicateFormProps) {
  const edit = (patch: Partial<DuplicateFields>) => setFields((f) => ({ ...f, ...patch }));
  switch (mode) {
    case "in-place":
      return (
        <section className="dlg-section" aria-label="Duplicate parts">
          <h3 className="dlg-section-title">Duplicate parts</h3>
          <Field label="Copies" value={fields.copies} onChange={(copies) => edit({ copies })} />
          <p className="dlg-note">The copies go right on top of the selected parts.</p>
        </section>
      );
    case "fill":
      return <FillForm fields={fields} edit={edit} />;
    case "grid":
      return <GridForm fields={fields} setFields={setFields} />;
  }
}

function Field({ label, unit, value, onChange }: { label: string; unit?: string; value: string; onChange(v: string): void }) {
  const id = useId();
  return (
    <div className="tf-row">
      <label className="dlg-label" htmlFor={id}>
        {label} {unit && <span className="dlg-unit">{unit}</span>}
      </label>
      <input
        id={id}
        className="dlg-input tf-num"
        type="text"
        inputMode="decimal"
        spellCheck={false}
        autoComplete="off"
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />
    </div>
  );
}

function FillForm({ fields, edit }: { fields: DuplicateFields; edit(patch: Partial<DuplicateFields>): void }) {
  const name = useId();
  return (
    <section className="dlg-section" aria-label="Settings">
      <h3 className="dlg-section-title">Settings</h3>
      <Field label="Minimum part distance" unit="mm" value={fields.distance} onChange={(distance) => edit({ distance })} />
      <Field label="Distance to side walls (XY)" unit="mm" value={fields.wall} onChange={(wall) => edit({ wall })} />
      <Field label="Limit copies" value={fields.limit} onChange={(limit) => edit({ limit })} />
      <div className="arr-radios" role="radiogroup" aria-label="Fill">
        <label className="dlg-check">
          <input type="radio" name={name} checked={fields.stack} onChange={() => edit({ stack: true })} />
          <span className="dlg-check-label">Fill the build room in 3D</span>
        </label>
        <label className="dlg-check">
          <input type="radio" name={name} checked={!fields.stack} onChange={() => edit({ stack: false })} />
          <span className="dlg-check-label">Only on the platform (2D)</span>
        </label>
      </div>
      <p className="dlg-note">
        The selection moves to the platform's front-left corner and the copies fill a grid from there by their boxes, keeping clear of the other parts.
      </p>
    </section>
  );
}

function GridForm({ fields, setFields }: { fields: DuplicateFields; setFields(update: (f: DuplicateFields) => DuplicateFields): void }) {
  const parts = useStore((s) => s.parts);
  const selection = useStore((s) => s.selection);
  const platform = useStore((s) => s.platform);
  const size = useMemo(() => {
    const box = groupBox(transformTargets({ parts, selection }).parts);
    return box ? boxSize(box) : [0, 0, 0];
  }, [parts, selection]);
  const typeId = useId();
  const editAxis = (i: number, patch: Partial<GridAxisFields>) =>
    setFields((f) => ({ ...f, axes: f.axes.map((a, k) => (k === i ? { ...a, ...patch } : a)) as DuplicateFields["axes"] }));
  const quantity = gridCounts(fields).reduce((n, c) => n * c, 1);
  const extent = gridExtent(fields, size);
  return (
    <>
      <section className="dlg-section" aria-label="Distance type">
        <h3 className="dlg-section-title">Distance type</h3>
        <select
          id={typeId}
          aria-label="Distance type"
          className="dlg-input dlg-select"
          value={fields.spacing}
          onChange={(e) => setFields((f) => ({ ...f, spacing: e.target.value === "origins" ? "origins" : "boxes" }))}
        >
          <option value="boxes">Gap between bounding boxes</option>
          <option value="origins">Distance between part origins</option>
          <option value="surfaces" disabled>
            Gap between part surfaces (Soon)
          </option>
        </select>
      </section>
      <section className="dlg-section" aria-label="Placement">
        <h3 className="dlg-section-title">Placement</h3>
        <div className="arr-grid" role="table" aria-label="Placement">
          <div className="arr-grid-row arr-grid-head" role="row">
            <span role="columnheader" />
            <span role="columnheader" />
            <span role="columnheader">Count</span>
            <span role="columnheader">Gap (mm)</span>
          </div>
          {fields.axes.map((a, i) => (
            <div key={i} className="arr-grid-row" role="row">
              <label className="dlg-check" role="cell">
                <input type="checkbox" checked={a.on} onChange={(e) => editAxis(i, { on: e.target.checked })} />
                <span className="dlg-check-label">{AXES[i]}</span>
              </label>
              <select
                role="cell"
                aria-label={`${AXES[i]} direction`}
                className="dlg-input dlg-select"
                disabled={!a.on}
                value={a.negative ? "negative" : "positive"}
                onChange={(e) => editAxis(i, { negative: e.target.value === "negative" })}
              >
                <option value="positive">{AXES[i]} axis positive</option>
                <option value="negative">{AXES[i]} axis negative</option>
              </select>
              <input
                role="cell"
                aria-label={`${AXES[i]} count`}
                className="dlg-input tf-num"
                type="text"
                inputMode="numeric"
                disabled={!a.on}
                value={a.on ? a.count : "1"}
                onChange={(e) => editAxis(i, { count: e.target.value })}
              />
              <input
                role="cell"
                aria-label={`${AXES[i]} gap`}
                className="dlg-input tf-num"
                type="text"
                inputMode="decimal"
                disabled={!a.on}
                value={a.gap}
                onChange={(e) => editAxis(i, { gap: e.target.value })}
              />
            </div>
          ))}
        </div>
      </section>
      <section className="dlg-section" aria-label="Total expected size">
        <h3 className="dlg-section-title">Total expected size</h3>
        <div className="tf-row">
          <span className="dlg-label">Quantity</span>
          <output className="dlg-input tf-num arr-readout">{quantity}</output>
        </div>
        <div className="arr-size" role="table" aria-label="Size and platform">
          <div className="arr-size-row arr-grid-head" role="row">
            <span role="columnheader" />
            <span role="columnheader">Size (mm)</span>
            <span role="columnheader">Platform (mm)</span>
          </div>
          {AXES.map((axis, i) => (
            <div key={axis} className="arr-size-row" role="row">
              <span role="rowheader">{axis}</span>
              <output role="cell" className={extent[i] > platform[i] ? "dlg-input tf-num arr-readout is-over" : "dlg-input tf-num arr-readout"}>
                {formatField(extent[i])}
              </output>
              <output role="cell" className="dlg-input tf-num arr-readout">
                {formatField(platform[i])}
              </output>
            </div>
          ))}
        </div>
      </section>
    </>
  );
}
