// Boolean operations' part of the right panel, under the Clip planes section: the green list (main parts, kept) and
// the red list (parts to subtract or intersect with) side by side with the arrow buttons between them, the log, the
// settings, and Union, Difference, Intersection and Cancel fixed at the bottom.
import { useEffect, useId, useRef } from "react";
import { ArrowLeft, ArrowRight, Wrench } from "lucide-react";
import { NumberField } from "../repair/panel/controls";
import { progressPercent } from "../repair/dialogs/RunProgressDialog";
import { useStore, type Part } from "../store";
import { cancelBoolean, moveSelected, repairFirst, runBoolean, selectInList } from "./session";
import { useBoolean, type BooleanBusy, type BooleanList, type BooleanSession, type LogLine } from "./store";
import "../repair/repair.css";
import "./boolean.css";

const cx = (...names: (string | false | null | undefined)[]) => names.filter(Boolean).join(" ");

/** Up to `decimals` decimals, trailing zeros dropped: 0.001, 0.0001. */
export function formatTolerance(v: number, decimals = 4): string {
  return String(Number(v.toFixed(decimals)));
}

/** The arrow buttons' state: which way the selected part can move. */
export function arrowState(s: Pick<BooleanSession, "main" | "tool" | "selected">): { toTool: boolean; toMain: boolean } {
  const id = s.selected;
  return { toTool: id !== null && s.main.includes(id), toMain: id !== null && s.tool.includes(id) };
}

export function BooleanPanel() {
  const session = useBoolean((s) => s.session);
  const busy = useBoolean((s) => s.busy);
  const parts = useStore((s) => s.parts);
  if (!session) return null;
  const byId = new Map(parts.map((p) => [p.id, p]));
  const arrows = arrowState(session);
  const running = busy !== null;

  return (
    <div className="rp bp">
      <div className="rp-panel rp-body">
        <div className="bp-lists">
          <PartList list="main" title="Main parts" note="kept" ids={session.main} byId={byId} selected={session.selected} disabled={running} />
          <div className="bp-arrows" role="group" aria-label="Move the selected part">
            <button
              type="button"
              className="clip-btn bp-arrow"
              title="Move the selected part to the red list (subtract or intersect with it)"
              aria-label="Move to the red list"
              disabled={running || !arrows.toTool}
              onClick={() => moveSelected("tool")}
            >
              <ArrowRight size={14} aria-hidden="true" />
            </button>
            <button
              type="button"
              className="clip-btn bp-arrow"
              title="Move the selected part to the green list (keep it)"
              aria-label="Move to the green list"
              disabled={running || !arrows.toMain}
              onClick={() => moveSelected("main")}
            >
              <ArrowLeft size={14} aria-hidden="true" />
            </button>
          </div>
          <PartList list="tool" title="Subtract / intersect" ids={session.tool} byId={byId} selected={session.selected} disabled={running} />
        </div>
        {busy && <Progress busy={busy} />}
        <Log lines={session.log} running={running} />
        <Settings disabled={running} />
      </div>
      <div className="rp-footer bp-footer">
        <button type="button" className="clip-btn" disabled={running} title="Fuse every part in both lists" onClick={() => void runBoolean("union")}>
          Union
        </button>
        <button type="button" className="clip-btn" disabled={running} title="Subtract the red parts from the green parts" onClick={() => void runBoolean("difference")}>
          Difference
        </button>
        <button type="button" className="clip-btn" disabled={running} title="Keep only the volume the green and red parts share" onClick={() => void runBoolean("intersection")}>
          Intersection
        </button>
        <button
          type="button"
          className="clip-btn"
          disabled={!running && !session.result}
          title={running ? "Stop the operation; nothing changes" : "Discard the result shown"}
          onClick={cancelBoolean}
        >
          Cancel
        </button>
      </div>
    </div>
  );
}

interface PartListProps {
  list: BooleanList;
  title: string;
  note?: string;
  ids: readonly string[];
  byId: ReadonlyMap<string, Part>;
  selected: string | null;
  disabled: boolean;
}

/** One list. A click selects a part (in the viewport too); a double click moves it to the other list. */
function PartList({ list, title, note, ids, byId, selected, disabled }: PartListProps) {
  const headingId = useId();
  const other: BooleanList = list === "main" ? "tool" : "main";
  return (
    <section className={cx("bp-list", list === "main" ? "is-main" : "is-tool")}>
      <h3 id={headingId} className="bp-list-title">
        {title}
        {note && <span className="bp-list-note"> ({note})</span>}
      </h3>
      <ul className="bp-items" role="listbox" aria-labelledby={headingId}>
        {ids.map((id) => {
          const name = byId.get(id)?.name ?? id;
          return (
            <li
              key={id}
              role="option"
              aria-selected={selected === id}
              className={cx("bp-item", selected === id && "is-selected")}
              title={name}
              onClick={() => selectInList(id)}
              onDoubleClick={() => {
                if (disabled) return;
                selectInList(id);
                moveSelected(other);
              }}
            >
              {name}
            </li>
          );
        })}
        {ids.length === 0 && <li className="bp-empty">{list === "main" ? "No parts" : "Move parts here with the arrow"}</li>}
      </ul>
    </section>
  );
}

function Progress({ busy }: { busy: BooleanBusy }) {
  const percent = progressPercent(busy.progress);
  return (
    <div className="bp-progress" role="status">
      <div className="bp-progress-head">
        <span className="bp-progress-step">{busy.cancelling ? "Cancelling…" : (busy.step ?? `${busy.label}…`)}</span>
        {percent !== null && <span className="rp-run-percent">{percent}%</span>}
      </div>
      <div className={cx("rp-progress", percent === null && "is-indeterminate")}>
        <div className="rp-progress-fill" style={percent === null ? undefined : { width: `${percent}%` }} />
      </div>
    </div>
  );
}

/** The log: checks and results, newest at the bottom, kept in view as lines arrive. */
function Log({ lines, running }: { lines: readonly LogLine[]; running: boolean }) {
  const box = useRef<HTMLOListElement>(null);
  const last = lines[lines.length - 1]?.id;
  useEffect(() => {
    const el = box.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [last]);
  return (
    <div className="rp-group">
      <h3 className="rp-title">Log</h3>
      <ol ref={box} className="bp-log" aria-live="polite">
        {lines.map((line) => (
          <li key={line.id} className={cx("bp-log-line", `is-${line.kind}`)}>
            <span>{line.text}</span>
            {line.repair && (
              <button
                type="button"
                className="clip-btn bp-repair"
                disabled={running}
                title="Run Extended repair on this part and put the repaired part in its place"
                onClick={() => void repairFirst(line.repair!)}
              >
                <Wrench size={12} aria-hidden="true" />
                Repair first
              </button>
            )}
          </li>
        ))}
      </ol>
    </div>
  );
}

function Settings({ disabled }: { disabled: boolean }) {
  const settings = useBoolean((s) => s.settings);
  const set = useBoolean((s) => s.setSettings);
  const degenerateId = useId();
  const noiseId = useId();
  return (
    <div className="rp-group">
      <h3 className="rp-title">Settings</h3>
      <div className="rp-checks">
        <label className="clip-check">
          <input type="checkbox" checked={settings.removeOriginals} disabled={disabled} onChange={(e) => set({ removeOriginals: e.target.checked })} />
          Remove original parts
        </label>
        <div className="bp-setting">
          <label className="clip-check">
            <input type="checkbox" checked={settings.removeDegenerate} disabled={disabled} onChange={(e) => set({ removeDegenerate: e.target.checked })} />
            Remove degenerate faces
          </label>
          <span className="bp-tolerance">
            <label htmlFor={degenerateId}>Tolerance</label>
            <NumberField
              id={degenerateId}
              value={settings.degenerateTolerance}
              format={(v) => formatTolerance(v)}
              unit="mm"
              chars={6}
              disabled={disabled || !settings.removeDegenerate}
              onCommit={(v) => v > 0 && set({ degenerateTolerance: v })}
            />
          </span>
        </div>
        <div className="bp-setting">
          <label className="clip-check">
            <input type="checkbox" checked={settings.filterNoise} disabled={disabled} onChange={(e) => set({ filterNoise: e.target.checked })} />
            Filter noise shells
          </label>
          <span className="bp-tolerance">
            <label htmlFor={noiseId}>Tolerance</label>
            <NumberField
              id={noiseId}
              value={settings.noiseVolume}
              format={(v) => formatTolerance(v, 6)}
              unit="cm³"
              chars={6}
              disabled={disabled || !settings.filterNoise}
              onCommit={(v) => v > 0 && set({ noiseVolume: v })}
            />
          </span>
        </div>
      </div>
    </div>
  );
}
