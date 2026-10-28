// Measure mode's right panel: the Measurements list (every measurement with its value; a click highlights it, the
// cross deletes it), the Tools tab (every tool as an icon, what the one picked needs and does), and the Options
// (snapping, the axes of a distance, radius or diameter, marker and arrow sizes). Advanced, Reset and Cancel at the
// bottom.
import { useMemo, type ReactNode } from "react";
import { X } from "lucide-react";
import { anyEnabled, useClip } from "../clip";
import { useStore } from "../store";
import { toast } from "../ui";
import { detailLines, valueText } from "./format";
import { GROUP_ICONS } from "./icons";
import { allMeasurements, closeMeasure, deleteMeasurement, pickMeasureTool, resetMeasurements, selectMeasurement, setPanelTab } from "./session";
import { useMeasure, type MeasurePanelTab } from "./store";
import { GROUP_LABELS, TOOLS, chosenText, describe, nextClickText, toolById, type ToolGroup } from "./tools";
import type { SnapMode } from "./types";
import "./measure.css";

const TABS: { id: MeasurePanelTab; label: string }[] = [
  { id: "measurements", label: "Measurements" },
  { id: "tools", label: "Tools" },
  { id: "options", label: "Options" },
];

export function MeasurePanel() {
  const tab = useMeasure((s) => s.session?.panelTab ?? "measurements");
  return (
    <section className="ms-panel" aria-label="Measure">
      <div className="ms-tabs" role="tablist">
        {TABS.map((t) => (
          <button key={t.id} type="button" role="tab" aria-selected={tab === t.id} className={`ms-tab${tab === t.id ? " is-selected" : ""}`} onClick={() => setPanelTab(t.id)}>
            {t.label}
          </button>
        ))}
      </div>
      <div className="ms-body">{tab === "measurements" ? <MeasurementsTab /> : tab === "tools" ? <ToolsTab /> : <OptionsTab />}</div>
      <div className="ms-actions">
        <button type="button" className="dlg-btn" title="Advanced measurement options are coming in a future version." onClick={() => toast("Advanced measurement options are coming in a future version.")}>
          Advanced <span className="soon-pill">Soon</span>
        </button>
        <button type="button" className="dlg-btn" title="Delete every measurement (one undo step)." onClick={resetMeasurements}>
          Reset
        </button>
        <button type="button" className="dlg-btn" onClick={closeMeasure}>
          Cancel
        </button>
      </div>
    </section>
  );
}

function MeasurementsTab() {
  const parts = useStore((s) => s.parts);
  const results = useMeasure((s) => s.results);
  const selected = useMeasure((s) => s.session?.selected ?? null);
  const session = useMeasure((s) => s.session);
  const list = useMemo(() => allMeasurements(parts), [parts]);
  const tool = toolById(session?.tool ?? "");

  return (
    <>
      {tool && session && (
        <p className="ms-next" role="status">
          {tool.label}: {nextClickText(tool, session.picks.length, session.points.length)}
        </p>
      )}
      {list.length === 0 ? (
        <p className="rpanel-muted">No measurements yet. Pick a tool in the Measurement tab and click on a part.</p>
      ) : (
        <ul className="ms-list" aria-label="Measurements">
          {list.map(({ part, measurement }, i) => {
            const entry = results[measurement.id];
            const label = toolById(measurement.tool)?.label ?? measurement.tool;
            return (
              <li key={measurement.id} className={`ms-item${selected === measurement.id ? " is-selected" : ""}`}>
                <button type="button" className="ms-item-main" aria-pressed={selected === measurement.id} onClick={() => selectMeasurement(selected === measurement.id ? null : measurement.id)}>
                  <span className="ms-item-title">
                    {i + 1}. {label}
                  </span>
                  <span className="ms-item-part">{part.name}</span>
                  <span className="ms-item-value">{entry?.result ? valueText(measurement, entry.result) : (entry?.error ?? "…")}</span>
                  {selected === measurement.id && entry?.result && detailLines(entry.result).map((line) => <span key={line} className="ms-item-detail">{line}</span>)}
                </button>
                <button type="button" className="ms-delete" aria-label={`Delete measurement ${i + 1}`} title="Delete this measurement." onClick={() => deleteMeasurement(measurement.id)}>
                  <X size={13} aria-hidden />
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </>
  );
}

const GROUP_ORDER: ToolGroup[] = ["point", "edge", "circle", "circle3", "sphere4", "surface", "wall", "angle3"];

function ToolsTab() {
  const toolId = useMeasure((s) => s.session?.tool ?? "");
  const tool = toolById(toolId);
  return (
    <>
      <h3 className="ms-heading">Tool selection</h3>
      {GROUP_ORDER.map((group) => {
        const Icon = GROUP_ICONS[group];
        return (
          <div key={group} className="ms-tool-row" role="group" aria-label={GROUP_LABELS[group]}>
            <span className="ms-tool-group" title={GROUP_LABELS[group]}>
              <Icon size={14} aria-hidden />
            </span>
            {TOOLS.filter((t) => t.group === group).map((t) => (
              <button key={t.id} type="button" className={`ms-tool${t.id === toolId ? " is-active" : ""}`} aria-pressed={t.id === toolId} title={t.label} onClick={() => pickMeasureTool(t.id)}>
                <Icon size={15} aria-hidden />
                <span className="ms-tool-tag">{tagOf(t.id)}</span>
              </button>
            ))}
          </div>
        );
      })}
      {tool && (
        <>
          <h3 className="ms-heading">Chosen tools</h3>
          <p className="ms-text">{chosenText(tool)}</p>
          <h3 className="ms-heading">Description</h3>
          <p className="ms-text">
            <strong>{tool.label}.</strong> {describe(tool)}
          </p>
        </>
      )}
    </>
  );
}

/** A short tag under a tool's icon: what it is measured against ("E" for an edge, "XY" for that plane...). */
function tagOf(id: string): string {
  const tail = id.split("-").slice(1).join("-");
  const tags: Record<string, string> = {
    point: "P",
    edge: "E",
    circle: "C",
    circle3: "3C",
    sphere: "S",
    surface: "F",
    position: "Pos",
    "edge-angle": "∠E",
    "surface-angle": "∠F",
    yz: "YZ",
    xz: "XZ",
    xy: "XY",
    centres: "CC",
    radius: "R",
    "circle3-angle": "∠",
  };
  return tags[tail] ?? (id === "wall" ? "T" : id === "angle3" ? "∠3" : "");
}

const SNAPS: { id: SnapMode; label: string; hint: string; clip?: boolean }[] = [
  { id: "auto", label: "Automatic", hint: "A corner when one is near the pointer, else a real edge when one is near, else the point clicked." },
  { id: "free", label: "Free point", hint: "The point clicked on the surface, not snapped." },
  { id: "edge", label: "Point on edge", hint: "The nearest point on the nearest real edge, where the surface folds." },
  { id: "node", label: "Point on node", hint: "The nearest corner of the mesh." },
  { id: "clip-line", label: "Point on clip line", hint: "Where a clip plane cuts the surface. Needs a clip plane.", clip: true },
  { id: "clip-corner", label: "Point on clip line corner", hint: "Where a clip plane crosses a real edge. Needs a clip plane.", clip: true },
];

function OptionsTab() {
  const options = useMeasure((s) => s.options);
  const set = useMeasure((s) => s.setOptions);
  const clipping = useClip((c) => anyEnabled(c));
  return (
    <>
      <Fieldset title="Snapping">
        {SNAPS.map((s) => (
          <label key={s.id} className="dlg-check" title={s.hint}>
            <input type="radio" name="ms-snap" checked={options.snap === s.id} disabled={s.clip && !clipping} onChange={() => set({ snap: s.id })} />
            <span className="dlg-check-label">{s.label}</span>
          </label>
        ))}
      </Fieldset>
      <Fieldset title="Distance measurement options">
        <div className="ms-row">
          {(["X", "Y", "Z"] as const).map((axis, k) => (
            <label key={axis} className="dlg-check" title={`Show the distance along ${axis} on the next measurement's label.`}>
              <input
                type="checkbox"
                checked={options.axes[k]}
                onChange={(e) => set({ axes: options.axes.map((v, j) => (j === k ? e.target.checked : v)) as [boolean, boolean, boolean] })}
              />
              <span className="dlg-check-label">{axis}</span>
            </label>
          ))}
        </div>
      </Fieldset>
      <Fieldset title="Radius measurement options">
        <div className="ms-row">
          <label className="dlg-check">
            <input type="radio" name="ms-radius" checked={!options.diameter} onChange={() => set({ diameter: false })} />
            <span className="dlg-check-label">Radius</span>
          </label>
          <label className="dlg-check">
            <input type="radio" name="ms-radius" checked={options.diameter} onChange={() => set({ diameter: true })} />
            <span className="dlg-check-label">Diameter</span>
          </label>
        </div>
      </Fieldset>
      <Fieldset title="Display options">
        <Percent label="Marker size" value={options.markerSize} onChange={(markerSize) => set({ markerSize })} />
        <label className="dlg-check">
          <input type="checkbox" checked={options.scaleArrows} onChange={(e) => set({ scaleArrows: e.target.checked })} />
          <span className="dlg-check-label">Scale arrows when zooming</span>
        </label>
        <Percent label="Arrow size" value={options.arrowSize} onChange={(arrowSize) => set({ arrowSize })} />
      </Fieldset>
    </>
  );
}

function Fieldset({ title, children }: { title: string; children: ReactNode }) {
  return (
    <fieldset className="ms-fieldset">
      <legend className="ms-heading">{title}</legend>
      {children}
    </fieldset>
  );
}

function Percent({ label, value, onChange }: { label: string; value: number; onChange(v: number): void }) {
  return (
    <label className="ms-percent">
      <span>{label}</span>
      <input type="range" min={25} max={300} step={5} value={value} onChange={(e) => onChange(Number(e.target.value))} />
      <span className="ms-percent-value">{value}%</span>
    </label>
  );
}
