// Right-click › Packing settings: the selected parts' priority, permitted rotation and position lock, as one undo step.
// The lock is the part's own lock (the parts list's padlock): a locked part never moves and the others pack round it.
import { useId, useMemo, useState } from "react";
import { Dialog } from "../dialogs/Dialog";
import "../dialogs/transform.css";
import { DEFAULT_PACKING, useStore, type PartPatch, type PartRotation } from "../store";
import "./arrange.css";

const ROTATIONS: { value: PartRotation; label: string }[] = [
  { value: "default", label: "Method's setting" },
  { value: "none", label: "No rotation" },
  { value: "z", label: "Around Z only" },
  { value: "free", label: "Any rotation" },
];

/** Several parts with different values show this until one is picked; it changes nothing. */
const MIXED = "mixed";

export function PackingSettingsDialog({ ids, onClose }: { ids: string[]; onClose(): void }) {
  const all = useStore((s) => s.parts);
  const parts = useMemo(() => all.filter((p) => ids.includes(p.id)), [all, ids]);
  const first = (pick: (p: (typeof parts)[number]) => string) => {
    const values = new Set(parts.map(pick));
    return values.size === 1 ? [...values][0] : MIXED;
  };
  const [priority, setPriority] = useState(() => first((p) => String((p.packing ?? DEFAULT_PACKING).priority)));
  const [rotation, setRotation] = useState(() => first((p) => (p.packing ?? DEFAULT_PACKING).rotation));
  const [locked, setLocked] = useState(() => first((p) => (p.locked ? "yes" : "no")));
  const priorityId = useId();
  const rotationId = useId();

  function apply() {
    const patches = new Map<string, PartPatch>();
    for (const p of parts) {
      const old = p.packing ?? DEFAULT_PACKING;
      const packing = {
        priority: priority === MIXED ? old.priority : Number(priority),
        rotation: rotation === MIXED ? old.rotation : (rotation as PartRotation),
      };
      const patch: PartPatch = {};
      if (packing.priority !== old.priority || packing.rotation !== old.rotation) patch.packing = packing;
      if (locked !== MIXED) patch.locked = locked === "yes";
      patches.set(p.id, patch);
    }
    useStore.getState().patchEach(patches, "Packing settings");
    onClose();
  }

  const title = parts.length === 1 ? `Packing settings: ${parts[0].name}` : `Packing settings: ${parts.length} parts`;
  return (
    <Dialog title={title} onClose={onClose} onOk={apply} className="dlg-arrange">
      <div className="tf-row">
        <label className="dlg-label" htmlFor={priorityId}>
          Priority
        </label>
        <select id={priorityId} className="dlg-input dlg-select" value={priority} onChange={(e) => setPriority(e.target.value)}>
          {priority === MIXED && <option value={MIXED}>Different for each part</option>}
          {Array.from({ length: 11 }, (_, k) => 10 - k).map((k) => (
            <option key={k} value={String(k)}>
              {k === 10 ? "10 (first)" : k === 0 ? "0 (last)" : String(k)}
            </option>
          ))}
        </select>
      </div>
      <p className="dlg-note">Parts with a higher priority are packed first and lowest.</p>
      <div className="tf-row">
        <label className="dlg-label" htmlFor={rotationId}>
          Permitted rotation
        </label>
        <select id={rotationId} className="dlg-input dlg-select" value={rotation} onChange={(e) => setRotation(e.target.value)}>
          {rotation === MIXED && <option value={MIXED}>Different for each part</option>}
          {ROTATIONS.map((r) => (
            <option key={r.value} value={r.value}>
              {r.label}
            </option>
          ))}
        </select>
      </div>
      <p className="dlg-note">Monte Carlo packing follows it; the other methods turn every part by their own settings.</p>
      <label className="dlg-check">
        <input
          type="checkbox"
          checked={locked === "yes"}
          ref={(el) => {
            if (el) el.indeterminate = locked === MIXED;
          }}
          onChange={(e) => setLocked(e.target.checked ? "yes" : "no")}
        />
        <span>
          <span className="dlg-check-label">Lock position</span>
          <span className="dlg-help">The part stays where it is and the others pack round it. It is the parts list's lock, so the part cannot be moved or deleted either until unlocked.</span>
        </span>
      </label>
    </Dialog>
  );
}
