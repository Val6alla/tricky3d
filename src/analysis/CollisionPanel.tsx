// The Collision detection panel, laid out as the reference's: the status and Update on top, Settings and Tolerance
// tabs, a large tick or cross with the pairs found, shortcuts to the interlocking test and packing, and Close.
import { CircleCheck, CircleX, LoaderCircle } from "lucide-react";
import { openPack } from "../arrange/pack";
import { formatFixed2 } from "../info";
import { useStore } from "../store";
import { SliderNumber } from "./AnalysisDialog";
import {
  closeCollisions,
  interlockingTest,
  MAX_MOVE_STEP,
  nudgeSelected,
  setCollisionSettings,
  updateCollisions,
  useCollisions,
  type CollisionSettings,
} from "./collisions";
import "./analysis.css";

/** "No collisions", "1 collision", "2 collisions, 1 pair too close". */
export function collisionStatus(pairs: { kind: "collide" | "near" }[]): string {
  const hits = pairs.filter((p) => p.kind === "collide").length;
  const near = pairs.length - hits;
  const parts = [hits === 0 && near === 0 ? "No collisions" : hits === 1 ? "1 collision" : hits > 1 ? `${hits} collisions` : ""];
  if (near > 0) parts.push(near === 1 ? "1 pair too close" : `${near} pairs too close`);
  return parts.filter(Boolean).join(", ");
}

export function CollisionPanel() {
  const state = useCollisions();
  const parts = useStore((s) => s.parts);
  const nameOf = (id: string) => parts.find((p) => p.id === id)?.name ?? "a part";
  const { settings, result, running, tab } = state;
  const pairs = result?.pairs ?? [];
  const clear = pairs.length === 0;
  const hasSelection = useStore((s) => s.selection.length > 0);
  const check = (key: keyof Omit<CollisionSettings, "minDistance" | "moveStep">, label: string, help: string) => (
    <label className="dlg-check" title={help}>
      <input type="checkbox" checked={settings[key]} onChange={(e) => setCollisionSettings({ [key]: e.target.checked })} />
      <span className="dlg-check-label">{label}</span>
    </label>
  );
  return (
    <section className="an-panel" aria-label="Collision detection">
      <div className="an-body">
        <div className="an-row an-status-row">
          <span className={`an-status ${result && !state.outdated ? (clear ? "is-pass" : "is-fail") : ""}`} role="status">
            {running !== null ? "Checking…" : result ? collisionStatus(pairs) : "Not checked yet"}
            {state.outdated && running === null && result ? " (parts changed since)" : ""}
          </span>
          <button type="button" className="dlg-btn" onClick={() => void updateCollisions()} title="Check the parts again now.">
            Update
          </button>
        </div>
        <div className="ms-tabs an-tabs" role="tablist">
          {(["settings", "tolerance"] as const).map((t) => (
            <button key={t} type="button" role="tab" aria-selected={tab === t} className={`ms-tab${tab === t ? " is-selected" : ""}`} onClick={() => useCollisions.setState({ tab: t })}>
              {t === "settings" ? "Settings" : "Tolerance"}
            </button>
          ))}
        </div>
        {tab === "settings" ? (
          <div className="an-checks">
            {check("autoUpdate", "Auto-update", "Check again whenever a part moves, turns or is scaled.")}
            {check("whileDragging", "Update while dragging", "Also check while a part is being dragged, not only when it is dropped.")}
            {check("display", "Display collisions", "Draw where parts cross in the view; otherwise only the ticks and crosses show.")}
            {check("checkHidden", "Check hidden parts", "Include hidden parts in the check.")}
            <div className="an-move" role="group" aria-label="Move by">
              <SliderNumber
                label="Move by"
                value={settings.moveStep}
                min={0}
                max={MAX_MOVE_STEP}
                sliderMax={10}
                step={0.1}
                unit="mm"
                onChange={(v) => setCollisionSettings({ moveStep: v })}
              />
              <div className="an-move-buttons">
                {([0, 1, 2] as const).flatMap((axis) =>
                  ([-1, 1] as const).map((sign) => {
                    const name = `${"XYZ"[axis]}${sign > 0 ? "+" : "−"}`;
                    return (
                      <button
                        key={name}
                        type="button"
                        className="dlg-btn"
                        disabled={!hasSelection}
                        onClick={() => nudgeSelected(axis, sign)}
                        title={`Move the selected parts ${settings.moveStep} mm along ${sign > 0 ? "+" : "−"}${"XYZ"[axis]}.`}
                      >
                        {name}
                      </button>
                    );
                  }),
                )}
              </div>
            </div>
          </div>
        ) : (
          <div className="an-checks">
            <SliderNumber label="Minimum distance" value={settings.minDistance} min={0} max={100} sliderMax={6} step={0.1} unit="mm" onChange={(v) => setCollisionSettings({ minDistance: v })} />
            <p className="rpanel-muted">Parts nearer to each other than this count as too close. At 0 only parts that touch or cross count.</p>
          </div>
        )}
        <div className={`an-big ${clear ? "is-pass" : "is-fail"}`} aria-hidden="true">
          {running !== null && !result ? <LoaderCircle size={56} /> : clear ? <CircleCheck size={64} /> : <CircleX size={64} />}
        </div>
        {pairs.length > 0 && (
          <ul className="an-pairs" aria-label="Pairs found">
            {pairs.map((p) => (
              <li key={`${p.a}-${p.b}`}>
                <span className="an-pair-names">
                  {nameOf(p.a)} and {nameOf(p.b)}
                </span>
                <span className="an-pair-kind">{p.kind === "collide" ? "collide" : `${formatFixed2(p.distance)} mm apart`}</span>
              </li>
            ))}
          </ul>
        )}
        <div className="an-row">
          <button type="button" className="dlg-btn" onClick={() => void interlockingTest()} title="Test whether the parts can be lifted out upwards one after another.">
            Interlocking test
          </button>
          <button type="button" className="dlg-btn" onClick={() => openPack()} title="Open Packing to arrange the parts apart.">
            Packing…
          </button>
        </div>
      </div>
      <div className="an-actions">
        <button type="button" className="dlg-btn" onClick={closeCollisions}>
          Close
        </button>
      </div>
    </section>
  );
}
