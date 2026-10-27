import { formatCount } from "../info";
import type { SplitAnswer } from "../ui";
import { Dialog } from "./Dialog";
import { answerPrompt } from "./PlacePromptDialog";

export interface SplitManyDialogProps {
  count: number;
  /** The part that would split, or null when several would. */
  partName: string | null;
  resolve(answer: SplitAnswer | null): void;
  onClose(): void;
}

/**
 * Split shells into parts would make more than MANY_PARTS parts (src/ops/meshOps.ts). Repair first, the default
 * (focused, and Enter), opens repair mode instead; Split anyway goes ahead. Esc or a click outside does neither.
 */
export function SplitManyDialog({ count, partName, resolve, onClose }: SplitManyDialogProps) {
  return (
    <Dialog
      title="Split into many parts?"
      onClose={() => answerPrompt(resolve, onClose, null)}
      onOk={() => answerPrompt(resolve, onClose, "repair")}
      okLabel="Repair first"
      secondary={{ label: "Split anyway", onClick: () => answerPrompt(resolve, onClose, "split") }}
    >
      <p className="dlg-text">
        {partName === null ? "The selected parts" : <strong className="dlg-file">{partName}</strong>} would split into{" "}
        {formatCount(count)} parts.
      </p>
      <p className="dlg-text">
        Shells part wherever an edge is open, non-manifold or turned over, so a damaged mesh falls into many small pieces.
        Repair can fix those edges first.
      </p>
    </Dialog>
  );
}
