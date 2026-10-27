// Window-level file handling: drag-and-drop import and the repair and unsaved-changes questions on close.
import { useEffect } from "react";
import { isTauri } from "@tauri-apps/api/core";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { closeRepair } from "../repair/session";
import { useUi } from "../ui";
import { confirmDiscard, importPaths } from "./files";

/**
 * Keeps a Tauri event listener while an effect is mounted; returns the effect's cleanup. Listening
 * is async, so the cleanup can run before the listener exists (React StrictMode mounts, unmounts
 * and mounts again at once); the late listener is then removed as soon as it arrives.
 */
export function listenWhileMounted(listen: () => Promise<UnlistenFn>): () => void {
  let mounted = true;
  let unlisten: UnlistenFn | null = null;
  listen().then(
    (fn) => {
      if (mounted) unlisten = fn;
      else fn();
    },
    (err: unknown) => console.error("Could not listen for window events", err),
  );
  return () => {
    mounted = false;
    unlisten?.();
    unlisten = null;
  };
}

/** Imports files dropped on the window and shows the viewport's drop hint while files are dragged over it. Used once, in App. */
export function useFileDrop(): void {
  useEffect(() => {
    if (!isTauri()) return;
    const setDropActive = (active: boolean) => useUi.getState().setDropActive(active);
    return listenWhileMounted(() =>
      getCurrentWebview().onDragDropEvent(({ payload }) => {
        switch (payload.type) {
          case "enter":
            // Dragged text or links come without paths and get no hint.
            setDropActive(payload.paths.length > 0);
            break;
          case "over":
            break; // Only the position changes; the hint is already up from "enter".
          case "leave":
            setDropActive(false);
            break;
          case "drop":
            setDropActive(false);
            if (payload.paths.length) void importPaths(payload.paths);
            break;
        }
      }),
    );
  }, []);
}

/**
 * What a close request asks before the window may close: about unapplied repair work first, as New and Open do
 * (closeRepair; repair mode never changes the parts, so the unsaved-changes check alone would let it go), then about
 * unsaved changes. False keeps the window open: Cancel to either question, or a repair run still going.
 */
export async function mayCloseWindow(): Promise<boolean> {
  return (await closeRepair()) && (await confirmDiscard());
}

/**
 * Asks about unapplied repair work and unsaved changes when the window is closed (title-bar button, Alt+F4), see
 * mayCloseWindow. Used once, in App.
 * On macOS, Cmd+Q (the default app menu's Quit) ends the app without a close request, so it skips
 * this; catching it needs a custom Quit menu item on the Rust side that calls window.close().
 */
export function useCloseGuard(): void {
  useEffect(() => {
    if (!isTauri()) return;
    let asking = false;
    return listenWhileMounted(() =>
      getCurrentWindow().onCloseRequested(async (event) => {
        // A second close request while the question is up must not close the window behind it.
        if (asking) {
          event.preventDefault();
          return;
        }
        asking = true;
        try {
          if (!(await mayCloseWindow())) event.preventDefault();
        } finally {
          asking = false;
        }
      }),
    );
  }, []);
}
