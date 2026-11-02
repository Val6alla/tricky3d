// The Hollow panel in the right panel: the wall (thickness, accuracy with Generate shell's work estimate, smoothness),
// the cavity as it is worked out (volumes, thin walls, the section view), the drain holes (the shape for new holes or
// the selected one, the list with each hole's check, Auto holes, plugs), then Apply and Close. Our own layout; no
// reference shot.
import { useEffect, useId, useState, type ReactNode } from "react";
import { X } from "lucide-react";
import { SliderNumber } from "../analysis/AnalysisDialog";
import "../analysis/analysis.css";
import "../modify/modify.css";
import "../shell/shell.css";
import { estimateParts, estimateText, heavyWarning } from "../shell/settings";
import { estimateShells } from "../shell/ipc";
import type { ShellEstimate } from "../shell/types";
import { useStore } from "../store";
import {
  applyHollowNow,
  autoHoles,
  cancelApply,
  closeHollow,
  deleteHole,
  editHole,
  editOptions,
  editSettings,
  removeAllHoles,
  selectHole,
  useHollow,
  volumesText,
  cm3,
} from "./session";
import "./hollow.css";

function Box({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="dlg-section md-box" aria-label={title}>
      <h3 className="dlg-section-title">{title}</h3>
      {children}
    </section>
  );
}

function Check({ label, help, checked, onChange }: { label: string; help?: string; checked: boolean; onChange(v: boolean): void }) {
  return (
    <label className="dlg-check" title={help}>
      <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />
      <span>
        <span className="dlg-check-label">{label}</span>
        {help && <span className="dlg-help">{help}</span>}
      </span>
    </label>
  );
}

const SMOOTHNESS = ["Off", "Low", "Medium", "High"];

function Progress({ fraction, stage, what }: { fraction: number; stage: string | null; what: string }) {
  const percent = Math.round(Math.min(1, Math.max(0, fraction)) * 100);
  return (
    <div className="hw-progress" role="status">
      <p className="rpanel-muted">
        {stage ?? what}… {percent} %
      </p>
      <div className="shell-progress" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent}>
        <div className="shell-progress-fill" style={{ width: `${percent}%` }} />
      </div>
    </div>
  );
}

export function HollowPanel() {
  const session = useHollow((h) => h.session);
  const options = useHollow((h) => h.options);
  const part = useStore((s) => s.parts.find((p) => p.id === session?.partId));
  const smoothId = useId();
  const countId = useId();
  const [estimate, setEstimate] = useState<ShellEstimate | null>(null);
  const { thickness, accuracy, smoothness } = options.settings;

  // Generate shell's estimate for the same grid: Rust only does arithmetic, so it comes back at once.
  useEffect(() => {
    if (!part) return;
    let live = true;
    estimateShells(estimateParts([part]), { kind: "hollow-part", thickness, accuracy, smooth: smoothness > 0 })
      .then((e) => live && setEstimate(e))
      .catch(() => live && setEstimate(null));
    return () => {
      live = false;
    };
  }, [part, thickness, accuracy, smoothness]);

  if (!session || !part) return null;
  const { preview, computing, failed, holes, selected, applying } = session;
  const chosen = holes.find((p) => p.id === selected) ?? null;
  const shape = chosen ? chosen.hole : options.shape;
  const editShape = (patch: Partial<typeof shape>) => (chosen ? editHole(chosen.id, patch) : editOptions({ shape: { ...options.shape, ...patch } }));
  const warning = estimate && !estimate.refused ? heavyWarning(estimate) : null;
  const ready = preview !== null && preview.cavity !== null && !computing && !applying;

  return (
    <section className="an-panel md-panel hw-panel" aria-label="Hollow">
      <div className="an-body">
        <h2 className="an-title">
          Hollow
          <span className="an-part" title={part.name}>
            {part.name}
          </span>
        </h2>
        <Box title="Wall">
          <SliderNumber label="Wall thickness" value={thickness} min={0.2} max={50} sliderMax={6} step={0.1} unit="mm" onChange={(v) => editSettings({ thickness: v })} />
          <SliderNumber
            label="Accuracy"
            value={accuracy}
            min={0.02}
            max={Math.max(0.02, thickness / 2)}
            sliderMax={Math.max(0.05, Math.min(2, thickness / 2))}
            step={0.01}
            unit="mm"
            onChange={(v) => editSettings({ accuracy: v })}
          />
          <p className="dlg-note">
            Smaller is finer and slower. {estimate && !estimate.refused ? estimateText(estimate) : (estimate?.refused ?? "")}
          </p>
          {warning && (
            <p className="shell-heavy" role="alert">
              {warning}
            </p>
          )}
          <div className="an-slider-row md-field">
            <label className="dlg-label" htmlFor={smoothId}>
              Inner surface
            </label>
            <select id={smoothId} className="dlg-input dlg-select" value={smoothness} onChange={(e) => editSettings({ smoothness: Number(e.target.value) })}>
              {SMOOTHNESS.map((label, i) => (
                <option key={label} value={i}>
                  {i === 0 ? "As built" : `Smoothed: ${label.toLowerCase()}`}
                </option>
              ))}
            </select>
          </div>
        </Box>
        <Box title="Cavity">
          {computing && <Progress fraction={computing.fraction} stage={computing.stage} what="Working out the cavity" />}
          {!computing && failed && (
            <p className="dlg-error" role="alert">
              {failed}
            </p>
          )}
          {!computing && preview && preview.cavity && (
            <dl className="hw-volumes">
              <dt>Cavity</dt>
              <dd>{cm3(preview.cavityVolume)}</dd>
              <dt>Part</dt>
              <dd>{volumesText(preview)}</dd>
            </dl>
          )}
          {!computing && preview && !preview.cavity && (
            <p className="an-stale" role="note">
              No cavity fits: every wall of the part is thinner than {2 * thickness} mm, twice the wall thickness.
            </p>
          )}
          {!computing && preview?.thin && (
            <p className="an-stale hw-thin" role="note" title="Marked in the view with a box">
              {preview.thin.message} It is marked in the view.
            </p>
          )}
          {!computing && preview?.warnings.map((w) => (
            <p key={w} className="an-stale" role="note">
              {w}
            </p>
          ))}
          <Check label="Section view" checked={options.section} onChange={(section) => editOptions({ section })} help="Cut the view through the part's middle, so the cavity shows. Move the cut with the Y plane in Clip planes." />
        </Box>
        <Box title="Drain holes">
          <p className="dlg-note">Click the part's outside to add a hole there. Drag a hole's handle to move it; Delete removes the selected hole.</p>
          <h4 className="hw-subtitle">{chosen ? `Hole ${holes.indexOf(chosen) + 1}` : "New holes"}</h4>
          <SliderNumber label="Diameter" value={shape.diameter} min={0.3} max={50} sliderMax={8} step={0.1} unit="mm" onChange={(diameter) => editShape({ diameter })} />
          <SliderNumber label="Taper" value={shape.taper} min={0} max={45} sliderMax={30} step={1} unit="°" onChange={(taper) => editShape({ taper })} />
          <SliderNumber label="Flare" value={shape.flare} min={0} max={10} sliderMax={2} step={0.05} unit="mm" onChange={(flare) => editShape({ flare })} />
          <p className="dlg-help">Taper widens the hole towards the outside; Flare rounds its rim on the outside.</p>
          {holes.length > 0 && (
            <ul className="hw-holes" aria-label="Holes">
              {holes.map((p, i) => {
                const bad = p.check && !p.check.ok;
                return (
                  <li key={p.id} className={`hw-hole${p.id === selected ? " is-selected" : ""}${bad ? " is-bad" : ""}`}>
                    <button type="button" className="hw-hole-pick" onClick={() => selectHole(p.id === selected ? null : p.id)} title={bad ? (p.check?.reason ?? "") : "Select to change its size"}>
                      <span className="hw-hole-n">{i + 1}</span>
                      <span className="hw-hole-text">
                        {p.check === null
                          ? `${p.hole.diameter} mm, checking…`
                          : p.check.ok
                            ? `${p.hole.diameter} mm through ${(p.check.depth ?? 0).toFixed(1)} mm of wall`
                            : p.check.reason}
                      </span>
                    </button>
                    <button type="button" className="hw-hole-delete" aria-label={`Remove hole ${i + 1}`} title="Remove" onClick={() => deleteHole(p.id)}>
                      <X size={14} aria-hidden />
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
          <div className="an-row hw-auto">
            <button type="button" className="dlg-btn" disabled={!ready} onClick={autoHoles} title="Place holes at the lowest points of the cavity, as the part stands, spread apart.">
              Auto holes
            </button>
            <label className="dlg-label" htmlFor={countId}>
              How many
            </label>
            <input
              id={countId}
              className="dlg-input hw-count"
              type="number"
              min={1}
              max={12}
              value={options.autoCount}
              onChange={(e) => editOptions({ autoCount: Math.min(12, Math.max(1, Math.round(Number(e.target.value) || 1))) })}
            />
            <button type="button" className="dlg-btn" disabled={holes.length === 0} onClick={removeAllHoles}>
              Remove all
            </button>
          </div>
          <Check label="Make plugs" checked={options.plugs} onChange={(plugs) => editOptions({ plugs })} help="A plug for each hole as a part of its own, smaller by the clearance, to close it after printing." />
          {options.plugs && (
            <SliderNumber label="Clearance" value={options.clearance} min={0} max={2} sliderMax={0.5} step={0.01} unit="mm" onChange={(clearance) => editOptions({ clearance })} />
          )}
        </Box>
        <Box title="Result">
          <Check label="Remove original part" checked={options.removeOriginal} onChange={(removeOriginal) => editOptions({ removeOriginal })} help={`The hollow part takes the original's place; else "${part.name} (hollow)" goes after it.`} />
        </Box>
        {applying && <Progress fraction={applying.fraction} stage={applying.stage} what="Hollowing the part" />}
      </div>
      <div className="an-actions">
        {applying ? (
          <button type="button" className="dlg-btn" disabled={applying.cancelling} onClick={cancelApply}>
            {applying.cancelling ? "Cancelling…" : "Cancel"}
          </button>
        ) : (
          <>
            <button type="button" className="dlg-btn dlg-btn-primary" disabled={!ready} onClick={() => void applyHollowNow()} title="Make the hollow part with its holes, as one undo step.">
              Apply
            </button>
            <button type="button" className="dlg-btn" onClick={closeHollow}>
              Close
            </button>
          </>
        )}
      </div>
    </section>
  );
}
