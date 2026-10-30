// The progress box of an Arrange run (Align, Lay flat, Rotate to smallest box, Orient, Duplicate, Pack), over
// everything, dialogs included. It shows once a run has taken BUSY_DELAY_MS, so quick runs do not flash it, and its
// Cancel stops the run in Rust at once; nothing changes then.
import { useEffect, useState } from "react";
import { BUSY_DELAY_MS } from "../ops/meshOps";
import "../dialogs/dialogs.css";
import "./arrange.css";
import { cancelArrange, useArrange } from "./session";

export function ArrangeBusy() {
  const busy = useArrange((a) => a.busy);
  const job = busy?.job ?? null;
  const [shown, setShown] = useState<number | null>(null);
  useEffect(() => {
    if (job === null) return;
    const timer = window.setTimeout(() => setShown(job), BUSY_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [job]);
  if (!busy || shown !== busy.job) return null;
  return <BusyBox label={busy.label} fraction={busy.fraction} cancelling={busy.cancelling} />;
}

export function BusyBox({ label, fraction, cancelling }: { label: string; fraction: number; cancelling: boolean }) {
  const percent = Math.round(Math.min(1, Math.max(0, fraction)) * 100);
  return (
    <div className="dlg-backdrop arr-busy-backdrop">
      <div className="dlg arr-busy" role="dialog" aria-modal="true" aria-label={label}>
        <h2 className="dlg-title">{label}</h2>
        <div className="dlg-body" role="status">
          <div className="arr-progress" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent}>
            <div className="arr-progress-fill" style={{ width: `${percent}%` }} />
          </div>
          <p className="dlg-note">Cancel stops it at once and leaves the project as it was.</p>
        </div>
        <div className="dlg-footer">
          <button type="button" className="dlg-btn dlg-btn-primary" disabled={cancelling} onClick={cancelArrange}>
            {cancelling ? "Cancelling…" : "Cancel"}
          </button>
        </div>
      </div>
    </div>
  );
}
