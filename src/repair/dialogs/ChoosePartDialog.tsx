// Repair mode › Choose part: Repair was asked for with several parts selected, and repair mode works on one part,
// so the user picks it. The parts are listed as they were selected, each with its damage badge when it has one.
import { useEffect, useId, useMemo, useState } from "react";
import { TriangleAlert } from "lucide-react";
import { Dialog } from "../../dialogs/Dialog";
import { formatCount, triangleCount } from "../../info";
import { useStore, type Part } from "../../store";
import { badgeText, cachedCheck, useBadges } from "../badges";
import { enterRepair } from "../session";
import "./dialogs.css";

/** One part the dialog offers. */
export interface PartChoice {
  id: string;
  name: string;
  color: string;
  triangles: number;
  /** The damage badge's text, or null when the part is closed and oriented or has not been checked yet. */
  badge: string | null;
}

/** The parts named by `ids` that still exist, in the order of `ids`, with their damage badge text. */
export function partChoices(parts: readonly Part[], ids: readonly string[]): PartChoice[] {
  const byId = new Map(parts.map((p) => [p.id, p]));
  return ids.flatMap((id) => {
    const part = byId.get(id);
    if (!part) return [];
    const check = cachedCheck(part.mesh);
    return [{ id, name: part.name, color: part.color, triangles: triangleCount(part.mesh), badge: check ? badgeText(check) : null }];
  });
}

/** Repair: closes the dialog, then opens repair mode for the part (enterRepair says with a toast when it cannot). */
export function choosePart(id: string, onClose: () => void): void {
  onClose();
  enterRepair(id);
}

export function ChoosePartDialog({ ids, onClose }: { ids: readonly string[]; onClose(): void }) {
  const parts = useStore((s) => s.parts);
  // The badges come from a cache outside React; a new check result bumps this version.
  const badges = useBadges((s) => s.version);
  const choices = useMemo(() => partChoices(parts, ids), [parts, ids, badges]);

  // The parts can go while the dialog is open (for example by undo); then there is nothing left to choose from.
  useEffect(() => {
    if (choices.length === 0) onClose();
  }, [choices.length, onClose]);

  if (choices.length === 0) return null;
  return <ChoosePartForm choices={choices} onClose={onClose} />;
}

export interface ChoosePartFormProps {
  /** The parts to choose from, in order; the first starts picked. */
  choices: readonly PartChoice[];
  onClose(): void;
}

export function ChoosePartForm({ choices, onClose }: ChoosePartFormProps) {
  const [picked, setPicked] = useState<string | null>(null);
  const group = useId();
  const promptId = useId();
  // The first part until another is picked; a picked part that went away falls back to the first as well.
  const current = choices.find((c) => c.id === picked)?.id ?? choices[0]?.id ?? null;

  return (
    <Dialog
      title="Repair part"
      onClose={onClose}
      onOk={() => {
        if (current !== null) choosePart(current, onClose);
      }}
      okLabel="Repair"
      okDisabled={current === null}
      className="dlg-rp-choose"
    >
      <p id={promptId} className="dlg-text">
        Several parts are selected. Pick the one to repair.
      </p>
      <div className="dlg-list" role="radiogroup" aria-labelledby={promptId}>
        {choices.map((c) => (
          // A double click picks the part and repairs it at once.
          <label key={c.id} className="dlg-check dlg-list-item rp-choice" title={c.name} onDoubleClick={() => choosePart(c.id, onClose)}>
            <input type="radio" name={group} checked={c.id === current} onChange={() => setPicked(c.id)} />
            <span className="rp-swatch" style={{ background: c.color }} aria-hidden="true" />
            <span className="dlg-list-name">{c.name}</span>
            {c.badge && (
              <span className="rp-badge" role="img" aria-label={c.badge} title={c.badge}>
                <TriangleAlert size={13} aria-hidden="true" />
              </span>
            )}
            <span className="rp-count">
              {formatCount(c.triangles)} {c.triangles === 1 ? "triangle" : "triangles"}
            </span>
          </label>
        ))}
      </div>
    </Dialog>
  );
}
