// Repair panel › Status: whether the working copy is closed and oriented, its statistics, Update and Auto-update,
// and what the viewport highlights. "?" means the status was not computed for the mesh as it is now.
import { useId } from "react";
import { RefreshCw } from "lucide-react";
import { closedMark, orientedMark, statusRows } from "../format";
import { refreshStatus } from "../session";
import { useRepair } from "../store";
import type { HighlightSettings, RepairStatus } from "../types";
import { cx } from "./controls";

/** The "Edges from" slider's far end, which turns the dihedral edge highlight off. */
export const EDGES_OFF = 180;

/** The slider position for an "Edges from" setting: its degrees, or the far end when off (null). */
export function edgesSliderValue(edgesFrom: number | null): number {
  return edgesFrom === null ? EDGES_OFF : Math.max(0, Math.min(EDGES_OFF - 1, edgesFrom));
}

/** The setting a slider position stands for: whole degrees, or null (off) at the far end. */
export function edgesFromSlider(position: number): number | null {
  return position >= EDGES_OFF ? null : Math.max(0, Math.round(position));
}

/** "30°", or "off". */
export function edgesFromText(edgesFrom: number | null): string {
  return edgesFrom === null ? "off" : `${edgesFrom}°`;
}

/** The statistics that count problems, shown in the warning colour when not zero. */
const PROBLEM_ROWS = new Set(["Border edges", "Inv. orientation", "Holes", "Non-manifold edges"]);

/** True for a statistics row (from statusRows) that counts a problem and is not zero. */
export function isProblemRow(label: string, text: string): boolean {
  return PROBLEM_ROWS.has(label) && text !== "0" && text !== "?";
}

/** Switching Auto-update on while the status is unknown computes it at once, unless a run is going (it would refuse). */
export function refreshOnAutoUpdate(on: boolean, status: RepairStatus | null, busy: boolean): boolean {
  return on && status === null && !busy;
}

/** The class of a ✓ / ✗ / ? mark. */
const markClass = (mark: string) => (mark === "✓" ? "is-yes" : mark === "✗" ? "is-no" : "is-unknown");

type Switch = Exclude<keyof HighlightSettings, "edgesFrom">;

/** The highlighting switches; `swatch` names the colour beside each (repair.css), `line` draws it as a line. */
const SWITCHES: { key: Switch; label: string; swatch: string; line?: boolean; hint: string }[] = [
  { key: "holes", label: "Holes", swatch: "holes", line: true, hint: "Draw the border edges of holes as thick yellow lines." },
  { key: "triangles", label: "Triangles", swatch: "inverted", hint: "Draw triangles with inverted orientation in red." },
  { key: "degenerate", label: "Degenerate faces", swatch: "degenerate", hint: "Draw degenerate faces in magenta." },
  { key: "errors", label: "Errors", swatch: "errors", line: true, hint: "Draw non-manifold edges and detected self-intersections in orange." },
];

export function StatusTab() {
  const status = useRepair((s) => s.session?.status ?? null);
  const busy = useRepair((s) => s.busy !== null);
  const autoUpdate = useRepair((s) => s.autoUpdate);
  const highlight = useRepair((s) => s.highlight);
  const { setAutoUpdate, setHighlight } = useRepair.getState();
  const titleId = useId();
  const marks = [
    { label: "Mesh is closed", mark: closedMark(status) },
    { label: "Mesh is oriented", mark: orientedMark(status) },
  ];

  function switchAutoUpdate(on: boolean) {
    setAutoUpdate(on);
    if (refreshOnAutoUpdate(on, status, busy)) void refreshStatus();
  }

  const check = ({ key, label, swatch, line, hint }: (typeof SWITCHES)[number]) => (
    <label key={key} className="clip-check rp-check" title={hint}>
      <input type="checkbox" checked={highlight[key]} onChange={(e) => setHighlight({ [key]: e.target.checked })} />
      <span className={cx("rp-swatch", `rp-swatch-${swatch}`, line && "is-line")} aria-hidden="true" />
      {label}
    </label>
  );

  return (
    <div className="rp-body">
      <section className="rp-group" aria-label="Status">
        <dl className="rpanel-info rp-info">
          {marks.map(({ label, mark }) => (
            <div key={label}>
              <dt>{label}</dt>
              <dd className={cx("rp-mark", markClass(mark))}>{mark}</dd>
            </div>
          ))}
        </dl>
        <dl className="rpanel-info rp-info rp-stats">
          {statusRows(status).map(({ label, text }) => (
            <div key={label}>
              <dt>{label}</dt>
              <dd className={cx(text !== "?" && "rp-value", isProblemRow(label, text) && "rp-problem")}>{text}</dd>
            </div>
          ))}
        </dl>
        {!status && <p className="rpanel-muted">Not computed for the mesh as it is now. Click Update.</p>}
        <div className="rp-row">
          <button
            type="button"
            className="clip-btn"
            title="Compute the status and highlights of the working copy"
            disabled={busy}
            onClick={() => void refreshStatus()}
          >
            <RefreshCw size={13} aria-hidden="true" />
            Update
          </button>
          <label className="clip-check" title="Compute the status again after every action">
            <input type="checkbox" checked={autoUpdate} onChange={(e) => switchAutoUpdate(e.target.checked)} />
            Auto-update
          </label>
        </div>
      </section>
      <section className="rp-group" aria-labelledby={titleId}>
        <h3 id={titleId} className="rp-title">
          Highlight
        </h3>
        <div className="rp-checks">
          {SWITCHES.slice(0, 2).map(check)}
          <EdgesFrom value={highlight.edgesFrom} onChange={(edgesFrom) => setHighlight({ edgesFrom })} />
          {SWITCHES.slice(2).map(check)}
        </div>
      </section>
    </div>
  );
}

/** "Edges from": edges whose dihedral angle exceeds the slider's degrees are drawn; the far end turns it off. */
function EdgesFrom({ value, onChange }: { value: number | null; onChange(v: number | null): void }) {
  const text = edgesFromText(value);
  return (
    <div className="rp-edges" title="Draw the edges where the surface bends by more than this angle">
      <span className="rp-edges-label">Edges from</span>
      <input
        type="range"
        className="clip-slider"
        aria-label="Edges from"
        aria-valuetext={value === null ? "off" : `${value} degrees`}
        min={0}
        max={EDGES_OFF}
        step={1}
        value={edgesSliderValue(value)}
        onChange={(e) => onChange(edgesFromSlider(Number(e.target.value)))}
      />
      <output className={cx("rp-edges-value", value === null && "is-off")}>{text}</output>
    </div>
  );
}
