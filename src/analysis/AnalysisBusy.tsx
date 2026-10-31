// The progress box of an analysis run, over everything. Like the Arrange busy box it shows once a run has taken
// BUSY_DELAY_MS, so quick runs do not flash it, and its Cancel stops the run in Rust at once; nothing changes then.
import { useEffect, useState } from "react";
import { BusyBox } from "../arrange/ArrangeBusy";
import { BUSY_DELAY_MS } from "../ops/meshOps";
import { cancelAnalysis, useAnalysis } from "./session";

export function AnalysisBusy() {
  const busy = useAnalysis((a) => a.busy);
  const job = busy?.job ?? null;
  const [shown, setShown] = useState<number | null>(null);
  useEffect(() => {
    if (job === null) return;
    const timer = window.setTimeout(() => setShown(job), BUSY_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [job]);
  if (!busy || shown !== busy.job) return null;
  return <BusyBox label={busy.label} fraction={busy.fraction} cancelling={busy.cancelling} onCancel={cancelAnalysis} />;
}
