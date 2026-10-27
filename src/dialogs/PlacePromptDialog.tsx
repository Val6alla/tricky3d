import { useId, useState } from "react";
import type { PlaceAnswer } from "../ui";
import { Dialog } from "./Dialog";

const answered = new WeakSet<object>();

/**
 * Answers a prompt exactly once, then closes it. A double click, or Enter right after a click,
 * cannot answer twice; the guard is per resolve function, so it holds even if React reuses the
 * component for the next prompt.
 */
export function answerPrompt<T>(resolve: (answer: T | null) => void, onClose: () => void, answer: T | null) {
  if (answered.has(resolve)) return;
  answered.add(resolve);
  resolve(answer);
  onClose();
}

/** "Place on platform?" for one imported file. Cancel, Esc or a click outside skips the file. */
export function PlacePromptDialog({ fileName, resolve, onClose }: { fileName: string; resolve(answer: PlaceAnswer | null): void; onClose(): void }) {
  const [place, setPlace] = useState(true);
  const [remember, setRemember] = useState(false);
  const name = useId();
  const rememberHelp = useId();

  return (
    <Dialog
      title="Place on platform?"
      onClose={() => answerPrompt(resolve, onClose, null)}
      onOk={() => answerPrompt(resolve, onClose, { place, remember })}
      okLabel="Import"
    >
      <p className="dlg-text">
        <strong className="dlg-file">{fileName}</strong>
      </p>
      <div className="dlg-choices" role="radiogroup" aria-label="Place on platform?">
        <label className="dlg-check">
          <input type="radio" name={name} checked={place} onChange={() => setPlace(true)} />
          <span>
            <span className="dlg-check-label">Yes, place it on the platform</span>
            <span className="dlg-help">Rests it on the platform, centred.</span>
          </span>
        </label>
        <label className="dlg-check">
          <input type="radio" name={name} checked={!place} onChange={() => setPlace(false)} />
          <span>
            <span className="dlg-check-label">No, keep its position</span>
            <span className="dlg-help">Uses the coordinates stored in the file.</span>
          </span>
        </label>
      </div>
      <label className="dlg-check dlg-check-separated">
        <input type="checkbox" checked={remember} aria-describedby={rememberHelp} onChange={(e) => setRemember(e.target.checked)} />
        <span>
          <span className="dlg-check-label">Remember my answer</span>
          <span id={rememberHelp} className="dlg-help">
            Change it later in File › Settings.
          </span>
        </span>
      </label>
    </Dialog>
  );
}
