// The settings of the analyses that have some: Up- and downskins (angles, minimum component size, filter small
// triangles) and Support volume (threshold angle). Each opens on the settings used last; OK runs the analysis.
import { useId, useState, type ReactNode } from "react";
import { Dialog } from "../dialogs/Dialog";
import { loadSetting, saveSetting, useStore } from "../store";
import "./analysis.css";
import { runPartAnalysis } from "./session";
import {
  DEFAULT_SKIN,
  DEFAULT_SUPPORT,
  DEFAULT_WALL,
  KIND_LABELS,
  MAX_SKIN_ANGLE,
  type AnalysisKind,
  type CompareDirection,
  type CompareSettings,
  type SkinSettings,
  type SupportSettings,
  type WallColors,
  type WallSettings,
} from "./types";

const SETTINGS_KEY = "tricky3d.analysisSettings.";

/** The settings `kind` ran with last, else its defaults. */
export function lastSettings<T extends object>(kind: AnalysisKind | "compare-options", defaults: T): T {
  try {
    const raw = JSON.parse(loadSetting(SETTINGS_KEY + kind) ?? "null") as unknown;
    if (typeof raw !== "object" || raw === null) return defaults;
    const out = { ...defaults };
    for (const key of Object.keys(defaults) as (keyof T)[]) {
      const v = (raw as Record<string, unknown>)[key as string];
      if (typeof v === typeof defaults[key] && (typeof v !== "number" || Number.isFinite(v))) out[key] = v as T[keyof T];
    }
    return out;
  } catch {
    return defaults;
  }
}

export function rememberSettings(kind: AnalysisKind | "compare-options", settings: object): void {
  saveSetting(SETTINGS_KEY + kind, JSON.stringify(settings));
}

const clamp = (v: number, min: number, max: number) => Math.min(max, Math.max(min, v));

interface SliderNumberProps {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  /** The slider's own range when the field may go past it. */
  sliderMax?: number;
  unit: string;
  disabled?: boolean;
  onChange(v: number): void;
}

/** A slider and a number field for one value, as the reference's dialogs pair them. */
export function SliderNumber({ label, value, min, max, step, sliderMax = max, unit, disabled, onChange }: SliderNumberProps) {
  const id = useId();
  const [text, setText] = useState(String(value));
  const [shown, setShown] = useState(value);
  if (shown !== value) {
    setShown(value);
    setText(String(value));
  }
  const commit = (raw: string) => {
    const v = Number(raw.replace(",", "."));
    if (Number.isFinite(v)) onChange(clamp(v, min, max));
    else setText(String(value));
  };
  return (
    <div className="an-slider-row">
      <label className="dlg-label" htmlFor={id}>
        {label}
      </label>
      <input
        className="an-slider"
        type="range"
        min={min}
        max={sliderMax}
        step={step}
        value={Math.min(value, sliderMax)}
        disabled={disabled}
        aria-label={label}
        onChange={(e) => onChange(Number(e.target.value))}
      />
      <span className="an-number">
        <input
          id={id}
          className="dlg-input"
          inputMode="decimal"
          value={text}
          disabled={disabled}
          onChange={(e) => setText(e.target.value)}
          onBlur={(e) => commit(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") commit((e.target as HTMLInputElement).value);
          }}
        />
        <span className="dlg-unit">{unit}</span>
      </span>
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

export function AnalysisDialog({ kind, partId, otherId, onClose }: { kind: AnalysisKind; partId: string; otherId?: string; onClose(): void }) {
  if (kind === "skin") return <SkinDialog partId={partId} onClose={onClose} />;
  if (kind === "support") return <SupportDialog partId={partId} onClose={onClose} />;
  if (kind === "wall") return <WallDialog partId={partId} onClose={onClose} />;
  if (kind === "compare") return <CompareDialog partId={partId} otherId={otherId} onClose={onClose} />;
  return null;
}

const WALL_COLORS: { value: WallColors; label: string }[] = [
  { value: "red-green", label: "Red and green" },
  { value: "red-green-gradient", label: "Red to green gradient" },
  { value: "blue-yellow-gradient", label: "Blue to yellow gradient" },
];

function WallDialog({ partId, onClose }: { partId: string; onClose(): void }) {
  const [s, setS] = useState<WallSettings>(() => lastSettings("wall", DEFAULT_WALL));
  const edit = (patch: Partial<WallSettings>) => setS((old) => ({ ...old, ...patch }));
  const colorsId = useId();
  const bad = !(s.threshold > 0);
  function run() {
    rememberSettings("wall", s);
    onClose();
    void runPartAnalysis("wall", partId, s);
  }
  return (
    <Dialog title={KIND_LABELS.wall} onClose={onClose} onOk={run} okDisabled={bad} className="dlg-analysis">
      <Box title="Wall thickness analysis">
        <SliderNumber label="Wall thickness threshold" value={s.threshold} min={0} max={1000} sliderMax={5} step={0.05} unit="mm" onChange={(v) => edit({ threshold: v })} />
      </Box>
      <Box title="Acceptance test">
        <SliderNumber label="Thinner surface allowed" value={s.acceptance} min={0} max={100} sliderMax={20} step={0.5} unit="%" onChange={(v) => edit({ acceptance: v })} />
        <p className="dlg-note">The test fails when more than this share of the surface of the part is thinner than the threshold.</p>
      </Box>
      <Box title="Colours">
        <div className="an-slider-row an-select-row">
          <label className="dlg-label" htmlFor={colorsId}>
            Result colours
          </label>
          <select id={colorsId} className="dlg-input dlg-select" value={s.colors} onChange={(e) => edit({ colors: e.target.value as WallColors })}>
            {WALL_COLORS.map((c) => (
              <option key={c.value} value={c.value}>
                {c.label}
              </option>
            ))}
          </select>
        </div>
      </Box>
      <Box title="Fast mode">
        <label className="dlg-check">
          <input type="checkbox" checked={s.stopEarly} onChange={(e) => edit({ stopEarly: e.target.checked })} />
          <span>
            <span className="dlg-check-label">Stop as soon as the test fails</span>
            <span className="dlg-help">A quick yes or no for big parts. The colours and numbers are then incomplete.</span>
          </span>
        </label>
      </Box>
      <Box title="When closing">
        <label className="dlg-check">
          <input type="checkbox" checked={s.colouredPart} onChange={(e) => edit({ colouredPart: e.target.checked })} />
          <span>
            <span className="dlg-check-label">Make a coloured part from the results when closing</span>
            <span className="dlg-help">Closing the analysis adds a copy of the part painted with its colours.</span>
          </span>
        </label>
      </Box>
      {bad && <p className="dlg-error">The threshold must be more than 0 mm.</p>}
    </Dialog>
  );
}

const DIRECTIONS: { value: CompareDirection; label: string }[] = [
  { value: "both", label: "Both ways" },
  { value: "reference-to-comparison", label: "Reference → comparison" },
  { value: "comparison-to-reference", label: "Comparison → reference" },
];

function CompareDialog({ partId, otherId, onClose }: { partId: string; otherId?: string; onClose(): void }) {
  const parts = useStore((st) => st.parts);
  const [reference, setReference] = useState(partId);
  const [comparison, setComparison] = useState(otherId ?? "");
  const [options, setOptions] = useState(() => lastSettings("compare-options", { direction: "both" as CompareDirection, align: false }));
  const referencePart = parts.find((p) => p.id === reference);
  const comparisonPart = parts.find((p) => p.id === comparison && p.id !== reference);
  const listId = useId();
  const directionId = useId();
  function run() {
    if (!comparisonPart) return;
    rememberSettings("compare-options", options);
    onClose();
    const settings: CompareSettings = {
      comparisonId: comparisonPart.id,
      comparisonName: comparisonPart.name,
      comparisonMatrix: [...comparisonPart.matrix],
      direction: options.direction,
      align: options.align,
    };
    void runPartAnalysis("compare", reference, settings);
  }
  return (
    <Dialog title="Compare meshes" onClose={onClose} onOk={run} okDisabled={!comparisonPart || !referencePart} className="dlg-analysis">
      <Box title="Meshes">
        <div className="an-slider-row an-select-row">
          <span className="dlg-label">Reference</span>
          <span className="an-tile" title={referencePart?.name}>
            {referencePart?.name ?? "—"}
          </span>
        </div>
        <div className="an-slider-row an-select-row">
          <label className="dlg-label" htmlFor={listId}>
            Comparison
          </label>
          <select id={listId} className="dlg-input dlg-select" value={comparisonPart ? comparison : ""} onChange={(e) => setComparison(e.target.value)}>
            <option value="" disabled>
              Pick a part
            </option>
            {parts
              .filter((p) => p.id !== reference)
              .map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
          </select>
        </div>
        <button
          type="button"
          className="dlg-btn an-switch"
          disabled={!comparisonPart}
          onClick={() => {
            setReference(comparison);
            setComparison(reference);
          }}
        >
          Switch reference and comparison
        </button>
      </Box>
      <Box title="Options">
        <div className="an-slider-row an-select-row">
          <label className="dlg-label" htmlFor={directionId}>
            Comparison direction
          </label>
          <select
            id={directionId}
            className="dlg-input dlg-select"
            value={options.direction}
            onChange={(e) => setOptions({ ...options, direction: e.target.value as CompareDirection })}
          >
            {DIRECTIONS.map((d) => (
              <option key={d.value} value={d.value}>
                {d.label}
              </option>
            ))}
          </select>
        </div>
        <label className="dlg-check">
          <input type="checkbox" checked={options.align} onChange={(e) => setOptions({ ...options, align: e.target.checked })} />
          <span>
            <span className="dlg-check-label">Align the meshes first</span>
            <span className="dlg-help">Fit the comparison onto the reference as well as it goes before measuring. Neither part moves.</span>
          </span>
        </label>
        <p className="dlg-note">Copies of the parts are made finer for the comparison; the parts themselves do not change.</p>
      </Box>
    </Dialog>
  );
}

function SkinDialog({ partId, onClose }: { partId: string; onClose(): void }) {
  const [s, setS] = useState<SkinSettings>(() => lastSettings("skin", DEFAULT_SKIN));
  const edit = (patch: Partial<SkinSettings>) => setS((old) => ({ ...old, ...patch }));
  const nothing = !s.upskins && !s.downskins;
  function run() {
    rememberSettings("skin", s);
    onClose();
    void runPartAnalysis("skin", partId, s);
  }
  return (
    <Dialog title={KIND_LABELS.skin} onClose={onClose} onOk={run} okDisabled={nothing} className="dlg-analysis">
      <Box title="Angles">
        <label className="dlg-check">
          <input type="checkbox" checked={s.upskins} onChange={(e) => edit({ upskins: e.target.checked })} />
          <span className="dlg-check-label">Upskins</span>
        </label>
        <SliderNumber label="Upskins up to" value={s.upAngle} min={0} max={MAX_SKIN_ANGLE} step={0.1} unit="°" disabled={!s.upskins} onChange={(v) => edit({ upAngle: v })} />
        <label className="dlg-check">
          <input type="checkbox" checked={s.downskins} onChange={(e) => edit({ downskins: e.target.checked })} />
          <span className="dlg-check-label">Downskins</span>
        </label>
        <SliderNumber
          label="Downskins up to"
          value={s.downAngle}
          min={0}
          max={MAX_SKIN_ANGLE}
          step={0.1}
          unit="°"
          disabled={!s.downskins}
          onChange={(v) => edit({ downAngle: v })}
        />
        <p className="dlg-note">A triangle counts when its surface is tilted against the platform by at most this angle: 0° is flat.</p>
      </Box>
      <Box title="Parameters">
        <SliderNumber
          label="Minimum component size"
          value={Number((s.minComponent / 100).toFixed(4))}
          min={0}
          max={1e6}
          sliderMax={1}
          step={0.01}
          unit="cm²"
          onChange={(v) => edit({ minComponent: v * 100 })}
        />
        <label className="dlg-check">
          <input type="checkbox" checked={s.filterSmall} onChange={(e) => edit({ filterSmall: e.target.checked })} />
          <span>
            <span className="dlg-check-label">Filter small triangles</span>
            <span className="dlg-help">Small areas at safe angles that a skin surrounds count as that skin.</span>
          </span>
        </label>
      </Box>
      {nothing && <p className="dlg-error">Pick upskins, downskins or both.</p>}
    </Dialog>
  );
}

function SupportDialog({ partId, onClose }: { partId: string; onClose(): void }) {
  const [s, setS] = useState<SupportSettings>(() => lastSettings("support", DEFAULT_SUPPORT));
  function run() {
    rememberSettings("support", s);
    onClose();
    void runPartAnalysis("support", partId, s);
  }
  return (
    <Dialog title={KIND_LABELS.support} onClose={onClose} onOk={run} className="dlg-analysis">
      <Box title="Support definition">
        <SliderNumber label="Threshold angle" value={s.angle} min={0} max={90} step={0.5} unit="°" onChange={(angle) => setS({ angle })} />
        <p className="dlg-note">
          Surfaces facing down and tilted against the platform by at most this angle need support. The result is an estimate: solid
          blocks from those surfaces straight down to the platform.
        </p>
      </Box>
    </Dialog>
  );
}
