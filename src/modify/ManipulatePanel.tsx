// The Manipulate mesh panel in the right panel: a tab per tool, laid out as the help lists each tool's controls (no
// reference shot: the layout is unconfirmed), then Display and settings (Highlight triangles, Show original held),
// the triangle count before and after, and OK and Close.
import { useId, useState, type ReactNode } from "react";
import { SliderNumber } from "../analysis/AnalysisDialog";
import "../analysis/analysis.css";
import { PresetRow } from "../dialogs/PresetRow";
import { fitPreset, TOOL_PRESETS } from "../dialogs/presets";
import { formatCount } from "../info";
import { useRepair } from "../repair/store";
import "./modify.css";
import {
  applyManipulate,
  calculate,
  closeManipulate,
  editSettings,
  previewIsCurrent,
  setHighlight,
  setShowOriginal,
  setTab,
  useModify,
} from "./session";
import {
  DEFAULT_SETTINGS,
  NEEDS_SELECTION,
  REMESH_METHODS,
  TAB_LABELS,
  TABS,
  type ExtrudeDirection,
  type ModifyTab,
  type RemeshMethod,
  type TabSettings,
} from "./types";

function Box({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="dlg-section md-box" aria-label={title}>
      <h3 className="dlg-section-title">{title}</h3>
      {children}
    </section>
  );
}

function Check({ label, help, checked, disabled, onChange }: { label: string; help?: string; checked: boolean; disabled?: boolean; onChange(v: boolean): void }) {
  return (
    <label className={`dlg-check${disabled ? " is-disabled" : ""}`} title={help}>
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
      <span>
        <span className="dlg-check-label">{label}</span>
        {help && <span className="dlg-help">{help}</span>}
      </span>
    </label>
  );
}

/** A number field with its unit that commits on Enter or leaving it. */
function NumberField({ label, value, unit, min, max, disabled, onChange }: { label: string; value: number; unit?: string; min: number; max: number; disabled?: boolean; onChange(v: number): void }) {
  const id = useId();
  const [text, setText] = useState(String(value));
  const [shown, setShown] = useState(value);
  if (shown !== value) {
    setShown(value);
    setText(String(value));
  }
  const commit = (raw: string) => {
    const v = Number(raw.replace(",", "."));
    if (Number.isFinite(v)) onChange(Math.min(max, Math.max(min, v)));
    else setText(String(value));
  };
  return (
    <div className="an-slider-row md-field">
      <label className="dlg-label" htmlFor={id}>
        {label}
      </label>
      <span className="an-number">
        <input
          id={id}
          className="dlg-input"
          inputMode="decimal"
          value={text}
          disabled={disabled}
          onChange={(e) => setText(e.target.value)}
          onBlur={(e) => commit(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") commit((e.target as HTMLInputElement).value);
          }}
        />
        {unit && <span className="dlg-unit">{unit}</span>}
      </span>
    </div>
  );
}

const SELECTED_HELP = "Change only the selected triangles. Select triangles in repair mode first.";

function ReduceTab({ s, selected }: { s: TabSettings["reduce"]; selected: boolean }) {
  const edit = (patch: Partial<TabSettings["reduce"]>) => editSettings("reduce", patch);
  return (
    <>
      <Box title="Target">
        <NumberField label="Target triangle count" value={s.target} min={4} max={1e9} disabled={s.lowest} onChange={(v) => edit({ target: Math.round(v) })} />
        <Check label="Set target as low as possible" checked={s.lowest} onChange={(lowest) => edit({ lowest })} help="Reduce as far as the limits below allow." />
      </Box>
      <Box title="Limits">
        <SliderNumber label="Max. deformation" value={s.maxDeviation} min={0.0001} max={100} sliderMax={1} step={0.005} unit="mm" onChange={(maxDeviation) => edit({ maxDeviation })} />
        <NumberField label="Max. edge length" value={s.maxEdge} unit="mm" min={0.001} max={1e6} onChange={(maxEdge) => edit({ maxEdge })} />
        <p className="dlg-note">No point of the surface moves further than Max. deformation, and no edge grows longer than Max. edge length.</p>
      </Box>
      <Box title="Options">
        <Check label="Selected triangles only" checked={selected && s.selectedOnly} disabled={!selected} onChange={(selectedOnly) => edit({ selectedOnly })} help={SELECTED_HELP} />
        <Check
          label="Fast mode"
          checked={s.fast}
          onChange={(fast) => edit({ fast })}
          help="Stop as soon as the target is reached. Quicker, but some areas may keep their fine triangles."
        />
      </Box>
    </>
  );
}

function SmoothTab({ s, selected }: { s: TabSettings["smooth"]; selected: boolean }) {
  const edit = (patch: Partial<TabSettings["smooth"]>) => editSettings("smooth", patch);
  return (
    <>
      <Box title="Smoothing">
        <SliderNumber label="Strength" value={s.strength} min={0.01} max={1} step={0.01} unit="" onChange={(strength) => edit({ strength })} />
        <NumberField label="Iterations" value={s.iterations} min={1} max={200} onChange={(v) => edit({ iterations: Math.round(v) })} />
        <p className="dlg-note">Each iteration pulls every point towards its neighbours, then pushes it back a little, which flattens roughness without shrinking the part.</p>
      </Box>
      <Box title="Options">
        <Check label="Keep sharp edges" checked={s.keepEdges} onChange={(keepEdges) => edit({ keepEdges })} help="Edges where the surface turns by more than the angle stay sharp." />
        <NumberField label="Sharper than" value={s.edgeAngle} unit="°" min={0} max={180} disabled={!s.keepEdges} onChange={(edgeAngle) => edit({ edgeAngle })} />
        <Check label="Keep the volume" checked={s.keepVolume} onChange={(keepVolume) => edit({ keepVolume })} help="Give a closed part its volume back after smoothing." />
        <Check label="Selected triangles only" checked={selected && s.selectedOnly} disabled={!selected} onChange={(selectedOnly) => edit({ selectedOnly })} help={SELECTED_HELP} />
      </Box>
    </>
  );
}

function RefineTab({ s, selected }: { s: TabSettings["refine"]; selected: boolean }) {
  const edit = (patch: Partial<TabSettings["refine"]>) => editSettings("refine", patch);
  return (
    <Box title="Refine">
      <NumberField label="Max. edge length" value={s.maxEdge} unit="mm" min={0.001} max={1e6} onChange={(maxEdge) => edit({ maxEdge })} />
      <Check label="Selected triangles only" checked={selected && s.selectedOnly} disabled={!selected} onChange={(selectedOnly) => edit({ selectedOnly })} help={SELECTED_HELP} />
      <p className="dlg-note">Triangles are split until no edge is longer than this. The shape does not change.</p>
    </Box>
  );
}

function Choice<T extends string>({ label, value, options, onChange }: { label: string; value: T; options: { value: T; label: string; soon?: boolean }[]; onChange(v: T): void }) {
  const id = useId();
  return (
    <div className="an-slider-row an-select-row">
      <label className="dlg-label" htmlFor={id}>
        {label}
      </label>
      <select id={id} className="dlg-input dlg-select" value={value} onChange={(e) => onChange(e.target.value as T)}>
        {options.map((o) => (
          <option key={o.value} value={o.value} disabled={o.soon}>
            {o.soon ? `${o.label} (Soon)` : o.label}
          </option>
        ))}
      </select>
    </div>
  );
}

const METHOD_NOTES: Record<RemeshMethod, string> = {
  volumetric: "Rebuilds the part on a grid: simple and fast. Needs a closed part.",
  uniform: "Rebuilds the surface with triangles as even in size and angles as it can, holes and all.",
  adaptive: "Small triangles where the surface curves, large ones where it is flat.",
  manifold: "Rebuilds a closed, manifold skin round everything, holes and open sheets too: the result is about half an edge length thicker.",
};

function RemeshTab({ s, selected }: { s: TabSettings["remesh"]; selected: boolean }) {
  const edit = (patch: Partial<TabSettings["remesh"]>) => editSettings("remesh", patch);
  const whole = s.method === "volumetric" || s.method === "manifold";
  const surfaceMethod = s.method === "uniform" || s.method === "adaptive";
  return (
    <>
      <Box title="Show slim faces">
        <Check label="Coloured" checked={s.slimColored} onChange={(slimColored) => edit({ slimColored })} help="Colour triangles with an angle under the smallest angle, yellow to red." />
        <SliderNumber label="Smallest angle" value={s.slimAngle} min={0} max={20} step={0.5} unit="°" disabled={!s.slimColored} onChange={(slimAngle) => edit({ slimAngle })} />
      </Box>
      <Box title="Method">
        <Choice label="Remeshing" value={s.method} options={REMESH_METHODS as { value: RemeshMethod; label: string; soon?: boolean }[]} onChange={(method) => edit({ method })} />
        <p className="dlg-note">{METHOD_NOTES[s.method]}</p>
        {s.method === "adaptive" ? (
          <>
            <NumberField label="Small edge length" value={s.edge} unit="mm" min={0.01} max={1e5} onChange={(edge) => edit({ edge })} />
            <NumberField label="Large edge length" value={s.largeEdge} unit="mm" min={0.01} max={1e5} onChange={(largeEdge) => edit({ largeEdge })} />
          </>
        ) : (
          <NumberField label="Target edge length" value={s.edge} unit="mm" min={0.01} max={1e5} onChange={(edge) => edit({ edge })} />
        )}
        {s.method === "volumetric" && (
          <Check label="Fast mode" checked={s.fast} onChange={(fast) => edit({ fast })} help="Leave out putting the new points back on the surface: quicker, slightly off the original shape." />
        )}
        {surfaceMethod && (
          <>
            <SliderNumber label="Smoothing" value={s.smoothing} min={0} max={1} step={0.01} unit="" onChange={(smoothing) => edit({ smoothing })} />
            <Check label="Keep sharp edges" checked={s.keepEdges} onChange={(keepEdges) => edit({ keepEdges })} help="Edges where the surface turns by more than the angle stay sharp." />
            <NumberField label="Sharper than" value={s.edgeAngle} unit="°" min={0} max={180} disabled={!s.keepEdges} onChange={(edgeAngle) => edit({ edgeAngle })} />
          </>
        )}
        <Check
          label="Selected triangles only"
          checked={selected && !whole && s.selectedOnly}
          disabled={!selected || whole}
          onChange={(selectedOnly) => edit({ selectedOnly })}
          help={whole ? "Volumetric and Manifold remeshing rebuild the whole part." : SELECTED_HELP}
        />
      </Box>
    </>
  );
}

function ZCompTab({ s }: { s: TabSettings["zcomp"] }) {
  const edit = (patch: Partial<TabSettings["zcomp"]>) => editSettings("zcomp", patch);
  return (
    <Box title="Z-compensation">
      <NumberField label="Distance" value={s.distance} unit="mm" min={0.001} max={10} onChange={(distance) => edit({ distance })} />
      <SliderNumber label="Threshold angle" value={s.angle} min={0} max={89} step={0.5} unit="°" onChange={(angle) => edit({ angle })} />
      <p className="dlg-note">
        Surfaces facing down, tilted against the platform by at most the angle, move up by the distance, to make up for a printer that cures or melts
        deeper than the layer. The walls round them get shorter; the part stays closed.
      </p>
    </Box>
  );
}

const DIRECTIONS: { value: ExtrudeDirection; label: string }[] = [
  { value: "normal", label: "Along the surface's normal" },
  { value: "x", label: "Along X" },
  { value: "y", label: "Along Y" },
  { value: "z", label: "Along Z" },
];

function ExtrudeTab({ s }: { s: TabSettings["extrude"] }) {
  const edit = (patch: Partial<TabSettings["extrude"]>) => editSettings("extrude", patch);
  return (
    <Box title="Extrude">
      <NumberField label="Distance" value={s.distance} unit="mm" min={-1000} max={1000} onChange={(distance) => edit({ distance })} />
      <Choice label="Direction" value={s.direction} options={DIRECTIONS} onChange={(direction) => edit({ direction })} />
      <Check
        label="Add side walls"
        checked={s.walls}
        onChange={(walls) => edit({ walls })}
        help="New triangles join the moved surface to where it was. Without, the triangles round it stretch to follow."
      />
      <p className="dlg-note">Moves the selected triangles together; a negative distance pushes them in.</p>
    </Box>
  );
}

function OffsetTab({ s }: { s: TabSettings["offset"] }) {
  const edit = (patch: Partial<TabSettings["offset"]>) => editSettings("offset", patch);
  return (
    <Box title="Offset a selection">
      <NumberField label="Distance" value={s.distance} unit="mm" min={-1000} max={1000} onChange={(distance) => edit({ distance })} />
      <NumberField label="Smoothed transition" value={s.transition} unit="mm" min={0} max={1000} onChange={(transition) => edit({ transition })} />
      <p className="dlg-note">Each point of the selected triangles moves along its own normal. With a transition, the points round the selection follow less and less over that width.</p>
    </Box>
  );
}

function HolesTab({ s, selected }: { s: TabSettings["holes"]; selected: boolean }) {
  const edit = (patch: Partial<TabSettings["holes"]>) => editSettings("holes", patch);
  return (
    <Box title="Offset hole edges">
      <NumberField label="Offset distance" value={s.distance} unit="mm" min={0.001} max={1000} onChange={(distance) => edit({ distance })} />
      <Check label="Selected triangles only" checked={selected && s.selectedOnly} disabled={!selected} onChange={(selectedOnly) => edit({ selectedOnly })} help="Only the hole edges of the selected triangles." />
      <p className="dlg-note">A band of new triangles grows from each hole's edge, this far out across the hole, in the plane of the triangles beside it.</p>
    </Box>
  );
}

export function ManipulatePanel() {
  const state = useModify();
  const repairSelected = useRepair((r) => (r.session?.selectedCount ?? 0) > 0);
  const { session, settings, busy, highlight } = state;
  if (!session) return null;
  const selected = session.target.kind === "repair" && repairSelected;
  const preview = session.preview;
  const current = previewIsCurrent(state);
  const before = preview?.before ?? session.base.indices.length / 3;
  const tab = session.tab;
  const needsSelection = NEEDS_SELECTION.has(tab) && !selected;
  return (
    <section className="an-panel md-panel" aria-label="Manipulate mesh">
      <div className="ms-tabs an-tabs md-tabs" role="tablist">
        {TABS.map((t) => (
          <button key={t} type="button" role="tab" aria-selected={tab === t} className={`ms-tab${tab === t ? " is-selected" : ""}`} onClick={() => setTab(t)}>
            {TAB_LABELS[t]}
          </button>
        ))}
      </div>
      <div className="an-body">
        <PresetRow
          key={tab}
          store={TOOL_PRESETS}
          scope={tab}
          values={settings[tab] as unknown as Record<string, unknown>}
          onLoad={(saved) => editSettings(tab, fitPreset(saved, DEFAULT_SETTINGS[tab] as ModifyToolSettings) as Partial<TabSettings[ModifyTab]>)}
        />
        {tab === "reduce" && <ReduceTab s={settings.reduce} selected={selected} />}
        {tab === "smooth" && <SmoothTab s={settings.smooth} selected={selected} />}
        {tab === "refine" && <RefineTab s={settings.refine} selected={selected} />}
        {tab === "remesh" && <RemeshTab s={settings.remesh} selected={selected} />}
        {tab === "zcomp" && <ZCompTab s={settings.zcomp} />}
        {tab === "extrude" && <ExtrudeTab s={settings.extrude} />}
        {tab === "offset" && <OffsetTab s={settings.offset} />}
        {tab === "holes" && <HolesTab s={settings.holes} selected={selected} />}
        {needsSelection && (
          <p className="an-stale" role="note">
            {session.target.kind === "repair"
              ? "Select the triangles to move first."
              : "This tool moves selected triangles: open the part in repair mode and select them."}
          </p>
        )}
        <div className="an-row md-calc">
          <button type="button" className="dlg-btn" disabled={busy !== null || needsSelection} onClick={() => void calculate()} title="Work out the result and show it in place of the part.">
            Calculate
          </button>
          <span className="md-count" role="status" aria-label="Triangles">
            {formatCount(before)} → {preview ? formatCount(preview.after) : "?"} triangles
          </span>
        </div>
        {busy && (
          <p className="rpanel-muted" role="status">
            {busy.stage ?? busy.label}…
          </p>
        )}
        {preview && <p className="md-message">{preview.message}</p>}
        {preview && !current && <p className="an-stale">The settings changed since Calculate. OK calculates again first.</p>}
        <Box title="Display and settings">
          <Check label="Highlight triangles" checked={highlight} onChange={setHighlight} help="Draw the edges of the triangles." />
          <button
            type="button"
            className="dlg-btn md-original"
            disabled={!preview}
            aria-pressed={session.showOriginal}
            onPointerDown={() => setShowOriginal(true)}
            onPointerUp={() => setShowOriginal(false)}
            onPointerLeave={() => setShowOriginal(false)}
            onKeyDown={(e) => {
              if (e.key === " " || e.key === "Enter") setShowOriginal(true);
            }}
            onKeyUp={() => setShowOriginal(false)}
            onBlur={() => setShowOriginal(false)}
            title="Hold to see the part as it was."
          >
            Show original
          </button>
        </Box>
      </div>
      <div className="an-actions">
        <button type="button" className="dlg-btn dlg-btn-primary" disabled={busy !== null || needsSelection} onClick={() => void applyManipulate()}>
          OK
        </button>
        <button type="button" className="dlg-btn" onClick={closeManipulate}>
          Close
        </button>
      </div>
    </section>
  );
}

type ModifyToolSettings = TabSettings[ModifyTab];
