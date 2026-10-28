import { useId, useState, type KeyboardEvent } from "react";
import { useStore } from "../store";
import { PartsTree } from "./PartsTree";
import { Splitter, usePanelWidth, type PanelSize } from "./Splitter";
import "./layout.css";

export const SIDEBAR_SIZE: PanelSize = { storageKey: "tricky3d.sidebarWidth", min: 180, max: 480, initial: 260 };

type DockTab = "machines" | "jobs";

const DOCK_TABS: { id: DockTab; label: string; message: string }[] = [
  { id: "machines", label: "Machines", message: "Machine setups are coming in a future version." },
  { id: "jobs", label: "Jobs", message: "Build jobs are coming in a future version." },
];

export function Sidebar() {
  const [width, setWidth] = usePanelWidth(SIDEBAR_SIZE);
  const hideSoon = useStore((s) => s.hideSoon);

  return (
    <aside className="side-panel sidebar" style={{ width, minWidth: SIDEBAR_SIZE.min }} aria-label="Parts">
      <div className="side-panel-content">
        <PartsTree />
        {!hideSoon && <Dock />}
      </div>
      <Splitter edge="right" size={SIDEBAR_SIZE} width={width} onResize={setWidth} label="Resize parts sidebar" />
    </aside>
  );
}

/** The Machines | Jobs tabs at the bottom of the sidebar. Both are placeholders for now. */
function Dock() {
  const [tab, setTab] = useState<DockTab>("machines");
  const baseId = useId();
  const current = DOCK_TABS.find((t) => t.id === tab) ?? DOCK_TABS[0];

  function onKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
    e.preventDefault();
    const i = DOCK_TABS.findIndex((t) => t.id === tab);
    const next = DOCK_TABS[(i + (e.key === "ArrowRight" ? 1 : -1) + DOCK_TABS.length) % DOCK_TABS.length];
    setTab(next.id);
    document.getElementById(`${baseId}-tab-${next.id}`)?.focus();
  }

  return (
    <div className="sidebar-dock">
      <div className="sidebar-tabs" role="tablist" aria-label="Machines and jobs" onKeyDown={onKeyDown}>
        {DOCK_TABS.map((t) => (
          <button
            key={t.id}
            id={`${baseId}-tab-${t.id}`}
            type="button"
            className="sidebar-tab"
            role="tab"
            aria-selected={t.id === tab}
            aria-controls={`${baseId}-panel`}
            tabIndex={t.id === tab ? 0 : -1}
            onClick={() => setTab(t.id)}
          >
            {t.label}
            <span className="soon-pill">Soon</span>
          </button>
        ))}
      </div>
      <div id={`${baseId}-panel`} className="sidebar-dock-body" role="tabpanel" aria-labelledby={`${baseId}-tab-${tab}`}>
        {current.message}
      </div>
    </div>
  );
}
