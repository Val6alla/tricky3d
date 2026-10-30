import { useEffect } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Ribbon } from "./ribbon/Ribbon";
import { Sidebar } from "./layout/Sidebar";
import { RightPanel } from "./layout/RightPanel";
import { StatusBar } from "./layout/StatusBar";
import { Toaster } from "./layout/Toaster";
import { Viewport } from "./viewport/Viewport";
import { ContextMenu } from "./menus/ContextMenu";
import { DialogHost } from "./dialogs/DialogHost";
import { ArrangeBusy } from "./arrange/ArrangeBusy";
import { useCloseGuard, useFileDrop } from "./io/hooks";
import { useKeyboardShortcuts } from "./keyboard";
import { startBadgeChecks } from "./repair/badges";
import { isDirty, useStore } from "./store";
import "./App.css";

function useWindowTitle() {
  const projectName = useStore((s) => s.projectName);
  const dirty = useStore(isDirty);
  useEffect(() => {
    const title = `tricky3D — ${projectName}${dirty ? " *" : ""}`;
    document.title = title;
    if (isTauri()) getCurrentWindow().setTitle(title).catch(() => {});
  }, [projectName, dirty]);
}

export default function App() {
  useWindowTitle();
  useKeyboardShortcuts();
  useFileDrop();
  useCloseGuard();
  // The parts tree's damage badges: every part mesh is checked in the background, now and after each change.
  useEffect(() => startBadgeChecks(), []);

  return (
    <div className="app">
      <Ribbon />
      <div className="main-row">
        <Sidebar />
        <Viewport />
        <RightPanel />
      </div>
      <StatusBar />
      <ContextMenu />
      <DialogHost />
      <ArrangeBusy />
      <Toaster />
    </div>
  );
}
