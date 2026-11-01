// The progress box of a mesh tool run, over everything: shown once the run has taken BUSY_DELAY_MS, with Cancel, which
// stops it in Rust at once; nothing changes then.
import { useEffect, useState } from "react";
import { BusyBox } from "../arrange/ArrangeBusy";
import { BUSY_DELAY_MS } from "../ops/meshOps";
import { cancelModify, useModify } from "./session";

export function ModifyBusy() {
  const busy = useModify((m) => m.busy);
  const job = busy?.job ?? null;
  const [shown, setShown] = useState<number | null>(null);
  useEffect(() => {
    if (job === null) return;
    const timer = window.setTimeout(() => setShown(job), BUSY_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [job]);
  if (!busy || shown !== busy.job) return null;
  const label = busy.stage ? `${busy.label}: ${busy.stage}` : busy.label;
  return <BusyBox label={label} fraction={busy.fraction} cancelling={busy.cancelling} onCancel={cancelModify} />;
}
