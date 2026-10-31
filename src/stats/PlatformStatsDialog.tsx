// Home › Platform statistics: the "Platform overview" dialog. Rust measures every part when the dialog opens (with a
// progress bar and Cancel); the dialog lays the numbers out (src/stats/format.ts): a sortable table of the parts, the
// parts' box, the platform's totals and packing density for all parts or only the selected ones, and Export data to a
// CSV file or the clipboard.
import { useEffect, useMemo, useState } from "react";
import { ArrowDown, ArrowUp } from "lucide-react";
import { Dialog } from "../dialogs/Dialog";
import { formatFixed2, formatFixed3 } from "../info";
import { errorMessage, withExtension } from "../io/files";
import { copyText } from "../io/clipboard";
import { pickSaveFile } from "../io/ipc";
import { CANCELLED, cancelRepair } from "../repair/ipc";
import { useStore, type Vec3 } from "../store";
import { toast } from "../ui";
import {
  COLUMNS,
  EXPORT_LAYOUTS,
  cellText,
  exportCells,
  percentText,
  sortRows,
  tableRows,
  toCsv,
  toTsv,
  type ColumnKey,
  type ExportLayout,
  type SortDirection,
  type TableRow,
} from "./format";
import { exportText, fetchPlatformStats, type StatsReply, type StatsSummary } from "./ipc";
import "./stats.css";
import { nextJob } from "../jobs";

type Scope = "all" | "selected";

/** What the dialog measured: the parts in list order, which of them were selected, and Rust's answer. */
interface Measured {
  parts: { id: string; name: string; index: number }[];
  selected: Set<string>;
  platform: Vec3;
  reply: StatsReply;
}

type Phase = { kind: "measuring"; job: number; progress: number } | { kind: "ready"; measured: Measured } | { kind: "failed"; message: string };

export function PlatformStatsDialog({ onClose }: { onClose(): void }) {
  const [phase, setPhase] = useState<Phase>(() => ({ kind: "measuring", job: nextJob(), progress: 0 }));
  const job = phase.kind === "measuring" ? phase.job : null;

  useEffect(() => {
    if (job === null) return;
    const s = useStore.getState();
    const parts = s.parts.map((p, index) => ({ id: p.id, name: p.name, index }));
    const selected = new Set(s.selection);
    const platform: Vec3 = [...s.platform];
    let live = true;
    fetchPlatformStats(
      s.parts.map((p) => ({ name: p.name, mesh: p.mesh, matrix: p.matrix })),
      platform,
      parts.filter((p) => selected.has(p.id)).map((p) => p.index),
      job,
      (progress) => live && setPhase((ph) => (ph.kind === "measuring" && ph.job === job ? { ...ph, progress } : ph)),
    )
      .then((reply) => live && setPhase({ kind: "ready", measured: { parts, selected, platform, reply } }))
      .catch((err: unknown) => {
        if (live && err !== CANCELLED) setPhase({ kind: "failed", message: errorMessage(err, "Could not work out the platform statistics") });
      });
    return () => {
      live = false;
      // Closed while measuring: Rust stops too.
      cancelRepair(job).catch(() => {});
    };
  }, [job]);

  return (
    <Dialog title="Platform overview" onClose={onClose} className="dlg-stats">
      {phase.kind === "measuring" && <Measuring progress={phase.progress} onCancel={onClose} />}
      {phase.kind === "failed" && (
        <p className="dlg-error" role="alert">
          {phase.message}
        </p>
      )}
      {phase.kind === "ready" && <Overview measured={phase.measured} />}
    </Dialog>
  );
}

function Measuring({ progress, onCancel }: { progress: number; onCancel(): void }) {
  const percent = Math.round(Math.min(1, Math.max(0, progress)) * 100);
  return (
    <div className="stats-measuring" role="status">
      <p className="dlg-text">Measuring the parts…</p>
      <div className="stats-progress" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent}>
        <div className="stats-progress-fill" style={{ width: `${percent}%` }} />
      </div>
      <div>
        <button type="button" className="dlg-btn" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </div>
  );
}

function Overview({ measured }: { measured: Measured }) {
  const { parts, selected, platform, reply } = measured;
  // Only selected parts, unless none was selected when the dialog opened.
  const [scope, setScope] = useState<Scope>(selected.size > 0 ? "selected" : "all");
  const [sort, setSort] = useState<{ key: ColumnKey; direction: SortDirection }>({ key: "id", direction: "asc" });
  const [layout, setLayout] = useState<ExportLayout>("table");
  const [namesOnly, setNamesOnly] = useState(false);

  const allRows = useMemo(() => tableRows(parts, reply.parts), [parts, reply]);
  const rows = useMemo(() => {
    const chosen = scope === "all" ? allRows : allRows.filter((r) => selected.has(r.partId));
    return sortRows(chosen, sort.key, sort.direction);
  }, [allRows, scope, selected, sort]);
  const summary = scope === "all" ? reply.all : reply.selected;

  function sortBy(key: ColumnKey) {
    setSort((s) => (s.key === key ? { key, direction: s.direction === "asc" ? "desc" : "asc" } : { key, direction: "asc" }));
  }

  const cells = () => exportCells({ layout, namesOnly }, rows, summary, platform);

  async function exportCsv() {
    const s = useStore.getState();
    let path: string | null;
    try {
      path = await pickSaveFile({
        title: "Export platform statistics",
        defaultPath: `${s.projectName} statistics.csv`,
        filters: [{ name: "CSV (*.csv)", extensions: ["csv"] }],
      });
    } catch (err) {
      toast(errorMessage(err, "Could not show the save dialog"), "error");
      return;
    }
    if (!path) return;
    const target = withExtension(path, "csv");
    try {
      await exportText(target, toCsv(cells()));
      toast(`Saved ${target.split(/[\\/]/).pop()}.`);
    } catch (err) {
      toast(errorMessage(err, "Could not write the CSV file"), "error");
    }
  }

  async function copy() {
    try {
      await copyText(toTsv(cells()));
      toast("Copied the platform statistics to the clipboard.");
    } catch (err) {
      toast(errorMessage(err, "Could not copy to the clipboard"), "error");
    }
  }

  return (
    <>
      <div className="stats-table-wrap">
        <table className="stats-table">
          <thead>
            <tr>
              {COLUMNS.map((c) => (
                <th key={c.key} className={c.numeric ? "is-numeric" : undefined} aria-sort={sort.key === c.key ? (sort.direction === "asc" ? "ascending" : "descending") : undefined}>
                  <button type="button" className="stats-sort" onClick={() => sortBy(c.key)}>
                    {c.label}
                    {sort.key === c.key && (sort.direction === "asc" ? <ArrowUp size={11} aria-hidden /> : <ArrowDown size={11} aria-hidden />)}
                  </button>
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.length === 0 && (
              <tr>
                <td colSpan={COLUMNS.length} className="stats-empty">
                  No part is selected.
                </td>
              </tr>
            )}
            {rows.map((row) => (
              <TableLine key={row.partId} row={row} />
            ))}
          </tbody>
        </table>
      </div>

      <div className="stats-info">
        <section className="dlg-section">
          <h3 className="dlg-section-title">Part positions</h3>
          <Positions summary={summary} />
        </section>
        <section className="dlg-section">
          <h3 className="dlg-section-title">Platform</h3>
          <dl className="stats-platform">
            <dt>Platform size</dt>
            <dd>{platform.map((v) => formatFixed2(v)).join(" × ")} mm</dd>
            <dt>Volume</dt>
            <dd>{summary.volume === null ? "—" : `${formatFixed3(summary.volume / 1000)} cm³`}</dd>
            <dt>Area</dt>
            <dd>{formatFixed3(summary.area / 100)} cm²</dd>
            <dt>Packing density</dt>
            <dd>{percentText(summary.packingDensity)}</dd>
            <dt>With object height</dt>
            <dd>{percentText(summary.withObjectHeight)}</dd>
          </dl>
          {summary.onPlatform < summary.parts && (
            <p className="dlg-note">
              {summary.parts - summary.onPlatform} of {summary.parts} parts stick out of the platform and are not counted in the totals.
            </p>
          )}
          {summary.volume === null && <p className="dlg-note">A part has holes, so its volume and the densities are unknown. Repair it first.</p>}
          <div className="stats-scope" role="radiogroup" aria-label="Parts to count">
            <label className="dlg-check">
              <input type="radio" name="stats-scope" checked={scope === "all"} onChange={() => setScope("all")} />
              <span className="dlg-check-label">Calculate all parts</span>
            </label>
            <label className="dlg-check">
              <input type="radio" name="stats-scope" checked={scope === "selected"} onChange={() => setScope("selected")} />
              <span className="dlg-check-label">Only selected parts</span>
            </label>
          </div>
        </section>
      </div>

      <section className="dlg-section stats-export">
        <h3 className="dlg-section-title">Export data</h3>
        <div className="stats-export-row">
          <select className="dlg-input dlg-select" aria-label="Export layout" value={layout} onChange={(e) => setLayout(e.target.value as ExportLayout)}>
            {EXPORT_LAYOUTS.map((l) => (
              <option key={l.id} value={l.id}>
                {l.label}
              </option>
            ))}
          </select>
          <label className="dlg-check">
            <input type="checkbox" checked={namesOnly} disabled={layout === "summary"} onChange={(e) => setNamesOnly(e.target.checked)} />
            <span className="dlg-check-label">Export filenames only</span>
          </label>
        </div>
        <div className="stats-export-row">
          <button type="button" className="dlg-btn" onClick={() => void exportCsv()}>
            Export to CSV
          </button>
          <button type="button" className="dlg-btn" onClick={() => void copy()}>
            Copy to clipboard
          </button>
        </div>
      </section>
    </>
  );
}

function TableLine({ row }: { row: TableRow }) {
  return (
    <tr>
      {COLUMNS.map((c) => (
        <td key={c.key} className={[c.numeric && "is-numeric", c.key === "status" && !row.valid && "is-problem"].filter(Boolean).join(" ") || undefined} title={c.key === "name" || c.key === "status" ? cellText(row, c.key) : undefined}>
          {cellText(row, c.key)}
        </td>
      ))}
    </tr>
  );
}

function Positions({ summary }: { summary: StatsSummary }) {
  const value = (v: Vec3 | null, k: number) => (v ? formatFixed2(v[k]) : "—");
  const size = (k: number) => (summary.min && summary.max ? formatFixed2(summary.max[k] - summary.min[k]) : "—");
  const lines: [string, (k: number) => string][] = [
    ["Minimum", (k) => value(summary.min, k)],
    ["Maximum", (k) => value(summary.max, k)],
    ["Size", size],
  ];
  return (
    <table className="stats-positions">
      <thead>
        <tr>
          <th />
          <th>X (mm)</th>
          <th>Y (mm)</th>
          <th>Z (mm)</th>
        </tr>
      </thead>
      <tbody>
        {lines.map(([label, of]) => (
          <tr key={label}>
            <th scope="row">{label}</th>
            {[0, 1, 2].map((k) => (
              <td key={k}>{of(k)}</td>
            ))}
          </tr>
        ))}
      </tbody>
    </table>
  );
}
