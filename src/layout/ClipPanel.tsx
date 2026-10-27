import { useMemo, useState, type FocusEvent, type KeyboardEvent } from "react";
import { Camera, FlipHorizontal2, RotateCcw } from "lucide-react";
import { parseNumber } from "../numbers";
import {
  clampTo,
  formatMm,
  rowOffset,
  rowRange,
  sceneBounds,
  sliderStep,
  useClip,
  type ClipAxis,
  type Range,
} from "../clip";
import type { Bounds } from "../mesh";
import { useStore, type Vec3 } from "../store";
import { toast } from "../ui";
import { viewport } from "../viewport/api";
import "./clip.css";

const ROWS: { axis: ClipAxis; label: string }[] = [
  { axis: "x", label: "X" },
  { axis: "y", label: "Y" },
  { axis: "z", label: "Z" },
  { axis: "custom", label: "Custom" },
];

const AXIS_NAMES = ["X", "Y", "Z"] as const;

/** Clip planes: X, Y, Z and Custom rows, Transparent cuts and Reset. The planes only change the view. */
export function ClipPanel() {
  const parts = useStore((s) => s.parts);
  const platform = useStore((s) => s.platform);
  const bounds = useMemo(() => sceneBounds(platform, parts), [platform, parts]);
  const clip = useClip();
  const ranges = ROWS.map(({ axis }) => rowRange(axis, clip, bounds));
  // Every row's field fits the longest value any row can show, so the sliders line up and keep their length.
  const fieldChars = Math.max(...ranges.flat().map((v) => formatMm(v).length));

  return (
    <div className="clip">
      {ROWS.map(({ axis, label }, i) => (
        <PlaneRow key={axis} axis={axis} label={label} range={ranges[i]} bounds={bounds} fieldChars={fieldChars} />
      ))}
      <div className="clip-footer">
        <label className="clip-check">
          <input type="checkbox" checked={clip.transparentCuts} onChange={(e) => clip.setTransparentCuts(e.target.checked)} />
          Transparent cuts
        </label>
        <button type="button" className="clip-btn" title="Turn all planes off and move them back to the middle" onClick={clip.reset}>
          <RotateCcw size={13} aria-hidden="true" />
          Reset
        </button>
      </div>
    </div>
  );
}

interface PlaneRowProps {
  axis: ClipAxis;
  label: string;
  range: Range;
  bounds: Bounds;
  /** Characters the mm field is sized for. */
  fieldChars: number;
}

function PlaneRow({ axis, label, range, bounds, fieldChars }: PlaneRowProps) {
  const row = useClip((s) => s.rows[axis]);
  const { setEnabled, setOffset, setFlipped } = useClip.getState();
  const offset = rowOffset(row, range);
  const step = sliderStep(range[1] - range[0]);
  // Slider ends on whole steps so dragging shows round values; the value itself stays within the range.
  const sliderMin = roundStep(Math.floor(range[0] / step) * step);
  const sliderMax = roundStep(Math.ceil(range[1] / step) * step);
  const off = !row.enabled;
  const name = axis === "custom" ? "Custom plane" : `${label} plane`;

  return (
    <div className={`clip-row${off ? " is-off" : ""}`} role="group" aria-label={name}>
      <div className="clip-row-head">
        <label className="clip-check clip-row-name">
          <input
            type="checkbox"
            aria-label={name}
            checked={row.enabled}
            onChange={(e) => setEnabled(axis, e.target.checked, range)}
          />
          <span className={`clip-axis clip-axis-${axis}`}>{label}</span>
        </label>
        <button
          type="button"
          className="clip-icon-btn"
          aria-label={`Flip ${name} side`}
          aria-pressed={row.flipped}
          title={row.flipped ? "Flip side: keeping the other half. Click to flip back." : "Flip side: keep the other half"}
          disabled={off}
          onClick={() => setFlipped(axis, !row.flipped)}
        >
          <FlipHorizontal2 size={14} aria-hidden="true" />
        </button>
      </div>
      <div className="clip-row-ctrl">
        <input
          type="range"
          className="clip-slider"
          aria-label={`${name} position`}
          aria-valuetext={`${formatMm(offset)} mm`}
          min={sliderMin}
          max={sliderMax}
          step={step}
          value={offset}
          disabled={off}
          onChange={(e) => setOffset(axis, clampTo(Number(e.target.value), range))}
        />
        <NumberField
          className="clip-mm"
          label={`${name} position in mm`}
          value={offset}
          format={formatMm}
          chars={fieldChars}
          unit="mm"
          disabled={off}
          onCommit={(v) => setOffset(axis, clampTo(v, range))}
        />
      </div>
      {axis === "custom" && row.enabled && <NormalEditor bounds={bounds} />}
    </div>
  );
}

/** Custom plane direction: three fields (normalised when applied) and quick picks. */
function NormalEditor({ bounds }: { bounds: Bounds }) {
  const normal = useClip((s) => s.customNormal);
  const setCustomNormal = useClip((s) => s.setCustomNormal);
  const [draft, setDraft] = useState<string[] | null>(null);
  const shown = draft ?? normal.map(formatComponent);

  function apply(v: Vec3) {
    if (!setCustomNormal(v, bounds)) toast("The direction cannot be zero on all three axes.", "warn");
  }

  // The three fields form one vector, so typing goes on until Enter or focus leaves the group.
  function commitDraft() {
    if (!draft) return;
    setDraft(null);
    const v = draft.map(parseNumber);
    if (v.every((x): x is number => x !== null)) apply(v as Vec3);
    else toast("Enter a number in each direction field.", "warn");
  }

  function onBlur(e: FocusEvent<HTMLDivElement>) {
    if (!e.currentTarget.contains(e.relatedTarget as Node | null)) commitDraft();
  }

  function onKeyDown(e: KeyboardEvent<HTMLInputElement>) {
    if (e.key === "Enter") {
      commitDraft();
      selectAfterRender(e.currentTarget);
    } else if (e.key === "Escape" && draft) {
      e.preventDefault();
      setDraft(null);
    }
  }

  function applyViewDirection() {
    // The view direction points away from the camera; the plane faces the camera, so the half
    // nearest the viewer is cut away and the inside shows.
    // Tiny components are dropped (the top view sits 1e-4 off the pole) so preset views give exactly axis-aligned planes.
    const [x, y, z] = viewport.viewDirection().map((v) => (Math.abs(v) < 1e-3 ? 0 : -v));
    apply([x, y, z]);
  }

  return (
    <div className="clip-normal">
      <div className="clip-normal-head">
        <span className="clip-normal-label">Direction</span>
        <div className="clip-quick" role="group" aria-label="Quick directions">
          {AXIS_NAMES.map((a, i) => (
            <button
              key={a}
              type="button"
              className="clip-quick-btn"
              title={`Point along +${a}`}
              onClick={() => apply([i === 0 ? 1 : 0, i === 1 ? 1 : 0, i === 2 ? 1 : 0])}
            >
              +{a}
            </button>
          ))}
          <button type="button" className="clip-quick-btn" aria-label="Use view direction" title="Use view direction" onClick={applyViewDirection}>
            <Camera size={13} aria-hidden="true" />
          </button>
        </div>
      </div>
      <div className="clip-normal-fields" onBlur={onBlur}>
        {AXIS_NAMES.map((a, i) => (
          <label key={a} className="clip-field clip-normal-field">
            <span className={`clip-field-tag clip-axis clip-axis-${a.toLowerCase()}`} aria-hidden="true">
              {a}
            </span>
            <input
              className="clip-input"
              type="text"
              inputMode="decimal"
              spellCheck={false}
              autoComplete="off"
              aria-label={`Direction ${a}`}
              value={shown[i]}
              onFocus={(e) => e.currentTarget.select()}
              onChange={(e) => {
                const next = [...shown];
                next[i] = e.target.value;
                setDraft(next);
              }}
              onKeyDown={onKeyDown}
            />
          </label>
        ))}
      </div>
    </div>
  );
}

/** Drops float noise such as 3.4000000000000004 from slider ends. */
const roundStep = (v: number) => Number(v.toPrecision(12));

/** Selects the field's text once React has shown the applied value, ready for the next entry. */
function selectAfterRender(input: HTMLInputElement) {
  requestAnimationFrame(() => {
    if (document.activeElement === input) input.select();
  });
}

/** Up to 3 decimals without trailing zeros: 1, 0, -0.577. */
function formatComponent(v: number): string {
  const s = String(Number(v.toFixed(3)));
  return s === "-0" ? "0" : s;
}

interface NumberFieldProps {
  className?: string;
  label: string;
  value: number;
  format(v: number): string;
  /**
   * Characters the input is sized for. Values clamped to a range are never longer than the longer
   * of its two ends, so the field fits every value and keeps its width while the slider moves.
   */
  chars?: number;
  unit?: string;
  disabled?: boolean;
  onCommit(v: number): void;
}

/** A number typed as text: applied on Enter or blur, Esc restores the current value, bad input is dropped. */
function NumberField({ className, label, value, format, chars, unit, disabled, onCommit }: NumberFieldProps) {
  const [draft, setDraft] = useState<string | null>(null);

  function commit() {
    if (draft === null) return;
    setDraft(null);
    const v = parseNumber(draft);
    if (v !== null) onCommit(v);
  }

  return (
    <label className={`clip-field${className ? ` ${className}` : ""}`}>
      <input
        className="clip-input"
        type="text"
        inputMode="decimal"
        spellCheck={false}
        autoComplete="off"
        aria-label={label}
        disabled={disabled}
        style={chars ? { width: `calc(${chars}ch + 2px)` } : undefined}
        value={draft ?? format(value)}
        onFocus={(e) => e.currentTarget.select()}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            commit();
            selectAfterRender(e.currentTarget);
          } else if (e.key === "Escape" && draft !== null) {
            e.preventDefault();
            setDraft(null);
          }
        }}
      />
      {unit && (
        <span className="clip-field-unit" aria-hidden="true">
          {unit}
        </span>
      )}
    </label>
  );
}
