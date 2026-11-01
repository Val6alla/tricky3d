// Modify › Mirror and Modify › Shear dialogs (no reference shot: unconfirmed layout). Both remember their settings
// and change the selected parts' matrices as one undo step.
import { useId, useState } from "react";
import { SliderNumber } from "../analysis/AnalysisDialog";
import { Dialog } from "../dialogs/Dialog";
import { loadSetting, saveSetting } from "../store";
import type { Axis } from "../matrix";
import { MAX_SHEAR_ANGLE, mirrorParts, shearParts, type MirrorSettings, type Pivot, type ShearSettings } from "./transforms";
import "../analysis/analysis.css";

const AXES: Axis[] = [0, 1, 2];
const NAMES = ["X", "Y", "Z"];

function remembered<T extends object>(key: string, defaults: T): T {
  try {
    const raw = JSON.parse(loadSetting(key) ?? "null") as Record<string, unknown> | null;
    if (!raw || typeof raw !== "object") return defaults;
    const out = { ...defaults } as Record<string, unknown>;
    for (const k of Object.keys(defaults)) if (typeof raw[k] === typeof out[k]) out[k] = raw[k];
    return out as T;
  } catch {
    return defaults;
  }
}

function AxisRow({ label, value, onChange, exclude }: { label: string; value: Axis; onChange(a: Axis): void; exclude?: Axis }) {
  return (
    <fieldset className="tf-row md-axes">
      <legend className="dlg-label">{label}</legend>
      {AXES.map((a) => (
        <label key={a} className="dlg-check md-axis">
          <input type="radio" checked={value === a} disabled={a === exclude} onChange={() => onChange(a)} />
          <span className="dlg-check-label">{NAMES[a]}</span>
        </label>
      ))}
    </fieldset>
  );
}

function PivotRow({ value, onChange }: { value: Pivot; onChange(p: Pivot): void }) {
  const id = useId();
  return (
    <div className="an-slider-row an-select-row">
      <label className="dlg-label" htmlFor={id}>
        About
      </label>
      <select id={id} className="dlg-input dlg-select" value={value} onChange={(e) => onChange(e.target.value as Pivot)}>
        <option value="centre">The part's centre</option>
        <option value="origin">The origin (0, 0, 0)</option>
      </select>
    </div>
  );
}

const MIRROR_KEY = "tricky3d.mirror";
const SHEAR_KEY = "tricky3d.shear";

export function MirrorDialog({ onClose }: { onClose(): void }) {
  const [s, setS] = useState<MirrorSettings>(() => remembered(MIRROR_KEY, { axis: 0 as Axis, pivot: "centre" as Pivot }));
  function ok() {
    saveSetting(MIRROR_KEY, JSON.stringify(s));
    onClose();
    mirrorParts(s);
  }
  return (
    <Dialog title="Mirror" onClose={onClose} onOk={ok} className="dlg-analysis">
      <section className="dlg-section" aria-label="Mirror">
        <AxisRow label="Mirror in" value={s.axis} onChange={(axis) => setS({ ...s, axis })} />
        <PivotRow value={s.pivot} onChange={(pivot) => setS({ ...s, pivot })} />
        <p className="dlg-note">
          Flips the selected parts across the plane at right angles to {NAMES[s.axis]} through {s.pivot === "centre" ? "each part's centre" : "the origin"}. The
          surface keeps facing outwards.
        </p>
      </section>
    </Dialog>
  );
}

export function ShearDialog({ onClose }: { onClose(): void }) {
  const [s, setS] = useState<ShearSettings>(() => remembered(SHEAR_KEY, { along: 0 as Axis, by: 2 as Axis, angle: 10, pivot: "centre" as Pivot }));
  const bad = s.along === s.by;
  function ok() {
    saveSetting(SHEAR_KEY, JSON.stringify(s));
    onClose();
    shearParts(s);
  }
  return (
    <Dialog title="Shear" onClose={onClose} onOk={ok} okDisabled={bad || s.angle === 0} className="dlg-analysis">
      <section className="dlg-section" aria-label="Shear">
        <AxisRow label="Move along" value={s.along} onChange={(along) => setS({ ...s, along, by: along === s.by ? (((along + 1) % 3) as Axis) : s.by })} />
        <AxisRow label="Rising with" value={s.by} exclude={s.along} onChange={(by) => setS({ ...s, by })} />
        <SliderNumber label="Angle" value={s.angle} min={-MAX_SHEAR_ANGLE} max={MAX_SHEAR_ANGLE} step={0.5} unit="°" onChange={(angle) => setS({ ...s, angle })} />
        <PivotRow value={s.pivot} onChange={(pivot) => setS({ ...s, pivot })} />
        <p className="dlg-note">
          Each point moves along {NAMES[s.along]} in proportion to its distance along {NAMES[s.by]} from {s.pivot === "centre" ? "the part's centre" : "the origin"}, so
          lines along {NAMES[s.by]} lean by the angle. The volume does not change.
        </p>
      </section>
    </Dialog>
  );
}
