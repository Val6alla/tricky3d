import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent, type ReactNode, type RefObject } from "react";
import { scaleAbout, translationMatrix, type Axis } from "../matrix";
import { centreOf, type Bounds } from "../mesh";
import { useStore, type Part, type TransformMode, type Vec3 } from "../store";
import {
  applyScale,
  boxSize,
  checkScale,
  formatField as format,
  groupBox,
  leftOutParts,
  MAX_MM,
  moveToDelta,
  parseField as parse,
  proportionalSizes,
  refPoint,
  rotateDelta,
  stepField,
  transformParts,
  transformTargets,
  type RefPoint,
  type Targets,
} from "../transformTools";
import { Dialog } from "./Dialog";
import "./transform.css";

const AXES = ["X", "Y", "Z"] as const;
const AXIS_IDS = [0, 1, 2] as const;
const MAX_DEGREES = 360;

/** Text of a number field. `exact` is the unrounded value it was filled with; it counts until the user edits the field. */
interface Field {
  text: string;
  exact?: number;
}

const filled = (v: number): Field => ({ text: format(v), exact: v });
const typed = (text: string): Field => ({ text });

/** The number a field stands for: its exact fill value while untouched, so OK without edits moves nothing by rounding. */
function valueOf(f: Field): number {
  return f.exact ?? parse(f.text);
}

const replaceAt = <T,>(list: T[], i: number, item: T) => list.map((old, j) => (j === i ? item : old));

const inRange = (v: number, min: number, max: number) => Number.isFinite(v) && v >= min && v <= max;

/** Focuses and selects the main number field on open; the Dialog would otherwise pick a select or checkbox above it. */
function useInitialFocus(ref: RefObject<HTMLInputElement | null>) {
  useEffect(() => {
    ref.current?.focus();
    ref.current?.select();
  }, [ref]);
}

interface FormProps {
  /** The parts the dialog was opened for; OK applies to these. */
  ids: readonly string[];
  /** Their combined world box, kept current while the dialog is open. */
  box: Bounds;
  summary: ReactNode;
  onClose(): void;
}

const sameParts = (parts: readonly Part[], ids: readonly string[]) =>
  parts.length === ids.length && parts.every((p) => ids.includes(p.id));

/** Numeric Move / Rotate / Scale for the selection's shown, unlocked parts, as a group about their combined box. */
export function TransformDialog({ mode, onClose }: { mode: TransformMode; onClose(): void }) {
  const parts = useStore((s) => s.parts);
  const selection = useStore((s) => s.selection);
  const targets = useMemo(() => transformTargets({ parts, selection }), [parts, selection]);
  // The parts the dialog opened for. When the selection's movable parts change while it is open (an import
  // selects the new parts, undo restores another selection, a part is hidden, locked or deleted) the dialog
  // closes, so values typed for one selection never land on another.
  const [ids] = useState(() => targets.parts.map((p) => p.id));
  const same = sameParts(targets.parts, ids);
  const box = useMemo(() => (same ? groupBox(targets.parts) : null), [same, targets]);

  useEffect(() => {
    if (!box) onClose();
  }, [box, onClose]);

  if (!box) return null;

  const props: FormProps = { ids, box, summary: <Summary selected={selection.length} targets={targets} />, onClose };
  if (mode === "translate") return <MoveForm {...props} />;
  if (mode === "rotate") return <RotateForm {...props} />;
  return <ScaleForm {...props} />;
}

function Summary({ selected, targets }: { selected: number; targets: Targets }) {
  const parts = selected === 1 ? "1 part selected" : `${selected} parts selected`;
  const out = targets.hidden + targets.locked;
  const left = out === 0 ? "" : ` · ${leftOutParts(targets)} ${out === 1 ? "is" : "are"} left out`;
  return (
    <p className="dlg-muted">
      {parts}
      {left}
    </p>
  );
}

interface Option<T> {
  value: T;
  label: ReactNode;
  className?: string;
}

/** A row of buttons of which exactly one is pressed. */
function Segmented<T extends string | number>(props: { label: string; value: T; options: Option<T>[]; onChange(value: T): void }) {
  return (
    <div className="tf-seg" role="group" aria-label={props.label}>
      {props.options.map((o) => (
        <button
          key={o.value}
          type="button"
          className={o.className ? `tf-seg-btn ${o.className}` : "tf-seg-btn"}
          aria-pressed={o.value === props.value}
          onClick={() => props.onChange(o.value)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

interface NumberInputProps {
  /** Accessible name; leave it out when a <label htmlFor={id}> names the field. */
  label?: string;
  id?: string;
  value: string;
  onChange(text: string): void;
  invalid?: boolean;
  errorId?: string;
  disabled?: boolean;
  inputRef?: RefObject<HTMLInputElement | null>;
  className?: string;
}

/** A number typed as text (see parseField): Up / Down step it by 1, with Shift by 10. */
function NumberInput({ label, id, value, onChange, invalid, errorId, disabled, inputRef, className }: NumberInputProps) {
  function onKeyDown(e: KeyboardEvent<HTMLInputElement>) {
    if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
    const next = stepField(value, (e.key === "ArrowUp" ? 1 : -1) * (e.shiftKey ? 10 : 1));
    if (next === null) return;
    e.preventDefault();
    onChange(next);
  }

  return (
    <input
      ref={inputRef}
      id={id}
      className={className ? `dlg-input tf-num ${className}` : "dlg-input tf-num"}
      type="text"
      inputMode="decimal"
      spellCheck={false}
      autoComplete="off"
      aria-label={label}
      value={value}
      disabled={disabled}
      aria-invalid={invalid || undefined}
      aria-describedby={invalid ? errorId : undefined}
      onChange={(e) => onChange(e.target.value)}
      onKeyDown={onKeyDown}
    />
  );
}

function ErrorText({ id, error }: { id: string; error: string | null }) {
  if (!error) return null;
  return (
    <p id={id} className="dlg-error" role="alert">
      {error}
    </p>
  );
}

// Move -----------------------------------------------------------------------------------------

const MOVE_HELP = {
  min: "Set Z to 0 to put the selection on the platform.",
  centre: "The centre of the selection's box goes to this point.",
  by: "Distances to move the selection along each axis.",
};

function MoveForm({ ids, box, summary, onClose }: FormProps) {
  const [by, setBy] = useState(false);
  const [ref, setRef] = useState<RefPoint>("min");
  const [fields, setFields] = useState(() => refPoint(box, "min").map(filled));
  const [error, setError] = useState<string | null>(null);
  const errorId = useId();
  const first = useRef<HTMLInputElement>(null);
  useInitialFocus(first);

  function switchKind(nextBy: boolean) {
    if (nextBy === by) return;
    setBy(nextBy);
    setFields(nextBy ? [0, 0, 0].map(filled) : refPoint(box, ref).map(filled));
    setError(null);
  }

  function switchRef(next: RefPoint) {
    setRef(next);
    setFields(refPoint(box, next).map(filled));
    setError(null);
  }

  function edit(i: number, text: string) {
    setFields((f) => replaceAt(f, i, typed(text)));
    setError(null);
  }

  function ok() {
    const v = fields.map(valueOf) as Vec3;
    if (!v.every((x) => Number.isFinite(x))) {
      setError("Enter a number for X, Y and Z.");
      return;
    }
    if (v.some((x) => Math.abs(x) > MAX_MM)) {
      setError(`Enter values from -${MAX_MM} to ${MAX_MM} mm.`);
      return;
    }
    transformParts(ids, by ? translationMatrix(v) : moveToDelta(box, ref, v), "Move");
    onClose();
  }

  return (
    <Dialog title="Move" onClose={onClose} onOk={ok}>
      {summary}
      <Segmented
        label="How to move"
        value={by ? "by" : "to"}
        options={[
          { value: "to", label: "Move to position" },
          { value: "by", label: "Move by offset" },
        ]}
        onChange={(v) => switchKind(v === "by")}
      />
      <label className="tf-row">
        <span className="dlg-label">Reference point</span>
        <select className="dlg-input" value={ref} disabled={by} onChange={(e) => switchRef(e.target.value as RefPoint)}>
          <option value="min">Box minimum</option>
          <option value="centre">Box centre</option>
        </select>
      </label>
      <div className="dlg-grid-3">
        {AXES.map((axis, i) => (
          <label key={axis} className="dlg-field">
            <span className="dlg-label">
              {axis} <span className="dlg-unit">mm</span>
            </span>
            <NumberInput
              label={axis}
              value={fields[i].text}
              onChange={(text) => edit(i, text)}
              invalid={!!error && !inRange(valueOf(fields[i]), -MAX_MM, MAX_MM)}
              errorId={errorId}
              inputRef={i === 0 ? first : undefined}
            />
          </label>
        ))}
      </div>
      <p className="dlg-help tf-help">{MOVE_HELP[by ? "by" : ref]}</p>
      <ErrorText id={errorId} error={error} />
    </Dialog>
  );
}

// Rotate ---------------------------------------------------------------------------------------

type Pivot = "centre" | "origin";

const QUICK_ANGLES = [
  { degrees: -90, label: "−90°" },
  { degrees: 90, label: "+90°" },
  { degrees: 180, label: "180°" },
];

function RotateForm({ ids, box, summary, onClose }: FormProps) {
  const [axis, setAxis] = useState<Axis>(2);
  const [angle, setAngle] = useState("90");
  const [pivot, setPivot] = useState<Pivot>("centre");
  const [error, setError] = useState<string | null>(null);
  const errorId = useId();
  const angleId = useId();
  const angleRef = useRef<HTMLInputElement>(null);
  useInitialFocus(angleRef);

  function editAngle(text: string) {
    setAngle(text);
    setError(null);
  }

  function ok() {
    const degrees = parse(angle);
    if (!Number.isFinite(degrees) || Math.abs(degrees) > MAX_DEGREES) {
      setError(`Enter an angle from -${MAX_DEGREES} to ${MAX_DEGREES}°.`);
      return;
    }
    const centre: Vec3 = pivot === "centre" ? centreOf(box) : [0, 0, 0];
    transformParts(ids, rotateDelta(axis, degrees, centre), "Rotate");
    onClose();
  }

  return (
    <Dialog title="Rotate" onClose={onClose} onOk={ok}>
      {summary}
      <div className="tf-row">
        <span className="dlg-label">Axis</span>
        <Segmented<Axis>
          label="Axis"
          value={axis}
          options={AXIS_IDS.map((i) => ({ value: i, label: AXES[i], className: `tf-axis tf-axis-${AXES[i].toLowerCase()}` }))}
          onChange={setAxis}
        />
      </div>
      <div className="tf-row">
        <label className="dlg-label" htmlFor={angleId}>
          Angle
        </label>
        <div className="tf-inline">
          <NumberInput
            id={angleId}
            inputRef={angleRef}
            className="tf-angle"
            value={angle}
            onChange={editAngle}
            invalid={!!error}
            errorId={errorId}
          />
          <span className="dlg-unit">°</span>
          <span className="tf-chips">
            {QUICK_ANGLES.map((q) => (
              <button
                key={q.degrees}
                type="button"
                className="tf-chip"
                aria-label={`${q.degrees} degrees`}
                onClick={() => {
                  editAngle(String(q.degrees));
                  angleRef.current?.focus();
                }}
              >
                {q.label}
              </button>
            ))}
          </span>
        </div>
      </div>
      <label className="tf-row">
        <span className="dlg-label">Pivot</span>
        <select className="dlg-input" value={pivot} onChange={(e) => setPivot(e.target.value as Pivot)}>
          <option value="centre">Selection centre</option>
          <option value="origin">Origin (0, 0, 0)</option>
        </select>
      </label>
      <p className="dlg-help tf-help">Positive angles turn counter-clockwise, seen from the +{AXES[axis]} side.</p>
      <ErrorText id={errorId} error={error} />
    </Dialog>
  );
}

// Scale ----------------------------------------------------------------------------------------

/** First axis with a size; a flat part cannot be sized along the axis it is flat in. */
function firstSized(size: Vec3): Axis {
  return AXIS_IDS.find((i) => size[i] > 0) ?? 0;
}

interface Problem {
  error: string;
  /** Axes whose field is marked invalid. */
  bad: readonly Axis[];
}

function ScaleForm({ ids, box, summary, onClose }: FormProps) {
  const current = boxSize(box);
  const [bySize, setBySize] = useState(false);
  const [keep, setKeep] = useState(true);
  const [percent, setPercent] = useState(() => [100, 100, 100].map(filled));
  const [size, setSize] = useState(() => boxSize(box).map(filled));
  /** The field edited last; with "Keep proportions" it sets the others. */
  const [driver, setDriver] = useState<Axis>(() => firstSized(boxSize(box)));
  const [problem, setProblem] = useState<Problem | null>(null);
  const errorId = useId();
  const first = useRef<HTMLInputElement>(null);
  useInitialFocus(first);

  const check = checkScale({ bySize, keep, percent: percent.map(valueOf), size: size.map(valueOf), driver, current });
  /** Shown live, also when out of range, so the user sees why OK refuses. */
  const factors = check.factors;

  /** The other sizes that keep the box's proportions when axis `i` is set to `value` mm. */
  function proportional(sizes: Field[], i: Axis, value: number): Field[] {
    const next = proportionalSizes(current, i, value);
    return next ? sizes.map((f, j) => (j === i ? f : filled(next[j]))) : sizes;
  }

  function editPercent(i: Axis, text: string) {
    setDriver(i);
    setPercent((p) => (keep ? [typed(text), typed(text), typed(text)] : replaceAt(p, i, typed(text))));
    setProblem(null);
  }

  function editSize(i: Axis, text: string) {
    setDriver(i);
    setSize((s) => {
      const next = replaceAt(s, i, typed(text));
      return keep ? proportional(next, i, parse(text)) : next;
    });
    setProblem(null);
  }

  function switchKeep(next: boolean) {
    setKeep(next);
    setProblem(null);
    if (!next) return;
    // Turning the lock on lines the other fields up with the one edited last.
    if (bySize) setSize((s) => proportional(s, driver, valueOf(s[driver])));
    else setPercent((p) => [p[driver], p[driver], p[driver]]);
  }

  /** Switching between percentage and size carries the current result over when it is valid. */
  function switchBy(next: boolean) {
    if (next === bySize) return;
    const carried = check.error ? null : factors;
    setBySize(next);
    setProblem(null);
    if (next) {
      setSize(current.map((c, i) => filled(c * (carried ? carried[i] : 1))));
      if (!(current[driver] > 0)) setDriver(firstSized(current));
    } else {
      setPercent((carried ?? [1, 1, 1]).map((f) => filled(f * 100)));
    }
  }

  function ok() {
    if (check.error !== null || !factors) {
      setProblem({ error: check.error ?? "", bad: check.bad });
      return;
    }
    // One applied scale also ends Scale mode, as a scale gizmo drag does; a refused one or Cancel leaves it on.
    if (!applyScale(ids, scaleAbout(factors, centreOf(box)))) {
      setProblem({ error: "That scale cannot be applied. Try a value closer to 100 %.", bad: [] });
      return;
    }
    onClose();
  }

  const invalid = (i: Axis) => !!problem && problem.bad.includes(i);

  const unit = (text: string) => <span className="dlg-unit">{text}</span>;
  const firstInput = (i: Axis) => (i === firstSized(current) ? first : undefined);

  return (
    <Dialog title="Scale" onClose={onClose} onOk={ok}>
      {summary}
      <Segmented
        label="How to scale"
        value={bySize ? "size" : "percent"}
        options={[
          { value: "percent", label: "By percentage" },
          { value: "size", label: "To size" },
        ]}
        onChange={(v) => switchBy(v === "size")}
      />
      <label className="dlg-check">
        <input type="checkbox" checked={keep} onChange={(e) => switchKeep(e.target.checked)} />
        <span className="dlg-check-label">Keep proportions</span>
      </label>
      <div className="tf-table">
        <span />
        {AXES.map((a) => (
          <span key={a} className="tf-th">
            {a}
          </span>
        ))}

        <span className="dlg-label">Scale {unit("%")}</span>
        {AXIS_IDS.map((i) =>
          bySize ? (
            <span key={i} className="tf-val is-result">
              {factors ? format(factors[i] * 100) : "—"}
            </span>
          ) : (
            <NumberInput
              key={i}
              label={`Scale ${AXES[i]} percent`}
              value={percent[i].text}
              onChange={(text) => editPercent(i, text)}
              invalid={invalid(i)}
              errorId={errorId}
              inputRef={i === 0 ? first : undefined}
            />
          ),
        )}

        <span className="dlg-label">Size now {unit("mm")}</span>
        {AXIS_IDS.map((i) => (
          <span key={i} className="tf-val">
            {format(current[i])}
          </span>
        ))}

        <span className="dlg-label">New size {unit("mm")}</span>
        {AXIS_IDS.map((i) =>
          bySize ? (
            <NumberInput
              key={i}
              label={`New size ${AXES[i]} mm`}
              value={size[i].text}
              onChange={(text) => editSize(i, text)}
              invalid={invalid(i)}
              errorId={errorId}
              disabled={!(current[i] > 0)}
              inputRef={firstInput(i)}
            />
          ) : (
            <span key={i} className={invalid(i) ? "tf-val is-result is-bad" : "tf-val is-result"}>
              {factors ? format(current[i] * factors[i]) : "—"}
            </span>
          ),
        )}
      </div>
      <ErrorText id={errorId} error={problem?.error ?? null} />
    </Dialog>
  );
}
