// Label generation › the right panel for the selected label, top to bottom: what the label shows (Caption and Font;
// the picture and Threshold; the list), then Height, Width with Keep aspect ratio, Depth with a slider, Build type (a
// tag's Thickness and Variant), and Apply and Cancel at the bottom. Every change goes through src/label/session.ts.
import { useEffect, useId, useMemo, useRef, useState } from "react";
import { ImagePlus } from "lucide-react";
import { Dialog } from "../dialogs/Dialog";
import { parseNumber } from "../numbers";
import { FontDialog } from "./FontDialog";
import { useLabel } from "./store";
import {
  answer,
  applyLabelMode,
  cancelApply,
  closeLabelMode,
  labelWidth,
  pickLabelCsv,
  pickLabelPicture,
  shapeKey,
  updateLabel,
} from "./session";
import { BUILD_TYPES, DEFAULT_NUMBERS, KIND_LABELS, listEntries, type BuildType, type ListSource, type PlacedLabel } from "./types";
import "./label.css";

/** Typing must not reach the app shortcuts. */
function keys(e: React.KeyboardEvent) {
  e.stopPropagation();
}

const DEPTH_MAX = 10;

/** A number field that keeps what is typed until it parses, then sets `onValue` (more than `min`). */
function NumberField({
  value,
  onValue,
  min = 0,
  disabled = false,
  label,
  integer = false,
}: {
  value: number;
  onValue(v: number): void;
  min?: number;
  disabled?: boolean;
  label: string;
  integer?: boolean;
}) {
  const shown = integer ? String(Math.round(value)) : String(Math.round(value * 100) / 100);
  const [text, setText] = useState(shown);
  const focused = useRef(false);
  useEffect(() => {
    if (!focused.current) setText(shown);
  }, [shown]);
  const parsed = parseNumber(text);
  const wrong = parsed === null || parsed <= min || (integer && !Number.isInteger(parsed));
  return (
    <input
      className="dlg-input lbl-number"
      type="text"
      inputMode={integer ? "numeric" : "decimal"}
      aria-label={label}
      aria-invalid={wrong ? true : undefined}
      disabled={disabled}
      value={disabled ? shown : text}
      onFocus={() => (focused.current = true)}
      onBlur={() => {
        focused.current = false;
        setText(shown);
      }}
      onChange={(e) => {
        setText(e.target.value);
        const v = parseNumber(e.target.value);
        if (v !== null && v > min && (!integer || Number.isInteger(v))) onValue(v);
      }}
      onKeyDown={keys}
    />
  );
}

/** A thumbnail of the picture with the parts darker than the threshold shown solid: what will be raised or sunk. */
function PicturePreview({ label }: { label: PlacedLabel }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const picture = label.picture;
  useEffect(() => {
    const c = canvas.current;
    if (!c || !picture) return;
    c.width = picture.width;
    c.height = picture.height;
    const context = c.getContext("2d");
    if (!context) return;
    const image = context.createImageData(picture.width, picture.height);
    for (let i = 0; i < picture.gray.length; i++) {
      const dark = picture.gray[i] < label.threshold;
      const g = dark ? 40 : 235;
      image.data.set([g, g, g, 255], i * 4);
    }
    context.putImageData(image, 0, 0);
  }, [picture, label.threshold]);
  return (
    <button type="button" className="lbl-picture" onClick={() => void pickLabelPicture(label.id)} title="Pick a PNG, JPEG or BMP picture.">
      {picture ? (
        <canvas ref={canvas} className="lbl-picture-canvas" aria-label={picture.name} />
      ) : (
        <span className="lbl-picture-empty">
          <ImagePlus size={28} aria-hidden="true" />
          Click to add image
        </span>
      )}
    </button>
  );
}

function ListFields({ label }: { label: PlacedLabel }) {
  const source = label.list;
  const set = (list: ListSource, step = "Change list") => updateLabel(label.id, { list }, step);
  const entries = useMemo(() => listEntries(source), [source]);
  const example = entries.slice(0, 3).join(", ") + (entries.length > 3 ? ", …" : "");
  const columns = source.kind === "csv" ? Math.max(0, ...source.rows.map((r) => r.length)) : 0;
  return (
    <>
      <div className="lbl-row">
        <span className="lbl-label">List</span>
        <div className="lbl-segmented" role="radiogroup" aria-label="List">
          <label>
            <input type="radio" checked={source.kind === "numbers"} onChange={() => set(DEFAULT_NUMBERS, "Running numbers")} />
            Running numbers
          </label>
          <label>
            <input type="radio" checked={source.kind === "csv"} onChange={() => void pickLabelCsv(label.id)} />
            CSV file
          </label>
        </div>
      </div>
      {source.kind === "numbers" ? (
        <div className="lbl-grid">
          {(["start", "step", "count", "digits"] as const).map((key) => (
            <label key={key} className="lbl-cell">
              <span className="lbl-label">{{ start: "Start", step: "Step", count: "Count", digits: "Digits" }[key]}</span>
              <NumberField
                label={key}
                integer
                min={key === "count" ? 0 : key === "digits" ? -1 : -1e9}
                value={source[key]}
                onValue={(v) => set({ ...source, [key]: Math.min(v, key === "count" ? 500 : key === "digits" ? 12 : v) })}
              />
            </label>
          ))}
          {(["prefix", "suffix"] as const).map((key) => (
            <label key={key} className="lbl-cell">
              <span className="lbl-label">{key === "prefix" ? "Prefix" : "Suffix"}</span>
              <input className="dlg-input" type="text" value={source[key]} onChange={(e) => set({ ...source, [key]: e.target.value })} onKeyDown={keys} />
            </label>
          ))}
        </div>
      ) : (
        <div className="lbl-grid">
          <span className="lbl-file" title={source.file}>
            {source.file}
          </span>
          <button type="button" className="dlg-btn" onClick={() => void pickLabelCsv(label.id)}>
            Load CSV file…
          </button>
          <label className="lbl-cell">
            <span className="lbl-label">Column</span>
            <select className="dlg-input" value={source.column} onChange={(e) => set({ ...source, column: Number(e.target.value) })} onKeyDown={keys}>
              {[...Array(columns).keys()].map((c) => (
                <option key={c} value={c}>
                  {source.header ? (source.rows[0]?.[c] ?? `Column ${c + 1}`) : `Column ${c + 1}`}
                </option>
              ))}
            </select>
          </label>
          <label className="lbl-check">
            <input type="checkbox" checked={source.header} onChange={(e) => set({ ...source, header: e.target.checked })} />
            First row is a header
          </label>
        </div>
      )}
      <p className="lbl-note" role="status">
        {entries.length === 0 ? "The list is empty." : `${entries.length} ${entries.length === 1 ? "copy" : "copies"}: ${example}`}
      </p>
    </>
  );
}

function Form({ label }: { label: PlacedLabel }) {
  const captionId = useId();
  const [fontOpen, setFontOpen] = useState(false);
  // Subscribed, so the width follows the shape once Rust has drawn it.
  const entry = useLabel((s) => s.shapes[shapeKey(label)]);
  const width = labelWidth(label);
  const set = (patch: Partial<PlacedLabel>, step: string) => updateLabel(label.id, patch, step);
  const usesFont = label.kind !== "image";
  return (
    <div className="lbl-form">
      <h3 className="lib-subheading">{KIND_LABELS[label.kind]}</h3>
      {(label.kind === "text" || label.kind === "tag") && (
        <label className="lbl-row lbl-row-top" htmlFor={captionId}>
          <span className="lbl-label">Caption</span>
          <textarea
            id={captionId}
            className="dlg-input lbl-caption"
            value={label.caption}
            spellCheck={false}
            onChange={(e) => set({ caption: e.target.value }, "Change caption")}
            onKeyDown={keys}
          />
        </label>
      )}
      {label.kind === "image" && (
        <>
          <div className="lbl-row lbl-row-top">
            <span className="lbl-label">Image</span>
            <PicturePreview label={label} />
          </div>
          <div className="lbl-row">
            <span className="lbl-label">Threshold</span>
            <input
              type="range"
              min={0.02}
              max={0.98}
              step={0.01}
              value={label.threshold}
              aria-label="Threshold"
              title="Parts of the picture darker than this are raised or sunk."
              onChange={(e) => set({ threshold: Number(e.target.value) }, "Change threshold")}
            />
          </div>
        </>
      )}
      {label.kind === "list" && <ListFields label={label} />}
      {usesFont && (
        <div className="lbl-row">
          <span className="lbl-label">Font</span>
          <div className="lbl-font">
            <button type="button" className="dlg-btn lbl-font-name" onClick={() => setFontOpen(true)} title="Pick the font, its style and size.">
              {label.font.family}
              {label.font.style && label.font.style !== "Regular" ? ` ${label.font.style}` : ""}
            </button>
            <button type="button" className="dlg-btn" aria-label="Font, style and size" onClick={() => setFontOpen(true)}>
              …
            </button>
          </div>
        </div>
      )}
      {entry?.status === "error" && (
        <p className="lbl-problem" role="alert">
          {entry.message}
        </p>
      )}
      <div className="lbl-row">
        <span className="lbl-label">Height</span>
        <span className="lbl-with-unit">
          <NumberField label="Height" value={label.height} onValue={(height) => set({ height }, "Change height")} />
          <span className="dlg-unit">mm</span>
        </span>
      </div>
      <div className="lbl-row">
        <span className={`lbl-label${label.keepAspect ? " is-disabled" : ""}`}>Width</span>
        <span className="lbl-with-unit">
          <NumberField label="Width" value={width ?? label.width} disabled={label.keepAspect} onValue={(w) => set({ width: w }, "Change width")} />
          <span className="dlg-unit">mm</span>
          <label className="lbl-check">
            <input type="checkbox" checked={label.keepAspect} onChange={(e) => set({ keepAspect: e.target.checked }, "Keep aspect ratio")} />
            Keep aspect ratio
          </label>
        </span>
      </div>
      <div className="lbl-row">
        <span className="lbl-label">Depth</span>
        <span className="lbl-with-unit">
          <NumberField label="Depth" value={label.depth} onValue={(depth) => set({ depth }, "Change depth")} />
          <span className="dlg-unit">mm</span>
          <input
            type="range"
            className="lbl-slider"
            min={0.1}
            max={DEPTH_MAX}
            step={0.1}
            value={Math.min(label.depth, DEPTH_MAX)}
            aria-label="Depth slider"
            onChange={(e) => set({ depth: Number(e.target.value) }, "Change depth")}
          />
        </span>
      </div>
      {label.kind === "tag" && (
        <div className="lbl-row">
          <span className="lbl-label">Thickness</span>
          <span className="lbl-with-unit">
            <NumberField label="Thickness" value={label.thickness} onValue={(thickness) => set({ thickness }, "Change thickness")} />
            <span className="dlg-unit">mm</span>
          </span>
        </div>
      )}
      <div className="lbl-row">
        <span className="lbl-label">{label.kind === "tag" ? "Variant" : "Build type"}</span>
        <select className="dlg-input" value={label.build} onChange={(e) => set({ build: e.target.value as BuildType }, "Change build type")} onKeyDown={keys}>
          {BUILD_TYPES.map((b) => (
            <option key={b.value} value={b.value}>
              {b.label}
            </option>
          ))}
        </select>
      </div>
      {fontOpen && (
        <FontDialog
          font={label.font}
          size={label.fontSize}
          onClose={() => setFontOpen(false)}
          onPick={(font, fontSize) => {
            set({ font, fontSize }, "Change font");
            setFontOpen(false);
          }}
        />
      )}
    </div>
  );
}

/** Apply's "remove the original?" and Close's "leave without applying?", with Yes and No. */
function Question() {
  const question = useLabel((s) => s.question);
  if (!question) return null;
  return (
    <Dialog title={question.title} onOk={() => answer(true)} okLabel="Yes" onClose={() => answer(false)} closeLabel="No" className="lbl-question">
      <p className="lbl-question-text">{question.text}</p>
    </Dialog>
  );
}

export function LabelPanel() {
  const session = useLabel((s) => s.session);
  const busy = useLabel((s) => s.busy);
  if (!session) return null;
  const label = session.labels.find((l) => l.id === session.selected) ?? null;
  return (
    <section className="lib-panel lbl-panel" aria-label="Label options">
      {label ? (
        <Form key={label.id} label={label} />
      ) : (
        <p className="lbl-empty">
          {session.labels.length === 0
            ? "Click on the part where the label goes."
            : "Click a label in the view or under the part in the parts list to change it, or Place label to add another."}
        </p>
      )}
      <Question />
      <div className="lib-actions">
        {busy ? (
          <>
            <span className="lib-busy" role="status">
              <span className="statusbar-spinner" aria-hidden="true" />
              {busy.cancelling ? "Cancelling…" : `Adding the labels… ${Math.round(busy.fraction * 100)}%`}
            </span>
            <button type="button" className="dlg-btn" onClick={cancelApply} disabled={busy.cancelling}>
              Cancel
            </button>
          </>
        ) : (
          <>
            <button type="button" className="dlg-btn dlg-btn-primary lbl-action" onClick={() => void applyLabelMode()}>
              Apply
            </button>
            <button type="button" className="dlg-btn lbl-action" onClick={() => void closeLabelMode()}>
              Cancel
            </button>
          </>
        )}
      </div>
    </section>
  );
}
