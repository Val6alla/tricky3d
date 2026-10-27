import pkg from "../../package.json";
import { Dialog } from "./Dialog";

export function AboutDialog({ onClose }: { onClose(): void }) {
  return (
    <Dialog title="About tricky3D" onClose={onClose}>
      <div className="dlg-about">
        <p className="dlg-about-name">tricky3D</p>
        <p className="dlg-about-version">Version {pkg.version}</p>
        <p>Free, open-source mesh preparation for 3D printing.</p>
        <p>Copyright (C) 2026 the tricky3D contributors.</p>
        <p>
          tricky3D is free software: you can redistribute it and/or modify it under the terms of the GNU General Public
          License, version 3 (gnu.org/licenses/gpl-3.0). It comes with ABSOLUTELY NO WARRANTY. Third-party licences are
          listed in THIRD_PARTY.md.
        </p>
        <p>Works fully offline: no accounts, telemetry or analytics, and its code makes no network connections.</p>
        <p className="dlg-about-note">Not affiliated with or endorsed by Autodesk. Netfabb is a trademark of Autodesk, Inc.</p>
      </div>
    </Dialog>
  );
}
