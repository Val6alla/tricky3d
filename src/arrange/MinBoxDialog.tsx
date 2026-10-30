// Arrange › Rotate to smallest box: about Z only (the part keeps standing as it does) or about all axes. OK runs it.
import { useId, useState } from "react";
import { Dialog } from "../dialogs/Dialog";
import { loadSetting, saveSetting } from "../store";
import "./arrange.css";
import { minBox } from "./session";
import type { BoxAxes } from "./types";

export const MIN_BOX_KEY = "tricky3d.minBoxAxes";

export function MinBoxDialog({ onClose }: { onClose(): void }) {
  const [axes, setAxes] = useState<BoxAxes>(() => (loadSetting(MIN_BOX_KEY) === "all" ? "all" : "z"));
  const name = useId();

  function run() {
    saveSetting(MIN_BOX_KEY, axes);
    onClose();
    void minBox(axes);
  }

  return (
    <Dialog title="Rotate to smallest box" onClose={onClose} onOk={run} className="dlg-arrange">
      <div className="dlg-choices" role="radiogroup" aria-label="Rotate">
        <label className="dlg-check">
          <input type="radio" name={name} checked={axes === "z"} onChange={() => setAxes("z")} />
          <span>
            <span className="dlg-check-label">Around Z only</span>
            <span className="dlg-help">The part keeps standing as it does and turns until its footprint's box is smallest.</span>
          </span>
        </label>
        <label className="dlg-check">
          <input type="radio" name={name} checked={axes === "all"} onChange={() => setAxes("all")} />
          <span>
            <span className="dlg-check-label">Around all axes</span>
            <span className="dlg-help">The part turns freely until its box is smallest, longest side along X and shortest along Z.</span>
          </span>
        </label>
      </div>
      <p className="dlg-note">Each selected part turns about its own centre and keeps its height above the platform.</p>
    </Dialog>
  );
}
