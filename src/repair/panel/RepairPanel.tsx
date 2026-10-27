// Repair mode's part of the right panel, under the Clip planes section: the Status, Actions, Repair scripts, Shells
// and View tabs, and the Apply repair and Run repair script buttons fixed at the bottom. A tab stays mounted once it
// has been shown, so what it holds (open rows, the shell list and its sort) lasts the whole repair session. While Cut
// surfaces is on, its own panel stands in their place (the tabs stay mounted, hidden).
import { useEffect, useId, useState, type KeyboardEvent, type ReactNode } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { CheckCheck, ScrollText } from "lucide-react";
import { useUi } from "../../ui";
import { CutPanel } from "../cut/CutPanel";
import { loadScripts } from "../scripts";
import { requestApply } from "../session";
import { useRepair, type RepairTab } from "../store";
import { ActionsTab } from "./ActionsTab";
import { ScriptsTab } from "./ScriptsTab";
import { ShellsTab } from "./ShellsTab";
import { StatusTab } from "./StatusTab";
import { ViewTab } from "./ViewTab";
import "../repair.css";

export type { RepairTab };

export const REPAIR_TABS: { id: RepairTab; label: string }[] = [
  { id: "status", label: "Status" },
  { id: "actions", label: "Actions" },
  { id: "scripts", label: "Repair scripts" },
  { id: "shells", label: "Shells" },
  { id: "view", label: "View" },
];

/**
 * Why the actions and scripts are missing, shown in their tabs; null once the catalog is there. In the plain browser
 * there is no Rust to ask; on the desktop it is loading, or loading failed (the toast said why).
 */
export function catalogNote(loaded: boolean, desktop: boolean, failed: boolean): string | null {
  if (loaded) return null;
  if (!desktop) return "Repair needs the tricky3D desktop app.";
  return failed ? "The repair actions could not be loaded. Close repair mode and open it again to retry." : "Loading the repair actions…";
}

export function RepairPanel() {
  const tab = useRepair((s) => s.panelTab);
  const setPanelTab = useRepair((s) => s.setPanelTab);
  const [shown, setShown] = useState<ReadonlySet<RepairTab>>(() => new Set(["status"]));
  const loaded = useRepair((s) => s.catalog !== null);
  const [failed, setFailed] = useState(false);
  const baseId = useId();
  const desktop = isTauri();
  const cutting = useRepair((s) => s.editTool === "cut");

  // Entering repair mode starts loading the catalog; this shares that load and notices when it fails.
  useEffect(() => {
    if (loaded || !desktop) return;
    let live = true;
    void loadScripts().then((ok) => {
      if (live && !ok) setFailed(true);
    });
    return () => {
      live = false;
    };
  }, [loaded, desktop]);

  const missing = catalogNote(loaded, desktop, failed);

  // A command can switch tabs while this panel is already open. Mount that tab the first time it is asked for.
  useEffect(() => {
    setShown((s) => (s.has(tab) ? s : new Set([...s, tab])));
  }, [tab]);

  function select(id: RepairTab) {
    setPanelTab(id);
    setShown((s) => (s.has(id) ? s : new Set([...s, id])));
  }

  // Arrow keys move between tabs, as in any tab list.
  function onTabKey(e: KeyboardEvent<HTMLDivElement>) {
    const i = REPAIR_TABS.findIndex((t) => t.id === tab);
    let next: number;
    if (e.key === "ArrowRight") next = (i + 1) % REPAIR_TABS.length;
    else if (e.key === "ArrowLeft") next = (i - 1 + REPAIR_TABS.length) % REPAIR_TABS.length;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = REPAIR_TABS.length - 1;
    else return;
    e.preventDefault();
    const id = REPAIR_TABS[next].id;
    select(id);
    document.getElementById(`${baseId}-tab-${id}`)?.focus();
  }

  function body(id: RepairTab): ReactNode {
    switch (id) {
      case "status":
        return <StatusTab />;
      case "actions":
        return <ActionsTab missing={missing} />;
      case "scripts":
        return <ScriptsTab missing={missing} />;
      case "shells":
        return <ShellsTab active={tab === "shells"} />;
      case "view":
        return <ViewTab />;
    }
  }

  return (
    <>
      {cutting && <CutPanel />}
      <div className="rp" hidden={cutting}>
        <div className="rp-tabs" role="tablist" aria-label="Repair" onKeyDown={onTabKey}>
          {REPAIR_TABS.map((t) => (
            <button
              key={t.id}
              id={`${baseId}-tab-${t.id}`}
              type="button"
              role="tab"
              className="rp-tab"
              aria-selected={t.id === tab}
              aria-controls={`${baseId}-panel-${t.id}`}
              tabIndex={t.id === tab ? 0 : -1}
              onClick={() => select(t.id)}
            >
              {t.label}
            </button>
          ))}
        </div>
        {REPAIR_TABS.filter((t) => shown.has(t.id)).map((t) => (
          <div
            key={t.id}
            id={`${baseId}-panel-${t.id}`}
            className="rp-panel"
            role="tabpanel"
            aria-labelledby={`${baseId}-tab-${t.id}`}
            hidden={t.id !== tab}
          >
            {body(t.id)}
          </div>
        ))}
        <RepairFooter />
      </div>
    </>
  );
}

/** Apply repair and Run repair script, fixed at the bottom of the panel. */
function RepairFooter() {
  const busy = useRepair((s) => s.busy !== null);
  return (
    <div className="rp-footer">
      <button
        type="button"
        className="clip-btn clip-btn-primary"
        title="Keep the repaired part: replace the original, or add it beside the original"
        disabled={busy}
        onClick={requestApply}
      >
        <CheckCheck size={14} aria-hidden="true" />
        Apply repair
      </button>
      <button
        type="button"
        className="clip-btn"
        title="Pick a repair script and run it on the working copy"
        disabled={busy}
        onClick={() => useUi.getState().openDialog({ kind: "repair-script-picker" })}
      >
        <ScrollText size={14} aria-hidden="true" />
        Run repair script
      </button>
    </div>
  );
}
