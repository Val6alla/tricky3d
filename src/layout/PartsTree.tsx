import { memo, useMemo, useRef, useState, useEffect, type KeyboardEvent, type MouseEvent, type RefObject } from "react";
import type { LucideIcon } from "lucide-react";
import { Boxes, ChevronDown, ChevronRight, Eye, EyeOff, Grid3x3, Layers, Lock, LockOpen, Ruler, TriangleAlert, Wrench, X } from "lucide-react";
import { deleteMeasurement, isMeasureOpen, openMeasure, selectMeasurement } from "../measure/session";
import { useMeasure } from "../measure/store";
import { toolById } from "../measure/tools";
import type { Measurement } from "../measure/types";
import { badgeText, cachedCheck, useBadges } from "../repair/badges";
import { enterRepair } from "../repair/session";
import { useRepair } from "../repair/store";
import { selectMode, useStore, type Part } from "../store";
import { toast, useUi } from "../ui";

const PARTS_KEY = "root:parts";
const SLICES_KEY = "root:slices";
const LATTICES_KEY = "root:lattices";
/** The "Part repair" row's key is this followed by the repaired part's id. */
const REPAIR_KEY = "repair:";
/** A "Measurement" row's key is this followed by the measurement's id. */
const MEASURE_KEY = "measure:";

/** `.ptree-row` height and `.ptree` top padding in layout.css. */
const ROW_HEIGHT = 26;
const TREE_PADDING = 6;
/** List rows drawn beyond each edge of the view, so the arrow keys always find a drawn neighbour to focus. */
const OVERSCAN = 20;

/**
 * The list rows [first, end) a tree scrolled to `scrollTop` and `height` px tall needs to draw, out of `count`
 * (rowCount). Only those are rendered: Split shells can make thousands of parts, and drawing a row for each
 * blocked the window for seconds. List rows start below the Parts row.
 */
export function visibleRange(count: number, scrollTop: number, height: number): [number, number] {
  const top = scrollTop - TREE_PADDING - ROW_HEIGHT;
  const first = Math.min(count, Math.max(0, Math.floor(top / ROW_HEIGHT) - OVERSCAN));
  const end = Math.min(count, Math.max(0, Math.ceil((top + height) / ROW_HEIGHT) + OVERSCAN));
  return [first, Math.max(first, end)];
}

/**
 * A row of the part list: the part at `index`, ("repair") the "Part repair" row under that part, or ("measurement") its
 * measurement number `measure`.
 */
export interface ListRow {
  kind: "part" | "repair" | "measurement";
  index: number;
  measure?: number;
}

/**
 * How many rows the part list has: one per part, plus "Part repair" while the part at `repairAt` is repaired (-1:
 * none), plus a row per measurement (`measured[i]` under part i).
 */
export function rowCount(parts: number, repairAt: number, measured: readonly number[] = []): number {
  const extra = measured.reduce((n, m) => n + m, 0);
  return (repairAt >= 0 && repairAt < parts ? parts + 1 : parts) + extra;
}

/**
 * The list rows [first, end) as parts, the "Part repair" row, which follows the part at `repairAt` (-1: none), and
 * each part's measurement rows after it (and after its "Part repair" row). Every row keeps the one height
 * visibleRange counts with.
 */
export function rowsInRange(parts: number, repairAt: number, first: number, end: number, measured: readonly number[] = []): ListRow[] {
  if (measured.some((m) => m > 0)) {
    const rows: ListRow[] = [];
    let r = 0;
    for (let i = 0; i < parts && r < end; i++) {
      const block: ListRow[] = [{ kind: "part", index: i }];
      if (i === repairAt) block.push({ kind: "repair", index: i });
      for (let k = 0; k < (measured[i] ?? 0); k++) block.push({ kind: "measurement", index: i, measure: k });
      for (const row of block) {
        if (r >= first && r < end) rows.push(row);
        r++;
      }
    }
    return rows;
  }
  const count = rowCount(parts, repairAt);
  const extra = count > parts;
  const rows: ListRow[] = [];
  for (let r = Math.max(0, first); r < Math.min(end, count); r++) {
    if (!extra || r <= repairAt) rows.push({ kind: "part", index: r });
    else if (r === repairAt + 1) rows.push({ kind: "repair", index: repairAt });
    else rows.push({ kind: "part", index: r - 1 });
  }
  return rows;
}

/** The tree's scroll position and height, followed as it scrolls and resizes. */
function useScrollView(ref: RefObject<HTMLDivElement | null>) {
  // Before the first measurement assume the window's height, so a short list draws whole at once.
  const [view, setView] = useState(() => ({ top: 0, height: typeof window === "undefined" ? 800 : window.innerHeight }));
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const update = () =>
      setView((v) => (v.top === el.scrollTop && v.height === el.clientHeight ? v : { top: el.scrollTop, height: el.clientHeight }));
    update();
    el.addEventListener("scroll", update, { passive: true });
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(update);
    observer?.observe(el);
    return () => {
      el.removeEventListener("scroll", update);
      observer?.disconnect();
    };
  }, [ref]);
  return view;
}

/** Moves keyboard focus to the previous or next focusable row of the tree. */
function focusSibling(row: HTMLElement, step: 1 | -1) {
  const tree = row.closest('[role="tree"]');
  if (!tree) return;
  const rows = Array.from(tree.querySelectorAll<HTMLElement>('[role="treeitem"]:not([aria-disabled="true"])'));
  rows[rows.indexOf(row) + step]?.focus();
}

const stop = (e: MouseEvent) => e.stopPropagation();

// Memoised so a splitter drag, which re-renders the Sidebar on every move, skips the whole tree.
export const PartsTree = memo(function PartsTree() {
  const parts = useStore((s) => s.parts);
  const selection = useStore((s) => s.selection);
  const hideSoon = useStore((s) => s.hideSoon);
  const repairId = useRepair((s) => s.session?.partId ?? null);
  // The damage badges come from a cache outside React; each check result bumps this version and redraws the rows.
  useBadges((s) => s.version);
  const [expanded, setExpanded] = useState(true);
  const [editingId, setEditingId] = useState<string | null>(null);
  // Roving tabindex: only one row is in the Tab order; arrow keys move between rows.
  const [activeKey, setActiveKey] = useState(PARTS_KEY);
  const treeRef = useRef<HTMLDivElement>(null);
  const view = useScrollView(treeRef);

  const selected = useMemo(() => new Set(selection), [selection]);
  const repairAt = useMemo(() => (repairId === null ? -1 : parts.findIndex((p) => p.id === repairId)), [parts, repairId]);
  const measured = useMemo(() => parts.map((p) => p.measurements?.length ?? 0), [parts]);
  const rows = rowCount(parts.length, repairAt, measured);
  const [first, end] = expanded ? visibleRange(rows, view.top, view.height) : [0, 0];
  const drawn = rowsInRange(parts.length, repairAt, first, end, measured);
  const keyOf = (row: ListRow) =>
    row.kind === "part"
      ? parts[row.index].id
      : row.kind === "repair"
        ? REPAIR_KEY + parts[row.index].id
        : MEASURE_KEY + (parts[row.index].measurements?.[row.measure ?? 0]?.id ?? "");
  const rowKeys = [PARTS_KEY, ...drawn.map(keyOf), ...(hideSoon ? [] : [SLICES_KEY, LATTICES_KEY])];
  // A focused row scrolled out of the drawn range is gone, so Tab enters the tree at the Parts row instead.
  const tabKey = rowKeys.includes(activeKey) ? activeKey : PARTS_KEY;

  function onTreeKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    const row = e.target as HTMLElement;
    if (row.getAttribute("role") !== "treeitem") return;
    e.preventDefault();
    focusSibling(row, e.key === "ArrowDown" ? 1 : -1);
  }

  return (
    <div ref={treeRef} className="ptree" role="tree" aria-label="Scene" aria-multiselectable="true" onKeyDown={onTreeKeyDown}>
      <div
        className="ptree-row ptree-root"
        role="treeitem"
        aria-level={1}
        aria-expanded={expanded}
        tabIndex={tabKey === PARTS_KEY ? 0 : -1}
        onFocus={() => setActiveKey(PARTS_KEY)}
        onClick={() => setExpanded((v) => !v)}
        onKeyDown={(e) => {
          if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
            e.preventDefault();
            setExpanded(e.key === "ArrowRight");
          } else if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            setExpanded((v) => !v);
          }
        }}
      >
        <span className="ptree-chevron" aria-hidden="true">
          {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        </span>
        <Boxes size={14} className="ptree-root-icon" />
        <span className="ptree-label">Parts</span>
        <span className="ptree-count" aria-label={`${parts.length} parts`}>
          {parts.length}
        </span>
      </div>

      {expanded && parts.length === 0 && (
        <div className="ptree-empty" role="treeitem" aria-level={2} aria-disabled="true">
          No parts yet. Use File › Import part…, or drop mesh files on the view.
        </div>
      )}
      {first > 0 && <div role="presentation" style={{ height: first * ROW_HEIGHT }} />}
      {expanded &&
        drawn.map((row) => {
          const part = parts[row.index];
          const key = keyOf(row);
          if (row.kind === "measurement") {
            const measurement = part.measurements?.[row.measure ?? 0];
            return measurement ? (
              <MeasurementRow key={key} part={part} measurement={measurement} number={(row.measure ?? 0) + 1} tabbable={tabKey === key} onFocus={() => setActiveKey(key)} />
            ) : null;
          }
          return row.kind === "repair" ? (
            <RepairRow key={key} part={part} tabbable={tabKey === key} onFocus={() => setActiveKey(key)} />
          ) : (
            <PartRow
              key={key}
              part={part}
              position={row.index + 1}
              count={parts.length}
              selected={selected.has(part.id)}
              repairing={row.index === repairAt}
              editing={editingId === part.id}
              tabbable={tabKey === key}
              onFocus={() => setActiveKey(key)}
              onStartRename={() => setEditingId(part.id)}
              onEndRename={() => setEditingId(null)}
            />
          );
        })}
      {end < (expanded ? rows : 0) && <div role="presentation" style={{ height: (rows - end) * ROW_HEIGHT }} />}

      {!hideSoon && (
        <>
          <SoonRoot
            rowKey={SLICES_KEY}
            label="Slices"
            icon={Layers}
            message="Slices are coming in a future version."
            tabbable={tabKey === SLICES_KEY}
            onFocus={setActiveKey}
          />
          <SoonRoot
            rowKey={LATTICES_KEY}
            label="Lattices"
            icon={Grid3x3}
            message="Lattices are coming in a future version."
            tabbable={tabKey === LATTICES_KEY}
            onFocus={setActiveKey}
          />
        </>
      )}
    </div>
  );
});

interface SoonRootProps {
  rowKey: string;
  label: string;
  icon: LucideIcon;
  message: string;
  tabbable: boolean;
  onFocus(key: string): void;
}

function SoonRoot({ rowKey, label, icon: Icon, message, tabbable, onFocus }: SoonRootProps) {
  return (
    <div
      className="ptree-row ptree-root is-soon"
      role="treeitem"
      aria-level={1}
      tabIndex={tabbable ? 0 : -1}
      title={message}
      onFocus={() => onFocus(rowKey)}
      onClick={() => toast(message)}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          toast(message);
        }
      }}
    >
      <span className="ptree-chevron" aria-hidden="true" />
      <Icon size={14} className="ptree-root-icon" />
      <span className="ptree-label">{label}</span>
      <span className="soon-pill">Soon</span>
    </div>
  );
}

export interface PartRowProps {
  part: Part;
  /** 1-based place in the list, and the list's length: only some rows are drawn, so screen readers are told. */
  position: number;
  count: number;
  selected: boolean;
  /** The part is in repair mode: its "Part repair" row follows. */
  repairing: boolean;
  editing: boolean;
  tabbable: boolean;
  onFocus(): void;
  onStartRename(): void;
  onEndRename(): void;
}

/** A part's row: visibility, colour, name (double-click or F2 renames), damage badge and lock. */
export function PartRow({ part, position, count, selected, repairing, editing, tabbable, onFocus, onStartRename, onEndRename }: PartRowProps) {
  const rowRef = useRef<HTMLDivElement>(null);
  const { select, setPartsVisible, setPartLocked, renamePart } = useStore.getState();
  const check = cachedCheck(part.mesh);
  const badge = check ? badgeText(check) : null;

  function onContextMenu(e: MouseEvent) {
    e.preventDefault();
    if (!useStore.getState().selection.includes(part.id)) select([part.id], "replace");
    useUi.getState().openContextMenu({ x: e.clientX, y: e.clientY, point: null });
  }

  function onKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    if (e.target !== e.currentTarget) return;
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      select([part.id], selectMode(e));
    } else if (e.key === "F2") {
      e.preventDefault();
      onStartRename();
    }
  }

  function endRename(name: string | null, refocus: boolean) {
    if (name !== null && name.trim() !== part.name) renamePart(part.id, name);
    onEndRename();
    // After Enter or Esc keep the keyboard user on the row; after a blur the focus has already moved on.
    if (refocus) requestAnimationFrame(() => rowRef.current?.focus());
  }

  const eye = !part.visible ? "hidden" : part.transparent ? "transparent" : "visible";
  const eyeTitle = {
    visible: "Visible. Click to hide.",
    transparent: "Visible, transparent. Click to hide.",
    hidden: "Hidden. Click to show.",
  }[eye];
  const classes = ["ptree-row", "ptree-part", selected && "is-selected", !part.visible && "is-hidden"];

  return (
    <div
      ref={rowRef}
      className={classes.filter(Boolean).join(" ")}
      role="treeitem"
      aria-level={2}
      aria-posinset={position}
      aria-setsize={count}
      aria-selected={selected}
      aria-expanded={repairing || undefined}
      tabIndex={tabbable ? 0 : -1}
      onFocus={onFocus}
      onClick={(e) => select([part.id], selectMode(e))}
      onContextMenu={onContextMenu}
      onKeyDown={onKeyDown}
    >
      <button
        type="button"
        className={`ptree-icon ptree-eye is-${eye}`}
        tabIndex={-1}
        aria-label={part.visible ? "Hide part" : "Show part"}
        title={eyeTitle}
        onClick={(e) => {
          e.stopPropagation();
          setPartsVisible([part.id], !part.visible);
        }}
        onDoubleClick={stop}
      >
        {part.visible ? <Eye size={14} /> : <EyeOff size={14} />}
      </button>
      <span className="ptree-swatch" style={{ background: part.color }} aria-hidden="true" />
      {editing ? (
        <RenameInput initial={part.name} onDone={endRename} />
      ) : (
        <span
          className="ptree-name"
          title={part.name}
          onDoubleClick={(e) => {
            e.stopPropagation();
            onStartRename();
          }}
        >
          {part.name}
        </span>
      )}
      {badge && (
        <span
          className="ptree-badge"
          role="img"
          aria-label={badge}
          title={`${badge}\nDouble-click to repair.`}
          onDoubleClick={(e) => {
            e.stopPropagation();
            enterRepair(part.id);
          }}
        >
          <TriangleAlert size={13} aria-hidden="true" />
        </span>
      )}
      <button
        type="button"
        className={`ptree-icon ptree-lock ${part.locked ? "is-locked" : "is-unlocked"}`}
        tabIndex={-1}
        aria-label={part.locked ? "Unlock part" : "Lock part"}
        aria-pressed={part.locked}
        title={part.locked ? "Locked. Click to unlock." : "Click to lock."}
        onClick={(e) => {
          e.stopPropagation();
          setPartLocked(part.id, !part.locked);
        }}
        onDoubleClick={stop}
      >
        {part.locked ? <Lock size={13} /> : <LockOpen size={13} />}
      </button>
    </div>
  );
}

export interface RepairRowProps {
  /** The part in repair mode. */
  part: Part;
  tabbable: boolean;
  onFocus(): void;
}

/** "Part repair": the working copy of the part in repair mode, one level under that part until repair mode ends. */
export function RepairRow({ part, tabbable, onFocus }: RepairRowProps) {
  const selectPart = () => useStore.getState().select([part.id], "replace");

  // Like the part's own row, so the menu acts on the part being repaired.
  function onContextMenu(e: MouseEvent) {
    e.preventDefault();
    if (!useStore.getState().selection.includes(part.id)) selectPart();
    useUi.getState().openContextMenu({ x: e.clientX, y: e.clientY, point: null });
  }

  return (
    <div
      className="ptree-row ptree-repair"
      role="treeitem"
      aria-level={3}
      aria-posinset={1}
      aria-setsize={1}
      tabIndex={tabbable ? 0 : -1}
      title={`Working copy of ${part.name}`}
      onFocus={onFocus}
      onClick={selectPart}
      onContextMenu={onContextMenu}
      onKeyDown={(e) => {
        if (e.target === e.currentTarget && (e.key === "Enter" || e.key === " ")) {
          e.preventDefault();
          selectPart();
        }
      }}
    >
      <Wrench size={13} className="ptree-repair-icon" aria-hidden="true" />
      <span className="ptree-name">Part repair</span>
    </div>
  );
}

export interface MeasurementRowProps {
  part: Part;
  measurement: Measurement;
  /** 1-based place among the part's measurements. */
  number: number;
  tabbable: boolean;
  onFocus(): void;
}

/**
 * "Measurement": one of a part's measurements, one level under the part. A click picks it (and its part); a
 * double-click opens Measure, which shows it; the cross deletes it as one undo step.
 */
export function MeasurementRow({ part, measurement, number, tabbable, onFocus }: MeasurementRowProps) {
  const selected = useMeasure((m) => m.session?.selected === measurement.id);
  const label = toolById(measurement.tool)?.label ?? "Measurement";
  const pick = () => {
    useStore.getState().select([part.id], "replace");
    if (isMeasureOpen()) selectMeasurement(measurement.id);
  };
  return (
    <div
      className={`ptree-row ptree-measurement${selected ? " is-selected" : ""}`}
      role="treeitem"
      aria-level={3}
      aria-selected={selected}
      tabIndex={tabbable ? 0 : -1}
      title={`${label} on ${part.name}. Double-click to show it in Measure.`}
      onFocus={onFocus}
      onClick={pick}
      onDoubleClick={() => {
        openMeasure();
        if (isMeasureOpen()) selectMeasurement(measurement.id);
      }}
      onKeyDown={(e) => {
        if (e.target !== e.currentTarget) return;
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          pick();
        } else if (e.key === "Delete") {
          e.preventDefault();
          e.stopPropagation();
          deleteMeasurement(measurement.id);
        }
      }}
    >
      <Ruler size={13} className="ptree-repair-icon" aria-hidden="true" />
      <span className="ptree-name">
        Measurement {number}: {label}
      </span>
      <button
        type="button"
        className="ptree-icon ptree-measure-delete"
        tabIndex={-1}
        aria-label={`Delete measurement ${number}`}
        title="Delete this measurement."
        onClick={(e) => {
          e.stopPropagation();
          deleteMeasurement(measurement.id);
        }}
        onDoubleClick={stop}
      >
        <X size={13} />
      </button>
    </div>
  );
}

interface RenameInputProps {
  initial: string;
  /** `name` is null when the edit was cancelled. */
  onDone(name: string | null, refocus: boolean): void;
}

function RenameInput({ initial, onDone }: RenameInputProps) {
  const ref = useRef<HTMLInputElement>(null);
  // Enter or Esc unmounts the input, which can also fire blur; only the first ending counts.
  const done = useRef(false);
  const [value, setValue] = useState(initial);

  useEffect(() => {
    ref.current?.focus();
    ref.current?.select();
  }, []);

  function finish(name: string | null, refocus: boolean) {
    if (done.current) return;
    done.current = true;
    onDone(name, refocus);
  }

  return (
    <input
      ref={ref}
      className="ptree-rename"
      aria-label="Part name"
      spellCheck={false}
      value={value}
      onChange={(e) => setValue(e.target.value)}
      onKeyDown={(e) => {
        // Typing must not reach the app shortcuts (Del, Ctrl+Z, F, A...).
        e.stopPropagation();
        if (e.key === "Enter") finish(value, true);
        else if (e.key === "Escape") finish(null, true);
      }}
      onBlur={() => finish(value, false)}
      onClick={stop}
      onDoubleClick={stop}
      onContextMenu={stop}
    />
  );
}
