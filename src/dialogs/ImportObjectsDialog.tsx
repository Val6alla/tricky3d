import { useId, useState } from "react";
import type { ObjectsAnswer } from "../ui";
import { Dialog } from "./Dialog";
import { answerPrompt } from "./PlacePromptDialog";

/** "One part / one part per object" for a file with several objects. Cancel, Esc or a click outside skips the file. */
export function ImportObjectsDialog({ fileName, count, resolve, onClose }: { fileName: string; count: number; resolve(answer: ObjectsAnswer | null): void; onClose(): void }) {
  const [choice, setChoice] = useState<ObjectsAnswer>("split");
  const name = useId();

  return (
    <Dialog
      title="Import as separate parts?"
      onClose={() => answerPrompt(resolve, onClose, null)}
      onOk={() => answerPrompt(resolve, onClose, choice)}
      okLabel="Import"
    >
      <p className="dlg-text">
        <strong className="dlg-file">{fileName}</strong> has {count} objects.
      </p>
      <div className="dlg-choices" role="radiogroup" aria-label="Import as">
        <label className="dlg-check">
          <input type="radio" name={name} checked={choice === "split"} onChange={() => setChoice("split")} />
          <span>
            <span className="dlg-check-label">One part per object</span>
            <span className="dlg-help">Each object becomes its own part.</span>
          </span>
        </label>
        <label className="dlg-check">
          <input type="radio" name={name} checked={choice === "single"} onChange={() => setChoice("single")} />
          <span>
            <span className="dlg-check-label">One part</span>
            <span className="dlg-help">All objects are joined into a single part.</span>
          </span>
        </label>
      </div>
    </Dialog>
  );
}
