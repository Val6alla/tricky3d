// Label generation › Font › …: the font family, its style and the size. The size sets how finely the letters' curves
// are cut, not how big they are (Height and Width do that).
import { useEffect, useMemo, useState } from "react";
import { Dialog } from "../dialogs/Dialog";
import { parseNumber } from "../numbers";
import { loadFonts } from "./session";
import { useLabel } from "./store";
import type { FontInfo } from "./types";

export function FontDialog({
  font,
  size,
  onPick,
  onClose,
}: {
  font: FontInfo;
  size: number;
  onPick(font: FontInfo, size: number): void;
  onClose(): void;
}) {
  const fonts = useLabel((s) => s.fonts);
  const [family, setFamily] = useState(font.family);
  const [style, setStyle] = useState(font.style);
  const [sizeText, setSizeText] = useState(String(size));
  const [filter, setFilter] = useState("");
  useEffect(() => void loadFonts(), []);
  const families = useMemo(() => {
    const seen = new Map<string, FontInfo[]>();
    for (const f of fonts ?? [font]) seen.set(f.family, [...(seen.get(f.family) ?? []), f]);
    return seen;
  }, [fonts, font]);
  const shown = [...families.keys()].filter((f) => f.toLowerCase().includes(filter.trim().toLowerCase()));
  const styles = families.get(family) ?? [];
  const picked = styles.find((f) => f.style === style) ?? styles[0];
  const sizeValue = parseNumber(sizeText);
  const sizeOk = sizeValue !== null && sizeValue >= 4 && sizeValue <= 400;
  return (
    <Dialog
      title="Font"
      onClose={onClose}
      okLabel="OK"
      okDisabled={!picked || !sizeOk}
      onOk={() => picked && sizeOk && onPick(picked, sizeValue)}
      closeLabel="Cancel"
      className="lbl-font-dialog"
    >
      <div className="lbl-font-grid" onKeyDown={(e) => e.stopPropagation()}>
        <label className="lib-field">
          <span className="lib-label">Font</span>
          <input className="dlg-input" type="search" placeholder="Search fonts" value={filter} onChange={(e) => setFilter(e.target.value)} />
          <select
            className="dlg-input lbl-font-list"
            size={10}
            value={family}
            onChange={(e) => {
              setFamily(e.target.value);
              const first = families.get(e.target.value)?.[0];
              if (first) setStyle(first.style);
            }}
          >
            {fonts === null && <option disabled>Looking for fonts…</option>}
            {shown.map((f) => (
              <option key={f} value={f}>
                {f}
              </option>
            ))}
          </select>
        </label>
        <div className="lbl-font-side">
          <label className="lib-field">
            <span className="lib-label">Style</span>
            <select className="dlg-input" value={picked?.style ?? ""} onChange={(e) => setStyle(e.target.value)}>
              {styles.map((f) => (
                <option key={f.id} value={f.style}>
                  {f.style}
                </option>
              ))}
            </select>
          </label>
          <label className="lib-field">
            <span className="lib-label">
              Size <span className="dlg-unit">pt</span>
            </span>
            <input className="dlg-input" type="text" inputMode="decimal" value={sizeText} aria-invalid={sizeOk ? undefined : true} onChange={(e) => setSizeText(e.target.value)} />
          </label>
          <p className="lbl-note">The size sets how smooth the letters' curves are. Height and Width set how big the label is.</p>
        </div>
      </div>
    </Dialog>
  );
}
