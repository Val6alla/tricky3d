// The right panel while an analysis is on show: its results as tables (as the reference lays them out, a header row
// over the quantities), what the colours in the view mean, Copy and Export to CSV, and Run again and Close.
import { ClipboardCopy, FileDown, RotateCw, TriangleAlert } from "lucide-react";
import { useStore } from "../store";
import { ANALYSIS_COLORS, legendOf, wallColor } from "./colors";
import { CompareTabs } from "./CompareControls";
import { resultTables } from "./rows";
import { closeAnalysisView, copyResults, exportResults, isOutOfDate, rerunAnalysis, shownAnalysis, useAnalysis } from "./session";
import { KIND_LABELS, type PartAnalysis } from "./types";
import "./analysis.css";

/** A short note under the results, for the kinds whose numbers need one. */
const NOTES: Partial<Record<string, string>> = {
  support: "An estimate: solid blocks from every surface that needs support straight down to the platform. Real supports are lighter.",
  shadow: "The part seen from straight above, where it covers the platform; overlaps count once.",
  gravity: "For a part of even density. The cross in the view marks the centre.",
  wall: "Measured straight in from each triangle against its outward normal, as Wall at a point measures. The pink line marks the thinnest wall.",
  compare: "Each point of one surface to the nearest point of the other: above is outside the other part, below inside it.",
  removability:
    "Each body of the part (its shells, joined where they touch, with any cavity inside) is lifted straight up alone. A body that would hit another is trapped.",
};

function Tables({ analysis }: { analysis: PartAnalysis }) {
  return (
    <>
      {resultTables(analysis).map((table, i) => (
        <table key={i} className="an-table">
          <thead>
            <tr>
              {table.columns.map((c, k) => (
                <th key={k} scope="col">
                  {c}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {table.rows.map((row) => (
              <tr key={row[0]}>
                <th scope="row">{row[0]}</th>
                {row.slice(1).map((cell, k) => (
                  <td key={k}>{cell}</td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      ))}
    </>
  );
}

/** Wall thickness: passed or failed in big letters, and the gradient's key. */
function WallSummary({ analysis }: { analysis: Extract<PartAnalysis, { kind: "wall" }> }) {
  const r = analysis.result;
  const s = analysis.settings;
  const css = (v: number) => {
    const [red, green, blue] = wallColor(v, s);
    return `rgb(${Math.round(red * 255)} ${Math.round(green * 255)} ${Math.round(blue * 255)})`;
  };
  return (
    <>
      <p className={`an-verdict ${r.passed ? "is-pass" : "is-fail"}`} role="status">
        {r.passed ? "Test passed" : "Test failed"}
      </p>
      {!r.complete && <p className="an-stale">Stopped as soon as the test failed, so the colours and the share are incomplete.</p>}
      {s.colors !== "red-green" && (
        <div className="an-gradient" aria-label="Colours in the view">
          <span className="an-gradient-bar" style={{ background: `linear-gradient(to right, ${css(0)}, ${css(s.threshold)}, ${css(2 * s.threshold)})` }} />
          <span className="an-gradient-labels">
            <span>0 mm</span>
            <span>{s.threshold} mm</span>
            <span>{2 * s.threshold} mm or more</span>
          </span>
        </div>
      )}
    </>
  );
}

export function AnalysisPanel() {
  const shown = useAnalysis((a) => a.shown);
  const parts = useStore((s) => s.parts);
  const found = shownAnalysis(shown, parts);
  if (!found) return null;
  const { part, analysis } = found;
  const stale = isOutOfDate(part, analysis);
  const legend = legendOf(analysis);
  const note = NOTES[analysis.kind];
  return (
    <section className="an-panel" aria-label={KIND_LABELS[analysis.kind]}>
      <div className="an-body">
        <h2 className="an-title">
          {KIND_LABELS[analysis.kind]}
          <span className="an-part" title={part.name}>
            {part.name}
          </span>
        </h2>
        {stale && (
          <p className="an-stale" role="status">
            <TriangleAlert size={13} aria-hidden="true" /> The part has moved or turned since this ran, so these results are out of date.
          </p>
        )}
        {analysis.kind === "wall" && <WallSummary analysis={analysis} />}
        {analysis.kind === "compare" ? (
          <CompareTabs analysis={analysis}>
            <Tables analysis={analysis} />
          </CompareTabs>
        ) : (
          <Tables analysis={analysis} />
        )}
        {legend.length > 0 && (
          <ul className="an-legend" aria-label="Colours in the view">
            {legend.map(({ cls, label }) => (
              <li key={label}>
                <span className="an-swatch" style={{ background: ANALYSIS_COLORS[cls] }} aria-hidden="true" />
                {label}
              </li>
            ))}
          </ul>
        )}
        {note && <p className="rpanel-muted">{note}</p>}
        <div className="an-row">
          <button type="button" className="dlg-btn" onClick={() => void copyResults(part, analysis)} title="Copy the results as a table, to paste into a spreadsheet.">
            <ClipboardCopy size={13} aria-hidden="true" /> Copy
          </button>
          <button type="button" className="dlg-btn" onClick={() => void exportResults(part, analysis)} title="Save the results as a CSV file.">
            <FileDown size={13} aria-hidden="true" /> Export to CSV
          </button>
        </div>
      </div>
      <div className="an-actions">
        <button type="button" className="dlg-btn" onClick={() => rerunAnalysis(part.id, analysis.id)} title="Run the analysis again on the part as it is now, with the same settings.">
          <RotateCw size={13} aria-hidden="true" /> Run again
        </button>
        <button type="button" className="dlg-btn" onClick={closeAnalysisView} title="Leave the analysis; it stays in the parts tree.">
          Close
        </button>
      </div>
    </section>
  );
}
