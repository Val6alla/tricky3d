import { useId, useState } from "react";
import { Dialog } from "./Dialog";
import { answerPrompt } from "./PlacePromptDialog";

/**
 * A file without units that is tiny when read as millimetres: scale it by 1000 (it was modelled in
 * metres) or keep its size. Cancel, Esc or a click outside skips the file.
 */
export function ScaleMetresDialog({ fileName, size, resolve, onClose }: { fileName: string; size: number; resolve(answer: boolean | null): void; onClose(): void }) {
  const [scale, setScale] = useState(true);
  const name = useId();

  return (
    <Dialog
      title="Modelled in metres?"
      onClose={() => answerPrompt(resolve, onClose, null)}
      onOk={() => answerPrompt(resolve, onClose, scale)}
      okLabel="Import"
    >
      <p className="dlg-text">
        <strong className="dlg-file">{fileName}</strong> is only {size.toPrecision(2)} mm across. Its format has no units, so
        tricky3D reads it as millimetres; it may have been modelled in metres.
      </p>
      <div className="dlg-choices" role="radiogroup" aria-label="Modelled in metres?">
        <label className="dlg-check">
          <input type="radio" name={name} checked={scale} onChange={() => setScale(true)} />
          <span>
            <span className="dlg-check-label">Scale by 1000</span>
            <span className="dlg-help">Reads it as metres: {(size * 1000).toPrecision(2)} mm across.</span>
          </span>
        </label>
        <label className="dlg-check">
          <input type="radio" name={name} checked={!scale} onChange={() => setScale(false)} />
          <span>
            <span className="dlg-check-label">Keep its size</span>
            <span className="dlg-help">Uses the numbers in the file as millimetres.</span>
          </span>
        </label>
      </div>
    </Dialog>
  );
}
