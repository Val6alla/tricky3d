import { useId, useState } from "react";
import type { ExportOptions } from "../store";
import { Dialog } from "./Dialog";
import { answerPrompt } from "./PlacePromptDialog";

export interface ExportOptionsDialogProps {
  /** How many parts are being exported; "Several parts" only shows for more than one. */
  count: number;
  /** The options used last time. */
  options: ExportOptions;
  resolve(answer: ExportOptions | null): void;
  onClose(): void;
}

function Choice({ name, checked, label, help, onPick }: { name: string; checked: boolean; label: string; help: string; onPick(): void }) {
  return (
    <label className="dlg-check">
      <input type="radio" name={name} checked={checked} onChange={onPick} />
      <span>
        <span className="dlg-check-label">{label}</span>
        <span className="dlg-help">{help}</span>
      </span>
    </label>
  );
}

/**
 * Export's options, before the system's save dialog: how STL is written and, for several parts, one file or one file
 * per part. It opens on the options used last time. Continue goes on to the save dialog; Cancel, Esc or a click
 * outside stops the export.
 */
export function ExportOptionsDialog({ count, options, resolve, onClose }: ExportOptionsDialogProps) {
  const [stl, setStl] = useState(options.stl);
  const [parts, setParts] = useState(options.parts);
  const stlName = useId();
  const partsName = useId();
  const stlTitle = useId();
  const partsTitle = useId();

  return (
    <Dialog
      title={count === 1 ? "Export 1 part" : `Export ${count} parts`}
      onClose={() => answerPrompt(resolve, onClose, null)}
      onOk={() => answerPrompt(resolve, onClose, { stl, parts })}
      okLabel="Continue"
    >
      <section className="dlg-section" aria-labelledby={stlTitle}>
        <h3 id={stlTitle} className="dlg-section-title">
          STL type
        </h3>
        <div className="dlg-choices" role="radiogroup" aria-labelledby={stlTitle}>
          <Choice name={stlName} checked={stl === "binary"} label="Binary" help="Small and quick to read. Most programs expect this." onPick={() => setStl("binary")} />
          <Choice name={stlName} checked={stl === "ascii"} label="ASCII" help="Plain text you can read, several times larger." onPick={() => setStl("ascii")} />
        </div>
      </section>
      {count > 1 && (
        <section className="dlg-section" aria-labelledby={partsTitle}>
          <h3 id={partsTitle} className="dlg-section-title">
            Several parts
          </h3>
          <div className="dlg-choices" role="radiogroup" aria-labelledby={partsTitle}>
            <Choice
              name={partsName}
              checked={parts === "one-file"}
              label="One file with all parts"
              help="The file name and type come from the next dialog."
              onPick={() => setParts("one-file")}
            />
            <Choice
              name={partsName}
              checked={parts === "per-part"}
              label="One file per part"
              help="Each file is named after its part. In the next dialog, pick the folder and the type."
              onPick={() => setParts("per-part")}
            />
          </div>
        </section>
      )}
      <p className="dlg-note">Parts are written exactly where they sit on the platform, so they keep their places relative to each other.</p>
    </Dialog>
  );
}
