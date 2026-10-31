// Mesh compare's part of the results panel: the Distances and Bar chart tabs, and the controls both share (tolerance,
// colour gradient, edge highlights, transparency, measure points). The chart's bars filter the colours in the view.
import { useMemo, type ReactNode } from "react";
import { MousePointer2, Plus, Trash2 } from "lucide-react";
import { formatCount, formatFixed3 } from "../info";
import { compareColor, histogram, largestDistance } from "./colors";
import { SliderNumber } from "./AnalysisDialog";
import { setCompareView, useAnalysis } from "./session";
import type { CompareView, PartAnalysis } from "./types";

type Compare = Extract<PartAnalysis, { kind: "compare" }>;

const css = ([r, g, b]: [number, number, number]) => `rgb(${Math.round(r * 255)} ${Math.round(g * 255)} ${Math.round(b * 255)})`;

export function CompareTabs({ analysis, children }: { analysis: Compare; children: ReactNode }) {
  const view = useAnalysis((s) => s.compare);
  return (
    <>
      <div className="ms-tabs an-tabs" role="tablist">
        {(["distances", "chart"] as const).map((t) => (
          <button key={t} type="button" role="tab" aria-selected={view.tab === t} className={`ms-tab${view.tab === t ? " is-selected" : ""}`} onClick={() => setCompareView({ tab: t })}>
            {t === "distances" ? "Distances" : "Bar chart"}
          </button>
        ))}
      </div>
      {view.tab === "distances" ? children : <BarChart analysis={analysis} view={view} />}
      <CompareSettingsBox analysis={analysis} view={view} />
    </>
  );
}

function BarChart({ analysis, view }: { analysis: Compare; view: CompareView }) {
  const surfaces = analysis.drawing.surfaces;
  const h = useMemo(() => histogram(surfaces ?? []), [surfaces]);
  const most = Math.max(1, ...h.counts);
  const largest = largestDistance(analysis);
  const width = (h.hi - h.lo) / h.counts.length;
  if (!surfaces) return <p className="rpanel-muted">The distances of this comparison were not kept, so there is no chart. Run it again.</p>;
  const toggle = (i: number) => {
    const bars = view.bars ?? [];
    const next = bars.includes(i) ? bars.filter((b) => b !== i) : [...bars, i];
    setCompareView({ bars: next.length > 0 ? next : null });
  };
  return (
    <div className="an-chart">
      <div className="an-bars" role="group" aria-label="Distances: click bars to colour only those">
        {h.counts.map((n, i) => {
          const centre = h.lo + (i + 0.5) * width;
          const picked = !view.bars || view.bars.includes(i);
          return (
            <button
              key={i}
              type="button"
              className={`an-bar${picked ? "" : " is-off"}`}
              style={{ height: `${Math.max(2, (n / most) * 100)}%`, background: css(compareColor(centre, { ...view, bars: null }, largest)) }}
              aria-pressed={!!view.bars?.includes(i)}
              title={`${formatFixed3(h.lo + i * width)} to ${formatFixed3(h.lo + (i + 1) * width)} mm: ${formatCount(n)} points`}
              onClick={() => toggle(i)}
            />
          );
        })}
      </div>
      <div className="an-axis" aria-hidden="true">
        <span>{formatFixed3(h.lo)} mm</span>
        <span>{formatFixed3(h.hi)} mm</span>
      </div>
      <div className="an-row">
        <span className="rpanel-muted">{view.bars ? `${view.bars.length} of ${h.counts.length} bars coloured.` : "Click bars to colour only those distances."}</span>
        {view.bars && (
          <button type="button" className="dlg-btn" onClick={() => setCompareView({ bars: null })}>
            Show all
          </button>
        )}
      </div>
    </div>
  );
}

function CompareSettingsBox({ analysis, view }: { analysis: Compare; view: CompareView }) {
  const largest = largestDistance(analysis);
  const check = (key: "gradient" | "triangleEdges" | "toleranceEdges" | "transparent", label: string, help: string) => (
    <label className="dlg-check" title={help}>
      <input type="checkbox" checked={view[key]} onChange={(e) => setCompareView({ [key]: e.target.checked })} />
      <span className="dlg-check-label">{label}</span>
    </label>
  );
  return (
    <section className="dlg-section an-compare-box" aria-label="Display">
      <h3 className="dlg-section-title">Display</h3>
      <SliderNumber label="Tolerance" value={view.tolerance} min={0} max={1e4} sliderMax={Math.max(largest, 0.001)} step={Math.max(largest / 200, 0.0005)} unit="mm" onChange={(v) => setCompareView({ tolerance: v })} />
      <ul className="an-legend" aria-label="Colours in the view">
        <li>
          <span className="an-swatch" style={{ background: css(compareColor(0, view, largest)) }} aria-hidden="true" />
          Within ± tolerance
        </li>
        <li>
          <span className="an-swatch" style={{ background: css(compareColor(largest + 1, { ...view, gradient: false }, largest)) }} aria-hidden="true" />
          Outside
        </li>
        <li>
          <span className="an-swatch" style={{ background: css(compareColor(-largest - 1, { ...view, gradient: false }, largest)) }} aria-hidden="true" />
          Inside
        </li>
      </ul>
      {check("gradient", "Colour gradient", "Shade from green to red or blue with the distance, rather than three plain colours.")}
      {check("triangleEdges", "Highlight triangle edges", "Draw the triangles of the compared copies in black lines.")}
      {check("toleranceEdges", "Highlight edges beyond tolerance", "Draw in yellow the edges where the colour changes at the tolerance.")}
      {check("transparent", "Transparency", "See through the compared parts.")}
      <div className="an-row" role="group" aria-label="Measure points">
        {(
          [
            ["select", "Select", MousePointer2, "Clicks in the view do nothing here."],
            ["add", "Add measure point", Plus, "Click the compared part to read the distance there."],
            ["remove", "Remove measure point", Trash2, "Click near a measure point to remove it."],
          ] as const
        ).map(([mode, label, Icon, hint]) => (
          <button key={mode} type="button" className={`dlg-btn${view.probe === mode ? " is-pressed" : ""}`} aria-pressed={view.probe === mode} title={hint} onClick={() => setCompareView({ probe: mode })}>
            <Icon size={13} aria-hidden="true" /> {label}
          </button>
        ))}
      </div>
    </section>
  );
}
