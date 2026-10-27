// Repair mode › the progress of a run: the run's name, the action running and the step it is on, a progress bar and
// Cancel.
// src/repair/session.ts opens it once a run has taken BUSY_DELAY_MS and closes it when the run ends. Unlike the
// other dialogs it cannot be dismissed: Esc and a click outside do nothing, because the run goes on until it ends
// or Cancel stops it (which leaves the working copy exactly as it was). That is why it draws the shared dialog
// frame's classes itself instead of using src/dialogs/Dialog.tsx, whose Esc, Enter and backdrop all close.
import { useEffect, useId, useRef, type KeyboardEvent } from "react";
import { useUi } from "../../ui";
import { cancelRun } from "../session";
import { useRepair } from "../store";
import type { RepairBusy } from "../types";
import "../../dialogs/dialogs.css";
import "./dialogs.css";

/** The bar's whole percentage (0 to 100), or null while the run has not reported any progress. */
export function progressPercent(progress: number | null): number | null {
  if (progress === null || !Number.isFinite(progress)) return null;
  return Math.round(Math.min(1, Math.max(0, progress)) * 100);
}

/** The line under the title: the action running ("Wrap part surface (9 of 11)"), or what the run is doing. */
export function progressStep(busy: RepairBusy): string {
  if (busy.cancelling) return "Cancelling…";
  return busy.step ?? "Working…";
}

/**
 * The line under it, while the run goes on: the step inside the action and how far the action itself is ("Finding
 * intersections · 40%"); the bar shares the whole run between its actions, so a long one moves it slowly.
 */
export function progressDetail(busy: RepairBusy): string | null {
  if (busy.cancelling || !busy.detail) return null;
  const percent = progressPercent(busy.local);
  return percent === null ? busy.detail : `${busy.detail} · ${percent}%`;
}

/**
 * Keys in the progress dialog: none reaches the app's shortcuts, Esc does nothing, and Tab stays put, since Cancel
 * is the only control. Enter and Space on Cancel press it as usual.
 */
export function onProgressKeyDown(e: Pick<KeyboardEvent, "key" | "preventDefault" | "stopPropagation">): void {
  e.stopPropagation();
  if (e.key === "Escape" || e.key === "Tab") e.preventDefault();
}

export function RunProgressDialog({ onClose }: { onClose(): void }) {
  const busy = useRepair((s) => s.busy);

  // session.ts closes the dialog when the run ends; this covers a run that ended before the dialog drew. The
  // check keeps a dialog opened meanwhile from being closed in its place.
  useEffect(() => {
    if (!busy && useUi.getState().dialog?.kind === "repair-progress") onClose();
  }, [busy, onClose]);

  return busy ? <RunProgressView busy={busy} onCancel={cancelRun} /> : null;
}

export interface RunProgressViewProps {
  busy: RepairBusy;
  onCancel(): void;
}

export function RunProgressView({ busy, onCancel }: RunProgressViewProps) {
  const panelRef = useRef<HTMLDivElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const titleId = useId();
  const stepId = useId();
  const percent = progressPercent(busy.progress);
  const detail = progressDetail(busy);

  // Like the shared Dialog: focus moves in on open and goes back where it was on close.
  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    cancelRef.current?.focus();
    return () => previous?.focus();
  }, []);

  // Cancel is disabled while the run stops, which would drop the focus out of the dialog.
  useEffect(() => {
    if (busy.cancelling) panelRef.current?.focus();
  }, [busy.cancelling]);

  return (
    <div className="dlg-backdrop" onKeyDown={onProgressKeyDown}>
      <div
        ref={panelRef}
        className="dlg dlg-rp-run"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={stepId}
        tabIndex={-1}
      >
        <h2 id={titleId} className="dlg-title">
          {busy.label}
        </h2>
        <div className="dlg-body">
          <div className="rp-run-line">
            <span id={stepId} className="rp-run-step">
              {progressStep(busy)}
            </span>
            {percent !== null && <span className="rp-run-percent">{percent}%</span>}
          </div>
          {detail && <div className="rp-run-detail">{detail}</div>}
          <div
            className={percent === null ? "rp-progress is-indeterminate" : "rp-progress"}
            role="progressbar"
            aria-labelledby={titleId}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={percent ?? undefined}
          >
            <div className="rp-progress-fill" style={percent === null ? undefined : { width: `${percent}%` }} />
          </div>
        </div>
        <div className="dlg-footer">
          <button ref={cancelRef} type="button" className="dlg-btn" disabled={busy.cancelling} onClick={onCancel}>
            Cancel
          </button>
        </div>
      </div>
    </div>
  );
}
