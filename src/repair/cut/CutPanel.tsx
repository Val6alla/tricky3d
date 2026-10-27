// Cut surfaces' part of the right panel, in place of the repair tabs while the cutting mode is on: the points, the
// line's settings and the insert order, and Execute cut and Close fixed at the bottom. The points are clicked in the
// viewport; src/repair/cut/session.ts does the rest.
import { useId } from "react";
import { ChevronLeft, ChevronRight, RefreshCw, Scissors, Trash, Undo2, X } from "lucide-react";
import { NumberField } from "../panel/controls";
import { useRepair } from "../store";
import {
  brokenSpans,
  closeCutMode,
  deleteAllPoints,
  executeCut,
  routeCut,
  setCutSettings,
  stepCurrentPoint,
  undoLastPoint,
} from "./session";
import { pathStale, useCut, type InsertOrder } from "./store";
import "../repair.css";
import "./cut.css";

/** A length as the fields show it: two decimals, "2.00". */
export function formatLength(v: number): string {
  return v.toFixed(2);
}

/** The line under the point buttons: how many points, and which is current. */
export function pointsNote(count: number, current: number): string {
  if (count === 0) return "No points yet. Click on the surface to add one.";
  const which = current >= 0 ? `point ${current + 1} of ${count} is current` : `${count} points`;
  return count === 1 ? `1 point (${which}). Click another to draw a line.` : `${count} points, ${which}.`;
}

const ORDERS: { id: InsertOrder; label: string; hint: string }[] = [
  { id: "nearest", label: "At nearest position", hint: "A new point goes into the line where it lengthens it least." },
  { id: "last", label: "At last position", hint: "A new point goes right after the current point." },
];

export function CutPanel() {
  const cut = useCut();
  const { current, settings, routing, routeError } = cut;
  const count = cut.points.length;
  const stale = pathStale(cut);
  const broken = brokenSpans(cut);
  const busy = useRepair((s) => s.busy !== null);
  const stepId = useId();
  const snapId = useId();
  const orderName = useId();
  const closes = settings.closed && count >= 3;

  return (
    <div className="rp cp">
      <div className="rp-panel rp-body">
        <div className="rp-group">
          <h3 className="rp-title">Cut surfaces</h3>
          <p className="rpanel-muted">Click points on the surface. The line between them follows the surface; Execute cut cuts it open along the line.</p>
          <p className="cp-note" aria-live="polite">
            {pointsNote(count, current)}
          </p>
          <div className="cp-buttons">
            <button type="button" className="clip-btn" disabled={count < 2} title="Make the point before the current one current" onClick={() => stepCurrentPoint(-1)}>
              <ChevronLeft size={14} aria-hidden="true" />
              Previous point
            </button>
            <button type="button" className="clip-btn" disabled={count < 2} title="Make the point after the current one current" onClick={() => stepCurrentPoint(1)}>
              Next point
              <ChevronRight size={14} aria-hidden="true" />
            </button>
            <button type="button" className="clip-btn" disabled={count === 0} title="Take out the point added last" onClick={undoLastPoint}>
              <Undo2 size={14} aria-hidden="true" />
              Undo last point
            </button>
            <button type="button" className="clip-btn" disabled={count === 0} title="Start the line again" onClick={deleteAllPoints}>
              <Trash size={14} aria-hidden="true" />
              Delete all points
            </button>
          </div>
        </div>

        <div className="rp-group">
          <h3 className="rp-title">Line</h3>
          <div className="rp-checks">
            <div className="cp-setting">
              <label className="clip-check">
                <input type="checkbox" checked={settings.smooth} onChange={(e) => setCutSettings({ smooth: e.target.checked })} />
                Smooth cut
              </label>
              <span className={`cp-field${settings.smooth ? "" : " is-off"}`}>
                <label htmlFor={stepId}>Smooth step length</label>
                <NumberField
                  id={stepId}
                  value={settings.step}
                  format={formatLength}
                  unit="mm"
                  chars={6}
                  disabled={!settings.smooth}
                  onCommit={(v) => v >= 0.01 && setCutSettings({ step: v })}
                />
              </span>
            </div>
            <div className="cp-setting">
              <label htmlFor={snapId}>Snap distance</label>
              <span className="cp-field">
                <NumberField id={snapId} value={settings.snap} format={formatLength} unit="mm" chars={6} onCommit={(v) => v >= 0 && setCutSettings({ snap: v })} />
              </span>
            </div>
            <label className="clip-check" title="Join the last point back to the first (from three points on)">
              <input type="checkbox" checked={settings.closed} onChange={(e) => setCutSettings({ closed: e.target.checked })} />
              Close line
            </label>
            <div className="cp-setting">
              <label className="clip-check" title="Draw the line again after every change">
                <input type="checkbox" checked={settings.auto} onChange={(e) => setCutSettings({ auto: e.target.checked })} />
                Automatic update
              </label>
              <button
                type="button"
                className="clip-btn cp-update"
                disabled={settings.auto || count < 2 || !stale}
                title="Draw the line through the points as they are"
                onClick={() => void routeCut()}
              >
                <RefreshCw size={13} aria-hidden="true" />
                Update
              </button>
            </div>
          </div>
        </div>

        <div className="rp-group" role="radiogroup" aria-labelledby={`${orderName}-title`}>
          <h3 id={`${orderName}-title`} className="rp-title">
            Order of new inserted point
          </h3>
          <div className="rp-checks">
            {ORDERS.map((o) => (
              <label key={o.id} className="clip-check" title={o.hint}>
                <input type="radio" name={orderName} checked={settings.order === o.id} onChange={() => setCutSettings({ order: o.id })} />
                {o.label}
              </label>
            ))}
          </div>
        </div>

        {(routing || stale) && count >= 2 && (
          <p className="rpanel-muted" aria-live="polite">
            {routing ? "Drawing the line…" : "The line shown is out of date. Click Update."}
          </p>
        )}
        {broken.length > 0 && (
          <p className="cp-problem" role="alert">
            The line cannot follow the surface between {broken.join(", ")}: an open edge is in the way, or the points lie on
            parts of the surface that do not meet. Move or delete one of them.
          </p>
        )}
        {routeError && (
          <p className="cp-problem" role="alert">
            {routeError}
          </p>
        )}
        {closes && <p className="rpanel-muted">The line is closed: after the cut, what it encloses is a shell of its own (Select shell picks it).</p>}
      </div>
      <div className="rp-footer">
        <button
          type="button"
          className="clip-btn clip-btn-primary"
          disabled={busy || count < 2 || broken.length > 0}
          title="Cut the surface open along the line"
          onClick={() => void executeCut()}
        >
          <Scissors size={14} aria-hidden="true" />
          Execute cut
        </button>
        <button type="button" className="clip-btn" disabled={busy} title="Leave Cut surfaces; the points go" onClick={closeCutMode}>
          <X size={14} aria-hidden="true" />
          Close
        </button>
      </div>
    </div>
  );
}
