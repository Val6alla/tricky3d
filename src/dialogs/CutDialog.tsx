import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { sceneBounds, useClip, type ClipAxis, type CutScope } from "../clip";
import { useCutPreview } from "../cutPreview";
import type { Bounds } from "../mesh";
import { cutTargets, runCut } from "../ops/meshOps";
import { axisCutPlane, centreAlong, enabledCutPlanes, type AxisName, type PlaneChoice } from "../ops/planes";
import type { CutOptions, CutPlane } from "../ops/types";
import { useStore } from "../store";
import { formatField, groupBox, MAX_MM, parseField, stepField } from "../transformTools";
import { Dialog } from "./Dialog";
import "./transform.css";

const AXES: { value: AxisName; label: string }[] = [
  { value: "x", label: "X" },
  { value: "y", label: "Y" },
  { value: "z", label: "Z" },
];

/** What the dialog's fields hold. */
export interface CutFields {
  /** The clip plane picked in the list; only read while clip planes are on. */
  picked: ClipAxis | null;
  axis: AxisName;
  /** Position field text, or null while untouched: the centre of the box on `axis`, unrounded, so it follows the scope. */
  position: string | null;
  cap: boolean;
  separate: boolean;
  /** "Separate cut parts by" distance text. */
  gap: string;
}

export const defaultCutFields = (planes: readonly PlaneChoice[]): CutFields => ({
  // One plane on is the plane to cut with; with several the user has to say which.
  picked: planes.length === 1 ? planes[0].axis : null,
  axis: "z",
  position: null,
  cap: true,
  separate: false,
  gap: "5",
});

export interface CutCheck {
  /** What Execute runs; null keeps it disabled. */
  options: CutOptions | null;
  /** The plane to preview, also while a field below it is wrong. */
  plane: CutPlane | null;
  /** Why Execute is disabled. Null when it is enabled, or when the plane list's own note says why. */
  problem: string | null;
  /** The field the problem is in. */
  bad: "position" | "gap" | null;
}

/** The Position field as shown: what was typed, else the box centre on the axis. */
export const positionText = (f: CutFields, box: Bounds) => f.position ?? formatField(centreAlong(f.axis, box));

/**
 * What Execute would run, or why it cannot. `planes` are the enabled clip planes: with any on, the cut
 * uses the one picked; with none, the axis and position. `targets` is how many parts the scope gives.
 */
export function checkCut(f: CutFields, planes: readonly PlaneChoice[], box: Bounds, scope: CutScope, targets: number): CutCheck {
  const fail = (problem: string, bad: CutCheck["bad"], plane: CutPlane | null = null): CutCheck => ({ options: null, plane, problem, bad });
  let plane: CutPlane | null;
  if (planes.length) {
    plane = planes.find((p) => p.axis === f.picked)?.plane ?? null;
  } else {
    const position = f.position === null ? centreAlong(f.axis, box) : parseField(f.position);
    if (!Number.isFinite(position)) return fail("Enter a number for the position.", "position");
    if (Math.abs(position) > MAX_MM) return fail(`Enter a position from -${MAX_MM} to ${MAX_MM} mm.`, "position");
    plane = axisCutPlane(f.axis, position);
  }
  const gap = f.separate ? parseField(f.gap) : 0;
  if (f.separate && !(gap > 0 && gap <= MAX_MM)) return fail(`Enter a distance greater than 0 and up to ${MAX_MM} mm.`, "gap", plane);
  if (targets === 0) {
    const why = scope === "selected" ? "Select a shown, unlocked part to cut." : "There are no shown, unlocked parts to cut.";
    return fail(why, null, plane);
  }
  if (!plane) return { options: null, plane, problem: null, bad: null };
  return { options: { plane, scope, cap: f.cap, gap }, plane, problem: null, bad: null };
}

/** Cut button and the Cuts panel's Cut: the plane, which parts, capping and separation. Execute runs the cut. */
export function CutDialog({ onClose }: { onClose(): void }) {
  const parts = useStore((s) => s.parts);
  const selection = useStore((s) => s.selection);
  const platform = useStore((s) => s.platform);
  const rows = useClip((s) => s.rows);
  const customNormal = useClip((s) => s.customNormal);
  const scope = useClip((s) => s.cutScope);
  // The clip panel's range box, so each listed plane sits where its slider shows it.
  const scene = useMemo(() => sceneBounds(platform, parts), [platform, parts]);
  const planes = useMemo(() => enabledCutPlanes({ rows, customNormal }, scene), [rows, customNormal, scene]);
  // cutTargets reads the store itself; parts and selection are listed so the targets follow it.
  const targets = useMemo(() => cutTargets(scope), [scope, parts, selection]);
  const box = useMemo(() => groupBox(targets) ?? scene, [targets, scene]);
  return <CutForm planes={planes} box={box} scope={scope} targets={targets.length} onClose={onClose} />;
}

export interface CutFormProps {
  /** The enabled clip planes; with none the form asks for an axis and a position. */
  planes: readonly PlaneChoice[];
  /** World box of the parts the cut acts on, else the scene: the default position and the preview's extent. */
  box: Bounds;
  scope: CutScope;
  /** How many parts the cut would act on. */
  targets: number;
  onClose(): void;
}

export function CutForm({ planes, box, scope, targets, onClose }: CutFormProps) {
  const [fields, setFields] = useState(() => defaultCutFields(planes));
  const check = checkCut(fields, planes, box, scope, targets);
  const titleId = useId();
  const radioName = useId();
  const pickNoteId = useId();
  const positionId = useId();
  const capHelpId = useId();
  const problemId = useId();
  const positionRef = useRef<HTMLInputElement>(null);

  const edit = (patch: Partial<CutFields>) => setFields((f) => ({ ...f, ...patch }));

  // The position is what one usually changes; the Dialog would focus the Axis select above it.
  useEffect(() => {
    positionRef.current?.focus();
    positionRef.current?.select();
  }, []);

  // The viewport previews the plane while the dialog is open, and drops it however the dialog closes.
  // Keyed on the plane's numbers (planeKey stands for plane): checkCut builds a new object on every render.
  const plane = check.plane;
  const planeKey = plane ? `${plane.normal.join()} ${plane.offset}` : "";
  useEffect(() => {
    useCutPreview.getState().setPlane(plane, box);
    return () => useCutPreview.getState().setPlane(null);
  }, [planeKey, box]);

  function execute() {
    if (!check.options) return;
    onClose();
    void runCut(check.options);
  }

  /** Up / Down step the position by 1 mm, with Shift by 10, moving the preview. */
  function stepPosition(e: KeyboardEvent<HTMLInputElement>) {
    if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
    const next = stepField(e.currentTarget.value, (e.key === "ArrowUp" ? 1 : -1) * (e.shiftKey ? 10 : 1));
    if (next === null) return;
    e.preventDefault();
    edit({ position: next });
  }

  const describe = (field: CutCheck["bad"]) => (check.bad === field ? problemId : undefined);

  return (
    <Dialog
      title="Cut"
      onClose={onClose}
      onOk={execute}
      okLabel="Execute"
      okDisabled={!check.options}
      className="dlg-cut"
      backdropClassName="dlg-backdrop-aside"
    >
      <section className="dlg-section" aria-labelledby={titleId}>
        <h3 id={titleId} className="dlg-section-title">
          Plane
        </h3>
        {planes.length > 0 ? (
          <>
            {planes.length > 1 && (
              <p id={pickNoteId} className="dlg-note">
                Several clip planes are on. Pick the one to cut with.
              </p>
            )}
            <div className="dlg-choices" role="radiogroup" aria-labelledby={titleId} aria-describedby={planes.length > 1 ? pickNoteId : undefined}>
              {planes.map((p) => (
                <label key={p.axis} className="dlg-check">
                  <input type="radio" name={radioName} checked={fields.picked === p.axis} onChange={() => edit({ picked: p.axis })} />
                  <span className="dlg-check-label">{p.label}</span>
                </label>
              ))}
            </div>
          </>
        ) : (
          <>
            <label className="tf-row">
              <span className="dlg-label">Axis</span>
              <select
                className="dlg-input dlg-select"
                value={fields.axis}
                // A new axis starts at the centre of the parts along it.
                onChange={(e) => edit({ axis: AXES.find((a) => a.value === e.target.value)?.value ?? "z", position: null })}
              >
                {AXES.map((a) => (
                  <option key={a.value} value={a.value}>
                    {a.label}
                  </option>
                ))}
              </select>
            </label>
            <div className="tf-row">
              <label className="dlg-label" htmlFor={positionId}>
                Position <span className="dlg-unit">mm</span>
              </label>
              <input
                ref={positionRef}
                id={positionId}
                className="dlg-input tf-num"
                type="text"
                inputMode="decimal"
                spellCheck={false}
                autoComplete="off"
                value={positionText(fields, box)}
                aria-invalid={check.bad === "position" || undefined}
                aria-describedby={describe("position")}
                onChange={(e) => edit({ position: e.target.value })}
                onKeyDown={stepPosition}
              />
            </div>
          </>
        )}
      </section>
      <label className="tf-row">
        <span className="dlg-label">Scope</span>
        <select
          className="dlg-input dlg-select"
          value={scope}
          onChange={(e) => useClip.getState().setCutScope(e.target.value === "all" ? "all" : "selected")}
        >
          <option value="selected">Cut selected parts</option>
          <option value="all">Cut all parts</option>
        </select>
      </label>
      <label className="dlg-check">
        <input type="checkbox" checked={fields.cap} aria-describedby={capHelpId} onChange={(e) => edit({ cap: e.target.checked })} />
        <span>
          <span className="dlg-check-label">Close cut surfaces</span>
          <span id={capHelpId} className="dlg-help">
            Fill the cut so each piece is a closed solid.
          </span>
        </span>
      </label>
      <div className="dlg-cut-gap">
        <label className="dlg-check">
          <input type="checkbox" checked={fields.separate} onChange={(e) => edit({ separate: e.target.checked })} />
          <span className="dlg-check-label">Separate cut parts by</span>
        </label>
        <input
          className="dlg-input tf-num"
          type="text"
          inputMode="decimal"
          spellCheck={false}
          autoComplete="off"
          aria-label="Separation in mm"
          value={fields.gap}
          disabled={!fields.separate}
          aria-invalid={check.bad === "gap" || undefined}
          aria-describedby={describe("gap")}
          onChange={(e) => edit({ gap: e.target.value })}
        />
        <span className="dlg-unit">mm</span>
      </div>
      {check.problem &&
        (check.bad ? (
          <p id={problemId} className="dlg-error" role="alert">
            {check.problem}
          </p>
        ) : (
          <p id={problemId} className="dlg-note" role="status">
            {check.problem}
          </p>
        ))}
    </Dialog>
  );
}
