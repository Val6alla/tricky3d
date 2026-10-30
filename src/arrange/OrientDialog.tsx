// Home › Orient: pick a goal, search, and the best ways for the part to stand are listed with their scores next to
// how it stands now. Clicking a row turns the part that way in the view; Apply keeps it as one undo step, Cancel puts it
// back as it was.
import { useId, useRef, useState } from "react";
import { Dialog } from "../dialogs/Dialog";
import type { Snapshot } from "../history";
import { loadSetting, saveSetting, useStore } from "../store";
import "./arrange.css";
import { searchOrientations } from "./session";
import type { OrientCandidate, OrientGoal, OrientScores } from "./types";

export const ORIENT_GOAL_KEY = "tricky3d.orientGoal";

export const GOALS: { value: OrientGoal; label: string; help: string }[] = [
  { value: "least-support", label: "Least support area", help: "Fewest surfaces facing down more steeply than 45°." },
  { value: "lowest-height", label: "Lowest height", help: "The shortest build: fewest layers." },
  { value: "smallest-footprint", label: "Smallest footprint", help: "The least room on the platform." },
];

const area = (v: number) => `${v.toFixed(v < 100 ? 1 : 0)}`;
const mm = (v: number) => v.toFixed(2);

function loadGoal(): OrientGoal {
  const saved = loadSetting(ORIENT_GOAL_KEY);
  return GOALS.some((g) => g.value === saved) ? (saved as OrientGoal) : "least-support";
}

export function OrientDialog({ partId, onClose }: { partId: string; onClose(): void }) {
  const part = useStore((s) => s.parts.find((p) => p.id === partId));
  const [goal, setGoal] = useState<OrientGoal>(loadGoal);
  const [found, setFound] = useState<{ now: OrientScores; candidates: OrientCandidate[] } | null>(null);
  const [picked, setPicked] = useState<number | null>(null);
  const [searching, setSearching] = useState(false);
  // The parts before the first preview; Cancel goes back to them, Apply makes the change one undo step from them.
  const before = useRef<Snapshot | null>(null);
  const original = useRef(part?.matrix ?? null);
  const name = useId();

  function preview(matrix: number[]) {
    const store = useStore.getState();
    before.current ??= store.snapshot();
    store.setPartMatrices(new Map([[partId, matrix]]), null);
  }

  function restore() {
    if (before.current) useStore.getState().discardPreview(before.current);
    before.current = null;
    setPicked(null);
  }

  function close() {
    restore();
    onClose();
  }

  function apply() {
    if (picked === null || !before.current) return;
    useStore.getState().recordChange("Orient", before.current);
    before.current = null;
    onClose();
  }

  async function search() {
    if (!part || searching) return;
    restore();
    // The part as it is now: a preview shown before has just been taken back.
    const now = useStore.getState().parts.find((p) => p.id === partId);
    if (!now) return;
    saveSetting(ORIENT_GOAL_KEY, goal);
    setSearching(true);
    try {
      setFound(await searchOrientations(now, goal));
    } finally {
      setSearching(false);
    }
  }

  function pick(i: number | null) {
    if (!found) return;
    if (i === null) {
      if (original.current) preview(original.current);
      setPicked(null);
      return;
    }
    preview(found.candidates[i].matrix);
    setPicked(i);
  }

  if (!part) return null;
  const column = goal === "least-support" ? 0 : goal === "lowest-height" ? 1 : 2;
  const cells = (s: OrientScores) => [area(s.supportArea), mm(s.height), area(s.footprint)];

  return (
    <Dialog title={`Orient ${part.name}`} onClose={close} onOk={apply} okLabel="Apply" okDisabled={picked === null} className="dlg-arrange dlg-arrange-wide">
      <section className="dlg-section" aria-label="Goal">
        <h3 className="dlg-section-title">Goal</h3>
        <div className="dlg-choices" role="radiogroup" aria-label="Goal">
          {GOALS.map((g) => (
            <label key={g.value} className="dlg-check">
              <input
                type="radio"
                name={name}
                checked={goal === g.value}
                onChange={() => {
                  restore();
                  setFound(null);
                  setGoal(g.value);
                }}
              />
              <span>
                <span className="dlg-check-label">{g.label}</span>
                <span className="dlg-help">{g.help}</span>
              </span>
            </label>
          ))}
        </div>
        <div className="arr-actions">
          <button type="button" className="dlg-btn" disabled={searching} onClick={() => void search()}>
            Search orientations
          </button>
        </div>
      </section>
      {found && (
        <section className="dlg-section" aria-label="Choose orientation">
          <h3 className="dlg-section-title">Choose orientation</h3>
          <table className="arr-table">
            <thead>
              <tr>
                <th scope="col">Orientation</th>
                <th scope="col" className={column === 0 ? "is-goal" : undefined}>
                  Support area (mm²)
                </th>
                <th scope="col" className={column === 1 ? "is-goal" : undefined}>
                  Height (mm)
                </th>
                <th scope="col" className={column === 2 ? "is-goal" : undefined}>
                  Footprint (mm²)
                </th>
              </tr>
            </thead>
            <tbody>
              <tr className={picked === null ? "is-picked" : undefined} onClick={() => pick(null)} aria-selected={picked === null}>
                <th scope="row">As it stands</th>
                {cells(found.now).map((c, k) => (
                  <td key={k} className={k === column ? "is-goal" : undefined}>
                    {c}
                  </td>
                ))}
              </tr>
              {found.candidates.map((cand, i) => (
                <tr key={i} className={picked === i ? "is-picked" : undefined} onClick={() => pick(i)} aria-selected={picked === i}>
                  <th scope="row">{i + 1}</th>
                  {cells(cand).map((c, k) => (
                    <td key={k} className={k === column ? "is-goal" : undefined}>
                      {c}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
          <p className="dlg-note">Click a row to see the part stand that way. Apply keeps the one shown.</p>
        </section>
      )}
    </Dialog>
  );
}
