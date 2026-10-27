import { useStore } from "../store";
import { Dialog } from "./Dialog";

interface FormatRow {
  name: string;
  detail: string;
  ext: string;
}

const IMPORT: FormatRow[] = [
  { name: "STL", detail: "Binary and ASCII", ext: ".stl" },
  { name: "OBJ", detail: "Can split objects into parts", ext: ".obj" },
  { name: "PLY", detail: "ASCII and binary", ext: ".ply" },
  { name: "3MF", detail: "Objects, placement and units", ext: ".3mf" },
  { name: "AMF", detail: "Plain or zipped, with units", ext: ".amf" },
  { name: "OFF", detail: "Text, polygons split into triangles", ext: ".off" },
];

const EXPORT: FormatRow[] = [
  { name: "STL (binary)", detail: "Parts merged into one mesh", ext: ".stl" },
  { name: "STL (ASCII)", detail: "Parts merged into one mesh", ext: ".stl" },
  { name: "OBJ", detail: "One object per part", ext: ".obj" },
  { name: "PLY (binary)", detail: "Parts merged into one mesh", ext: ".ply" },
  { name: "3MF", detail: "One object per part, in mm", ext: ".3mf" },
  { name: "AMF", detail: "One object per part, in mm", ext: ".amf" },
];

const IMPORT_SOON = ["X3D", "VRML", "3DS", "GTS", "Colour STL", "STEP", "IGES"];
const EXPORT_SOON = ["Colour STL", "GTS", "X3D", "3DS", "VRML", "BINVOX", "Compressed mesh", "Formlabs", "Slice"];

function FormatSection({ title, rows, soon, hideSoon }: { title: string; rows: FormatRow[]; soon: string[]; hideSoon: boolean }) {
  return (
    <section className="dlg-formats-col">
      <h3 className="dlg-section-title">{title}</h3>
      <ul className="dlg-formats">
        {rows.map((r) => (
          <li key={r.name} className="dlg-format">
            <span className="dlg-format-name">{r.name}</span>
            <span className="dlg-format-detail">{r.detail}</span>
            <span className="dlg-format-ext">{r.ext}</span>
          </li>
        ))}
      </ul>
      {!hideSoon && (
        <div className="dlg-formats-later">
          <span className="dlg-formats-later-label">Coming later</span>
          <ul className="dlg-formats-soon" aria-label={`${title} formats coming later`}>
            {soon.map((name) => (
              <li key={name} className="dlg-format-soon">
                {name}
              </li>
            ))}
          </ul>
        </div>
      )}
    </section>
  );
}

/** File › Supported formats…: what tricky3D reads and writes, and what is planned. */
export function FormatsDialog({ onClose }: { onClose(): void }) {
  const hideSoon = useStore((s) => s.hideSoon);
  return (
    <Dialog title="Supported file formats" onClose={onClose} className="dlg-formats-dialog">
      {/* Focusable so the keyboard can scroll it if a short window cuts it off; Close is the dialog's only other control. */}
      <div className="dlg-formats-scroll" role="region" aria-label="Format lists" tabIndex={0}>
        <div className="dlg-formats-cols">
          <FormatSection title="Import" rows={IMPORT} soon={IMPORT_SOON} hideSoon={hideSoon} />
          <FormatSection title="Export" rows={EXPORT} soon={EXPORT_SOON} hideSoon={hideSoon} />
        </div>
        <p className="dlg-note">STL, OBJ and PLY files have no units, so tricky3D reads them as millimetres.</p>
      </div>
    </Dialog>
  );
}
