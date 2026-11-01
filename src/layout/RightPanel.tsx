import { lazy, Suspense, useEffect, useRef, useState, type ReactNode } from "react";
import { ChevronRight } from "lucide-react";
import { useCollisions } from "../analysis/collisions";
import { shownAnalysis, useAnalysis } from "../analysis/session";
import { useBoolean } from "../boolean/store";
import { useLibrary } from "../library/store";
import { useLabel } from "../label/store";
import { useMeasure } from "../measure/store";
import { useModify } from "../modify/session";
import { useRepair } from "../repair/store";
import { useStore } from "../store";
import { useUi, type PanelSection } from "../ui";
import { ClipPanel } from "./ClipPanel";
import { CutsPanel } from "./CutsPanel";
import { InfoPanel } from "./InfoPanel";
import { QuickActions } from "./QuickActions";
import { Splitter, usePanelWidth, type PanelSize } from "./Splitter";
import "./layout.css";

// The mode panels load when a mode first opens, so the app's start parses less code.
const RepairPanel = lazy(() => import("../repair/panel/RepairPanel").then((m) => ({ default: m.RepairPanel })));
const BooleanPanel = lazy(() => import("../boolean/BooleanPanel").then((m) => ({ default: m.BooleanPanel })));
const LibraryPanel = lazy(() => import("../library/LibraryPanel").then((m) => ({ default: m.LibraryPanel })));
const MeasurePanel = lazy(() => import("../measure/MeasurePanel").then((m) => ({ default: m.MeasurePanel })));
const LabelPanel = lazy(() => import("../label/LabelPanel").then((m) => ({ default: m.LabelPanel })));
const AnalysisPanel = lazy(() => import("../analysis/AnalysisPanel").then((m) => ({ default: m.AnalysisPanel })));
const CollisionPanel = lazy(() => import("../analysis/CollisionPanel").then((m) => ({ default: m.CollisionPanel })));
const ManipulatePanel = lazy(() => import("../modify/ManipulatePanel").then((m) => ({ default: m.ManipulatePanel })));

export const RIGHT_PANEL_SIZE: PanelSize = { storageKey: "tricky3d.rightPanelWidth", min: 200, max: 480, initial: 280 };

const SECTIONS: { id: PanelSection; title: string }[] = [
  { id: "clip", title: "Clip planes" },
  { id: "cuts", title: "Cuts" },
  { id: "information", title: "Information" },
];

/**
 * In repair mode and Boolean operations the Clip planes section stays on top, and the mode's panel takes the place of
 * the others (Analyze › Information, which opens that section, is greyed out then).
 */
const MODE_SECTIONS = SECTIONS.filter((s) => s.id === "clip");

type Flash = { section: PanelSection; nonce: number };

/**
 * The right panel. Normal mode: Clip planes, Cuts and Information, with the quick-action strip at the bottom. Repair
 * mode: Clip planes, then the repair tabs with Apply repair and Run repair script at the bottom. Boolean operations:
 * Clip planes, then the two lists, the log and the settings, with the operation buttons at the bottom.
 */
export function RightPanel() {
  const [width, setWidth] = usePanelWidth(RIGHT_PANEL_SIZE);
  const [open, setOpen] = useState<Record<PanelSection, boolean>>({ clip: true, cuts: true, information: true });
  const [flash, setFlash] = useState<Flash | null>(null);
  const sectionEls = useRef<Partial<Record<PanelSection, HTMLElement | null>>>({});
  const repairing = useRepair((s) => s.session !== null);
  const booleanOpen = useBoolean((s) => s.session !== null);
  const libraryOpen = useLibrary((s) => s.session !== null);
  const measureOpen = useMeasure((s) => s.session !== null);
  const labelOpen = useLabel((s) => s.session !== null);
  const shown = useAnalysis((a) => a.shown);
  const analysisOpen = useStore((s) => shownAnalysis(shown, s.parts) !== null);
  const collisionsOpen = useCollisions((c) => c.open);
  const modifyOpen = useModify((m) => m.session !== null);
  // Manipulate mesh opened in repair mode stands in for the repair tabs until it closes.
  const mode = modifyOpen
    ? "modify"
    : repairing
    ? "repair"
    : booleanOpen
      ? "boolean"
      : libraryOpen
        ? "library"
        : labelOpen
          ? "label"
          : measureOpen
            ? "measure"
            : analysisOpen
              ? "analysis"
              : collisionsOpen
                ? "collisions"
                : null;
  // The library's gallery covers the view, so the clip planes have nothing to show.
  const sections = mode === "library" ? [] : mode ? MODE_SECTIONS : SECTIONS;

  // focusSection() is an event, not a state to mirror: react to each new nonce, even for the same section.
  useEffect(
    () =>
      useUi.subscribe((s, prev) => {
        const focus = s.focus;
        if (!focus || focus === prev.focus) return;
        setOpen((o) => (o[focus.section] ? o : { ...o, [focus.section]: true }));
        setFlash(focus);
      }),
    [],
  );

  // Runs after the commit that expanded the section, so the scroll sees its full height.
  useEffect(() => {
    if (flash) sectionEls.current[flash.section]?.scrollIntoView({ block: "nearest" });
  }, [flash]);

  // Starting or ending a mode mounts the sections anew, which would play a flash still set (one cut short by entering
  // the mode) again unasked.
  useEffect(() => setFlash(null), [mode]);

  return (
    <aside className="side-panel rpanel" style={{ width, minWidth: RIGHT_PANEL_SIZE.min }} aria-label="Inspector">
      <Splitter edge="left" size={RIGHT_PANEL_SIZE} width={width} onResize={setWidth} label="Resize right panel" />
      <div className={`side-panel-content rpanel-content ${mode ? `is-${mode}` : "has-quick"}`}>
        {sections.map(({ id, title }) => (
          <section
            key={id}
            ref={(el) => {
              sectionEls.current[id] = el;
            }}
            className="rpanel-section"
          >
            <h2 className="rpanel-heading">
              <button
                type="button"
                className="rpanel-toggle"
                aria-expanded={open[id]}
                aria-controls={`rpanel-${id}`}
                onClick={() => setOpen((o) => ({ ...o, [id]: !o[id] }))}
              >
                <ChevronRight size={14} className={`rpanel-chevron${open[id] ? " is-open" : ""}`} />
                {title}
              </button>
            </h2>
            {open[id] && (
              <div id={`rpanel-${id}`} className="rpanel-body">
                <SectionBody id={id} />
              </div>
            )}
            {flash?.section === id && (
              // Played once: the flash is over when its animation ends (a newer one keeps going).
              <span
                key={flash.nonce}
                className="rpanel-flash"
                aria-hidden="true"
                onAnimationEnd={() => setFlash((f) => (f === flash ? null : f))}
              />
            )}
          </section>
        ))}
        <Suspense fallback={null}>
          {mode === "modify" ? (
            <ManipulatePanel />
          ) : mode === "repair" ? (
            <RepairPanel />
          ) : mode === "boolean" ? (
            <BooleanPanel />
          ) : mode === "library" ? (
            <LibraryPanel />
          ) : mode === "label" ? (
            <LabelPanel />
          ) : mode === "measure" ? (
            <MeasurePanel />
          ) : mode === "analysis" ? (
            <AnalysisPanel />
          ) : mode === "collisions" ? (
            <CollisionPanel />
          ) : (
            <QuickActions />
          )}
        </Suspense>
      </div>
    </aside>
  );
}

function SectionBody({ id }: { id: PanelSection }): ReactNode {
  switch (id) {
    case "clip":
      return <ClipPanel />;
    case "cuts":
      return <CutsPanel />;
    case "information":
      return <InfoPanel />;
  }
}
