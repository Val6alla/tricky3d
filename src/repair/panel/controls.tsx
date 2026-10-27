// Small controls the repair panel's tabs share: a typed number field and a slider with its number field. They wear
// the Clip planes section's classes (src/layout/clip.css), so the repair tabs look like the rest of the right panel.
import { useId, useRef, useState } from "react";
import { parseNumber } from "../../numbers";
import "../../layout/clip.css";

/** Joins the class names that are not false. */
export const cx = (...names: (string | false | null | undefined)[]) => names.filter(Boolean).join(" ");

/** `value` kept within min..max. */
export const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));

interface NumberFieldProps {
  /** For a <label htmlFor> elsewhere; without one, `label` names the field. */
  id?: string;
  label?: string;
  value: number;
  /** The value as the field shows it while it is not being typed in. */
  format(v: number): string;
  unit?: string;
  /** Characters the input is sized for. */
  chars?: number;
  disabled?: boolean;
  onCommit(v: number): void;
}

/**
 * A number typed as text: applied on Enter or when focus leaves, Esc puts the current value back, and text that is
 * not a number is dropped. A click anywhere in the box, the unit included, goes to the input.
 */
export function NumberField({ id, label, value, format, unit, chars, disabled, onCommit }: NumberFieldProps) {
  const [draft, setDraft] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);

  function commit() {
    if (draft === null) return;
    setDraft(null);
    const v = parseNumber(draft);
    if (v !== null) onCommit(v);
  }

  return (
    <span
      className="clip-field rp-field"
      onMouseDown={(e) => {
        if (e.target === input.current || disabled) return;
        e.preventDefault();
        input.current?.focus();
      }}
    >
      <input
        ref={input}
        id={id}
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
            const el = e.currentTarget;
            // Select the applied value once React has shown it, ready for the next entry.
            requestAnimationFrame(() => {
              if (document.activeElement === el) el.select();
            });
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
    </span>
  );
}

/** Whole degrees, or one decimal when the value has one: "10", "7.5". */
export function formatDegrees(v: number): string {
  return String(Math.round(v * 10) / 10);
}

interface SliderFieldProps {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  unit: string;
  format?(v: number): string;
  disabled?: boolean;
  onChange(v: number): void;
}

/** A named slider over its own line with the same value in a number field; typed values are kept in the slider's range. */
export function SliderField({ label, value, min, max, step = 1, unit, format = formatDegrees, disabled, onChange }: SliderFieldProps) {
  const id = useId();
  return (
    <div className={cx("rp-slider", disabled && "is-off")}>
      <label className="rp-slider-label" htmlFor={id}>
        {label}
      </label>
      <div className="rp-slider-ctrl">
        <input
          type="range"
          className="clip-slider"
          aria-label={label}
          aria-valuetext={`${format(value)}${unit}`}
          min={min}
          max={max}
          step={step}
          value={value}
          disabled={disabled}
          onChange={(e) => onChange(clamp(Number(e.target.value), min, max))}
        />
        <NumberField
          id={id}
          value={value}
          format={format}
          unit={unit}
          chars={Math.max(format(min).length, format(max).length) + 1}
          disabled={disabled}
          onCommit={(v) => onChange(clamp(v, min, max))}
        />
      </div>
    </div>
  );
}
