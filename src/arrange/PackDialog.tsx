// Home › Pack and Arrange › Packing: one dialog, the method chosen at the top, then the reference's layout for that
// method: Manage settings (presets), Performance, the method's options in its boxes, and Pack platform. The run shows
// its progress and Cancel in the busy box (src/arrange/ArrangeBusy.tsx); Cancel leaves the project as it was.
import { useId, useState, type ReactNode } from "react";
import { Dialog } from "../dialogs/Dialog";
import { PresetRow } from "../dialogs/PresetRow";
import { PACKING_PRESETS } from "../dialogs/presets";
import "../dialogs/transform.css";
import { useStore } from "../store";
import "./arrange.css";
import { checkPack, loadPack, PACK_METHODS, pickFields, presetValues, runPack, Z_STEPS, type PackFields, type PackMethod } from "./pack";

export function PackDialog({ method, onClose }: { method: PackMethod; onClose(): void }) {
  const platformHeight = useStore((s) => s.platform[2]);
  const [fields, setFields] = useState<PackFields>(() => loadPack(method));
  const check = checkPack(fields, platformHeight);
  const edit = (patch: Partial<PackFields>) => setFields((f) => ({ ...f, ...patch }));

  function pack() {
    if (!check.settings) return;
    onClose();
    void runPack(fields);
  }

  return (
    <Dialog title="Pack parts" onClose={onClose} onOk={pack} okLabel="Pack platform" okDisabled={!check.settings} className="dlg-arrange dlg-pack">
      <MethodPicker value={fields.method} onChange={(m) => setFields(loadPack(m))} />
      <PresetRow
        store={PACKING_PRESETS}
        scope={fields.method}
        values={presetValues(fields)}
        onLoad={(saved) => setFields((f) => ({ ...f, ...pickFields(saved as Partial<PackFields>) }))}
      />
      <PackForm fields={fields} edit={edit} />
      {check.problem && (
        <p className="dlg-error" role="alert">
          {check.problem}
        </p>
      )}
    </Dialog>
  );
}

function MethodPicker({ value, onChange }: { value: PackMethod; onChange(m: PackMethod): void }) {
  const id = useId();
  return (
    <div className="tf-row arr-method">
      <label className="dlg-label" htmlFor={id}>
        Packing method
      </label>
      <select id={id} className="dlg-input dlg-select" value={value} onChange={(e) => onChange(e.target.value as PackMethod)}>
        {PACK_METHODS.map((m) => (
          <option key={m.value} value={m.value} disabled={m.soon}>
            {m.soon ? `${m.label} (Soon)` : m.label}
          </option>
        ))}
      </select>
    </div>
  );
}

function Box({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="dlg-section" aria-label={title}>
      <h3 className="dlg-section-title">{title}</h3>
      {children}
    </section>
  );
}

function Mm({ label, value, onChange, disabled }: { label: string; value: string; onChange(v: string): void; disabled?: boolean }) {
  const id = useId();
  return (
    <div className="tf-row">
      <label className="dlg-label" htmlFor={id}>
        {label} <span className="dlg-unit">mm</span>
      </label>
      <input
        id={id}
        className="dlg-input tf-num"
        type="text"
        inputMode="decimal"
        spellCheck={false}
        autoComplete="off"
        disabled={disabled}
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />
    </div>
  );
}

function Check({ label, checked, onChange, help, disabled }: { label: string; checked: boolean; onChange(v: boolean): void; help?: string; disabled?: boolean }) {
  return (
    <label className="dlg-check">
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
      <span>
        <span className="dlg-check-label">{label}</span>
        {help && <span className="dlg-help">{help}</span>}
      </span>
    </label>
  );
}

function Choice<T extends string | number>({ label, value, options, onChange }: { label: string; value: T; options: { value: T; label: string }[]; onChange(v: T): void }) {
  const id = useId();
  return (
    <div className="tf-row">
      <label className="dlg-label" htmlFor={id}>
        {label}
      </label>
      <select
        id={id}
        className="dlg-input dlg-select"
        value={String(value)}
        onChange={(e) => onChange(options.find((o) => String(o.value) === e.target.value)?.value ?? value)}
      >
        {options.map((o) => (
          <option key={String(o.value)} value={String(o.value)}>
            {o.label}
          </option>
        ))}
      </select>
    </div>
  );
}

function Performance({ value, onChange }: { value: number; onChange(v: number): void }) {
  return (
    <Box title="Performance">
      <input
        className="arr-slider"
        type="range"
        min={1}
        max={9}
        step={1}
        aria-label="Performance, from accurate to fast"
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
      />
      <div className="arr-slider-ends" aria-hidden="true">
        <span>Accurate</span>
        <span>Fast</span>
      </div>
    </Box>
  );
}

function Selection({ fields, edit }: { fields: PackFields; edit(patch: Partial<PackFields>): void }) {
  const name = useId();
  return (
    <div className="arr-radios" role="radiogroup" aria-label="Part selection">
      <label className="dlg-check">
        <input type="radio" name={name} checked={fields.onlySelected} onChange={() => edit({ onlySelected: true })} />
        <span className="dlg-check-label">Pack only selected parts</span>
      </label>
      <label className="dlg-check">
        <input type="radio" name={name} checked={!fields.onlySelected} onChange={() => edit({ onlySelected: false })} />
        <span className="dlg-check-label">Pack all parts</span>
      </label>
    </div>
  );
}

function HeightLimit({ fields, edit, label }: { fields: PackFields; edit(patch: Partial<PackFields>): void; label: string }) {
  const id = useId();
  return (
    <div className="tf-row">
      <label className="dlg-check">
        <input type="checkbox" checked={fields.limitHeight} onChange={(e) => edit({ limitHeight: e.target.checked })} />
        <span className="dlg-check-label">{label}</span>
      </label>
      <input
        id={id}
        aria-label={`${label}, mm`}
        className="dlg-input tf-num"
        type="text"
        inputMode="decimal"
        disabled={!fields.limitHeight}
        value={fields.height}
        onChange={(e) => edit({ height: e.target.value })}
      />
    </div>
  );
}

const VOXELS_2D = ["0.5", "1", "1.5", "2", "2.5"].map((v) => ({ value: v, label: `${v} mm` }));
const VOXELS_3D = ["1", "2", "3", "4", "5"].map((v) => ({ value: v, label: `${v} mm` }));
const SPACING = [
  { value: 0, label: "None" },
  { value: 1, label: "Small" },
  { value: 2, label: "Large" },
];

/** The chosen method's settings, in the reference's boxes and order. */
export function PackForm({ fields, edit }: { fields: PackFields; edit(patch: Partial<PackFields>): void }) {
  const dims = useId();
  const rot = useId();
  switch (fields.method) {
    case "bounding-box":
      return (
        <>
          <Box title="Packing options">
            <Mm label="Minimum part distance" value={fields.distance} onChange={(distance) => edit({ distance })} />
            <Mm label="Voxel size" value={fields.voxel} onChange={(voxel) => edit({ voxel })} />
            <Mm label="Distance to side walls (XY)" value={fields.wall} onChange={(wall) => edit({ wall })} />
            <Mm label="Distance to platform (Z)" value={fields.floor} onChange={(floor) => edit({ floor })} />
            <Mm label="Distance to ceiling (Z)" value={fields.ceiling} onChange={(ceiling) => edit({ ceiling })} />
          </Box>
          <Box title="Packing dimensions">
            <div className="arr-radios" role="radiogroup" aria-label="Packing dimensions">
              <label className="dlg-check">
                <input type="radio" name={dims} checked={fields.stack} onChange={() => edit({ stack: true })} />
                <span className="dlg-check-label">In 3D</span>
              </label>
              <label className="dlg-check">
                <input type="radio" name={dims} checked={!fields.stack} onChange={() => edit({ stack: false })} />
                <span className="dlg-check-label">Only in 2D</span>
              </label>
            </div>
            <p className="dlg-note">Parts keep their orientation. Rotate to smallest box first for a tighter result.</p>
          </Box>
          <Box title="Part selection">
            <Selection fields={fields} edit={edit} />
          </Box>
        </>
      );
    case "scanline2d":
      return (
        <>
          <Performance value={fields.performance} onChange={(performance) => edit({ performance })} />
          <Box title="Packing options">
            <Check label="Optimise alignment" checked={fields.align} onChange={(align) => edit({ align })} help="Turn each part to its smallest footprint first." />
            <Choice label="Z-rotation steps" value={fields.zSteps} options={Z_STEPS} onChange={(zSteps) => edit({ zSteps })} />
            <Mm label="Distance to platform border" value={fields.wall} onChange={(wall) => edit({ wall })} />
          </Box>
          <Box title="Expert packing options">
            <Choice label="Distance between parts" value={fields.spacing} options={SPACING} onChange={(spacing) => edit({ spacing })} />
            <Choice label="Voxel size" value={fields.voxel} options={VOXELS_2D} onChange={(voxel) => edit({ voxel })} />
          </Box>
          <Box title="Part selection">
            <Selection fields={fields} edit={edit} />
          </Box>
        </>
      );
    case "scanline3d":
      return (
        <>
          <Performance value={fields.performance} onChange={(performance) => edit({ performance })} />
          <Box title="Other performance settings">
            <Choice label="Voxel size" value={fields.voxel} options={VOXELS_3D} onChange={(voxel) => edit({ voxel })} />
            <Choice label="Z-rotation steps" value={fields.zSteps} options={Z_STEPS} onChange={(zSteps) => edit({ zSteps })} />
          </Box>
          <Box title="Part settings">
            <Check label="Allow parts to be turned upside down" checked={fields.flip} onChange={(flip) => edit({ flip })} />
            <Selection fields={fields} edit={edit} />
          </Box>
          <Box title="Platform settings">
            <Check
              label="Prevent interlocking"
              checked
              disabled
              onChange={() => {}}
              help="Always on here: every part comes down from above onto the others, so each can be lifted straight out. That packs less tightly than letting parts go under others."
            />
            <HeightLimit fields={fields} edit={edit} label="Limit platform height" />
            <Mm label="Distance to side walls (XY)" value={fields.wall} onChange={(wall) => edit({ wall })} />
            <Mm label="Distance to platform (Z)" value={fields.floor} onChange={(floor) => edit({ floor })} />
          </Box>
        </>
      );
    case "monte-carlo":
      return (
        <>
          <Performance value={fields.performance} onChange={(performance) => edit({ performance })} />
          <Box title="Part settings">
            <Mm label="Minimum distance between parts" value={fields.distance} onChange={(distance) => edit({ distance })} />
            <div className="arr-radios" role="radiogroup" aria-label="Part rotation">
              {(
                [
                  ["free", "Arbitrary"],
                  ["z", "Around Z axis only"],
                  ["none", "No rotation"],
                ] as const
              ).map(([value, label]) => (
                <label key={value} className="dlg-check">
                  <input type="radio" name={rot} checked={fields.rotation === value} onChange={() => edit({ rotation: value })} />
                  <span className="dlg-check-label">{label}</span>
                </label>
              ))}
            </div>
            <p className="dlg-note">A part's own Packing settings (right-click) can set its priority and rotation.</p>
            <Selection fields={fields} edit={edit} />
          </Box>
          <Box title="Platform settings">
            <HeightLimit fields={fields} edit={edit} label="Limit fill height" />
            <Mm label="Distance from side walls (XY)" value={fields.wall} onChange={(wall) => edit({ wall })} />
            <Mm label="Distance from platform (Z)" value={fields.floor} onChange={(floor) => edit({ floor })} />
            <div className="tf-row">
              <label className="dlg-label" htmlFor={`${rot}-seed`}>
                Seed
              </label>
              <input
                id={`${rot}-seed`}
                className="dlg-input tf-num"
                type="text"
                inputMode="numeric"
                value={fields.seed}
                onChange={(e) => edit({ seed: e.target.value })}
              />
            </div>
            <p className="dlg-note">The same seed and settings always give the same packing.</p>
          </Box>
        </>
      );
  }
}

