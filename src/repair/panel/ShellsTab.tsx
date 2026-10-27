// Repair panel › Shells: one row per shell of the working copy (measured in Rust), sortable by any column. With
// Auto-select on, a click on a row selects that shell's triangles (Shift+click adds or removes them). A row shows as
// picked while every triangle of its shell is selected, however that came about. While the tab is in view the list
// follows the working copy: it is fetched again once a run that changed the mesh is over.
import { useCallback, useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { ArrowDown, ArrowUp, RefreshCw } from "lucide-react";
import { errorMessage } from "../../io/files";
import type { MeshData } from "../../store";
import { formatCount, formatNumber, yesNo } from "../format";
import { fetchShells } from "../ipc";
import { trianglesOfShell, type SelectMode } from "../selection";
import { selectTriangles } from "../session";
import { useRepair } from "../store";
import type { ShellRow } from "../types";
import { cx } from "./controls";

export type ShellColumn = "triangles" | "area" | "bboxVolume" | "volume" | "watertight";

export interface ShellSort {
  column: ShellColumn;
  descending: boolean;
}

/** Biggest shells first: the main body on top, specks at the bottom. */
export const DEFAULT_SORT: ShellSort = { column: "triangles", descending: true };

/** Rows drawn at most, so a mesh of loose triangles cannot stall the window; sorting reaches the others. */
export const MAX_ROWS = 2000;

export const COLUMNS: { id: ShellColumn; label: string; unit?: string; title: string }[] = [
  { id: "triangles", label: "Triangles", title: "Triangles" },
  { id: "area", label: "Area", unit: "cm²", title: "Area (cm²)" },
  { id: "bboxVolume", label: "Box volume", unit: "cm³", title: "Bounding box volume (cm³)" },
  { id: "volume", label: "Volume", unit: "cm³", title: "Volume (cm³); — for a shell that is not watertight" },
  // A soft hyphen lets the header break as "Water-tight" in a narrow panel.
  { id: "watertight", label: "Water­tight", title: "Watertight: every edge of the shell has exactly two triangles" },
];

function sortValue(row: ShellRow, column: ShellColumn): number | null {
  switch (column) {
    case "watertight":
      return row.watertight ? 1 : 0;
    case "volume":
      return row.volume;
    default:
      return row[column];
  }
}

/**
 * Row indices in display order. Shells without a volume (not watertight) go last when sorting by volume, either
 * way; ties keep Rust's order (by first triangle).
 */
export function sortShells(rows: readonly ShellRow[], sort: ShellSort): number[] {
  const sign = sort.descending ? -1 : 1;
  return rows
    .map((_, i) => i)
    .sort((a, b) => {
      const va = sortValue(rows[a], sort.column);
      const vb = sortValue(rows[b], sort.column);
      if (va === null || vb === null) return va === vb ? a - b : va === null ? 1 : -1;
      return va === vb ? a - b : (va < vb ? -1 : 1) * sign;
    });
}

/**
 * The sort after a click on a column's header: the same column turns around; another starts with the biggest values
 * first, or with No first for Watertight, so the shells that need attention come up.
 */
export function nextSort(sort: ShellSort, column: ShellColumn): ShellSort {
  if (sort.column === column) return { column, descending: !sort.descending };
  return { column, descending: column !== "watertight" };
}

/**
 * A cm² or cm³ value in a table cell: four significant digits (12.35, 120.0, 8,123), enough to compare shells and
 * narrow enough for five columns in the panel; 0.012 keeps three decimals. A positive value too small for three
 * decimals reads "<0.001" rather than a misleading 0.000.
 */
export function measureText(cm: number): string {
  if (cm > 0 && cm < 0.0005) return "<0.001";
  const size = Math.abs(cm);
  return formatNumber(cm, size < 9.9995 ? 3 : size < 99.995 ? 2 : size < 999.95 ? 1 : 0);
}

/** A row's cells, in column order (mm² and mm³ shown as cm² and cm³). */
export function shellCells(row: ShellRow): string[] {
  return [
    formatCount(row.triangles),
    measureText(row.area / 100),
    measureText(row.bboxVolume / 1000),
    row.volume === null ? "—" : measureText(row.volume / 1000),
    yesNo(row.watertight),
  ];
}

/** "27 shells, 3 not watertight". */
export function shellsSummary(rows: readonly ShellRow[]): string {
  const open = rows.filter((r) => !r.watertight).length;
  const shells = `${formatCount(rows.length)} ${rows.length === 1 ? "shell" : "shells"}`;
  return open === 0 ? shells : `${shells}, ${formatCount(open)} not watertight`;
}

const NO_ROWS: ReadonlySet<number> = new Set();

/**
 * The rows (of `rows`) whose every triangle is selected: `shellOf` is each triangle's row and `selection` the triangle
 * mask of the same working copy. The table marks exactly these picked, so it shows the selection as it is after Esc,
 * a click in the viewport, an undo or a run, not only after a click on a row.
 */
export function fullySelectedShells(shellOf: Uint32Array, selection: Uint8Array, rows: number): ReadonlySet<number> {
  const seen = new Uint8Array(rows);
  const missing = new Uint8Array(rows);
  for (let t = 0; t < shellOf.length; t++) {
    seen[shellOf[t]] = 1;
    if (!selection[t]) missing[shellOf[t]] = 1;
  }
  const picked = new Set<number>();
  for (let row = 0; row < rows; row++) if (seen[row] && !missing[row]) picked.add(row);
  return picked;
}

/**
 * How a click on a row (`picked`: marked picked) changes the selection: a plain click selects only its shell; Shift
 * (`toggle`) takes a picked row's shell out and puts any other's in, so the row turns the way the viewport does.
 */
export function rowSelectMode(picked: boolean, toggle: boolean): SelectMode {
  if (!toggle) return "replace";
  return picked ? "remove" : "add";
}

/**
 * What a key does in the table: Up, Down, Home and End pick the next row in display order (from before the first when
 * there is no cursor), and Enter or Space picks the cursor row again (Shift adds or removes it). A key on a column
 * header (`onTable` false) is the header's own: Enter and Space sort.
 */
export function tableKey(e: { key: string; shiftKey: boolean }, onTable: boolean, order: readonly number[], cursor: number | null): { row: number; toggle: boolean } | null {
  if (!onTable || order.length === 0) return null;
  if (e.key === "Enter" || e.key === " ") return cursor === null ? null : { row: cursor, toggle: e.shiftKey };
  const at = cursor === null ? -1 : order.indexOf(cursor);
  let to: number;
  if (e.key === "ArrowDown") to = Math.min(order.length - 1, at + 1);
  else if (e.key === "ArrowUp") to = Math.max(0, at - 1);
  else if (e.key === "Home") to = 0;
  else if (e.key === "End") to = order.length - 1;
  else return null;
  return { row: order[to], toggle: false };
}

/** The shells of one mesh: Rust's rows and the row of each triangle. */
interface ShellList {
  mesh: MeshData;
  rows: ShellRow[];
  shellOf: Uint32Array;
}

export function ShellsTab({ active }: { active: boolean }) {
  const mesh = useRepair((s) => s.session?.mesh ?? null);
  const selection = useRepair((s) => s.session?.selection ?? null);
  const selectedCount = useRepair((s) => s.session?.selectedCount ?? 0);
  const busy = useRepair((s) => s.busy !== null);
  const autoSelect = useRepair((s) => s.shellsAutoSelect);
  const [list, setList] = useState<ShellList | null>(null);
  const [loading, setLoading] = useState<MeshData | null>(null);
  const [failure, setFailure] = useState<{ mesh: MeshData; message: string } | null>(null);
  const [sort, setSort] = useState(DEFAULT_SORT);
  const [cursor, setCursor] = useState<number | null>(null);
  const request = useRef(0);
  /** The mesh being fetched, also seen by an effect that runs again before the loading state has rendered. */
  const fetching = useRef<MeshData | null>(null);
  const desktop = isTauri();

  const load = useCallback((m: MeshData) => {
    const id = ++request.current;
    fetching.current = m;
    setLoading(m);
    setFailure(null);
    fetchShells(m)
      .then(
        ({ rows, shellOf }) => {
          if (request.current !== id) return;
          setList({ mesh: m, rows, shellOf });
          setCursor(null);
        },
        (err: unknown) => {
          if (request.current === id) setFailure({ mesh: m, message: errorMessage(err, "Could not list the shells") });
        },
      )
      .finally(() => {
        if (request.current !== id) return;
        fetching.current = null;
        setLoading(null);
      });
  }, []);

  // In view, the list follows the working copy, once each run is over; a failed fetch waits for Refresh.
  useEffect(() => {
    if (!active || !mesh || busy || !desktop) return;
    if (list?.mesh === mesh || fetching.current === mesh || failure?.mesh === mesh) return;
    load(mesh);
  }, [active, mesh, busy, desktop, list, failure, load]);

  const order = useMemo(() => (list ? sortShells(list.rows, sort) : []), [list, sort]);
  const shown = order.length > MAX_ROWS ? order.slice(0, MAX_ROWS) : order;
  const current = list !== null && list.mesh === mesh;
  // Worked out from the selection, never kept beside it: the rows can then not drift from what the viewport shows.
  // Only while the tab is in view, since each selection change costs a pass over the triangles.
  const picked = useMemo(
    () => (active && list && list.mesh === mesh && selection && selectedCount > 0 ? fullySelectedShells(list.shellOf, selection, list.rows.length) : NO_ROWS),
    [active, list, mesh, selection, selectedCount],
  );

  function pick(row: number, toggle: boolean) {
    if (!list || !current) return;
    setCursor(row);
    if (autoSelect) selectTriangles(trianglesOfShell(list.shellOf, row), rowSelectMode(picked.has(row), toggle));
  }

  let note: string;
  if (!desktop) note = "The shell list needs the tricky3D desktop app.";
  else if (failure && failure.mesh === mesh) note = failure.message;
  else if (loading) note = "Listing the shells…";
  else if (!list) note = "";
  else if (!current) note = busy ? "The working copy is changing; the list follows when the repair is done." : "Listing the shells…";
  else note = shellsSummary(list.rows) + (order.length > MAX_ROWS ? `. The first ${formatCount(MAX_ROWS)} in this order are listed.` : ".");

  return (
    <div className="rp-body">
      <div className="rp-row">
        <label className="clip-check" title="Clicking a row selects that shell's triangles">
          <input type="checkbox" checked={autoSelect} onChange={(e) => useRepair.getState().setShellsAutoSelect(e.target.checked)} />
          Auto-select
        </label>
        <button
          type="button"
          className="clip-btn rp-push"
          title="List the shells of the working copy again"
          disabled={!desktop || !mesh || loading !== null}
          onClick={() => mesh && load(mesh)}
        >
          <RefreshCw size={13} aria-hidden="true" />
          Refresh
        </button>
      </div>
      {note && <p className="rpanel-muted">{note}</p>}
      {list && list.rows.length > 0 && (
        <ShellTable
          rows={list.rows}
          order={shown}
          sort={sort}
          picked={picked}
          cursor={cursor}
          stale={!current}
          onSort={(column) => setSort(nextSort(sort, column))}
          onPick={pick}
        />
      )}
    </div>
  );
}

interface ShellTableProps {
  rows: readonly ShellRow[];
  /** The rows to draw (indices into `rows`), in display order. */
  order: readonly number[];
  sort: ShellSort;
  /** The rows whose every triangle is selected (fullySelectedShells). */
  picked: ReadonlySet<number>;
  /** The row the keyboard is on. */
  cursor: number | null;
  /** The rows belong to an earlier working copy: drawn faded until the list is fetched again. */
  stale: boolean;
  onSort(column: ShellColumn): void;
  onPick(row: number, toggle: boolean): void;
}

/**
 * The shells as a table in a box of its own that scrolls both ways (big parts have wide numbers), the header staying
 * in view: one header per column, which sorts by it, and one row per shell. With the table focused, Up, Down, Home
 * and End pick the next row, and Enter or Space picks the row again (Shift adds or removes it); Tab goes on to the
 * column headers, which sort with Enter or Space.
 */
export function ShellTable({ rows, order, sort, picked, cursor, stale, onSort, onPick }: ShellTableProps) {
  const baseId = useId();
  const rowId = (row: number) => `${baseId}-row-${row}`;

  function onKeyDown(e: KeyboardEvent<HTMLTableElement>) {
    const action = tableKey(e, e.target === e.currentTarget, order, cursor);
    if (!action) return;
    e.preventDefault();
    onPick(action.row, action.toggle);
    document.getElementById(rowId(action.row))?.scrollIntoView({ block: "nearest" });
  }

  return (
    <div className="rp-table-box">
      <table
        className={cx("rp-shells", stale && "is-stale")}
        role="grid"
        aria-label="Shells"
        aria-rowcount={rows.length + 1}
        aria-activedescendant={cursor !== null ? rowId(cursor) : undefined}
        tabIndex={0}
        onKeyDown={onKeyDown}
      >
        <thead>
          <tr>
            {COLUMNS.map((c) => {
              const sorted = sort.column === c.id;
              const Arrow = sort.descending ? ArrowDown : ArrowUp;
              return (
                <th key={c.id} scope="col" aria-sort={sorted ? (sort.descending ? "descending" : "ascending") : "none"}>
                  <button type="button" title={`${c.title}. Click to sort.`} onClick={() => onSort(c.id)}>
                    <span className="rp-th-label">{c.label}</span>
                    {/* The arrow goes on the unit line, so sorting does not widen the column. */}
                    <span className="rp-th-unit">
                      {sorted && <Arrow size={10} aria-hidden="true" />}
                      {c.unit}
                    </span>
                  </button>
                </th>
              );
            })}
          </tr>
        </thead>
        <tbody>
          {order.map((row) => (
            <tr
              key={row}
              id={rowId(row)}
              aria-selected={picked.has(row)}
              className={cx(picked.has(row) && "is-picked", cursor === row && "is-cursor", !rows[row].watertight && "is-open")}
              onClick={(e) => onPick(row, e.shiftKey)}
            >
              {shellCells(rows[row]).map((text, k) => (
                <td key={COLUMNS[k].id}>{text}</td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
