import { useEffect, useState } from "react";
import { useStore } from "../store";
import { Dialog } from "./Dialog";

interface RenameDialogProps {
  partId: string;
  onClose(): void;
}

export function RenameDialog({ partId, onClose }: RenameDialogProps) {
  const part = useStore((s) => s.parts.find((p) => p.id === partId));
  const [name, setName] = useState(part?.name ?? "");

  // The part can be deleted while the dialog is open (for example by undo); there is nothing left to rename.
  useEffect(() => {
    if (!part) onClose();
  }, [part, onClose]);

  if (!part) return null;

  const empty = name.trim() === "";

  function ok() {
    if (empty) return;
    useStore.getState().renamePart(partId, name);
    onClose();
  }

  return (
    <Dialog title="Rename part" onClose={onClose} onOk={ok} okDisabled={empty}>
      <label className="dlg-field">
        <span className="dlg-label">Name</span>
        <input className="dlg-input" type="text" spellCheck={false} value={name} onChange={(e) => setName(e.target.value)} />
      </label>
    </Dialog>
  );
}
