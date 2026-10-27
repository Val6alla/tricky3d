import { Scissors } from "lucide-react";
import { useClip, type CutScope } from "../clip";
import { runFeature } from "../features";
import "./clip.css";

/** Cuts section: which parts to cut (shared with the Cut dialog's scope) and the Cut button, which opens that dialog. */
export function CutsPanel() {
  const scope = useClip((s) => s.cutScope);
  const setCutScope = useClip((s) => s.setCutScope);

  return (
    <div className="cuts">
      <div className="cuts-row">
        <select
          className="clip-select"
          aria-label="Parts to cut"
          value={scope}
          onChange={(e) => setCutScope(e.target.value as CutScope)}
        >
          <option value="selected">Cut selected parts</option>
          <option value="all">Cut all parts</option>
        </select>
        <button type="button" className="clip-btn clip-btn-primary" onClick={() => runFeature("modify.cut")}>
          <Scissors size={13} aria-hidden="true" />
          Cut
        </button>
      </div>
      <p className="rpanel-muted">Cuts along the enabled clip plane, or a plane you set in the Cut dialog.</p>
    </div>
  );
}
